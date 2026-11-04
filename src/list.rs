//! The display list, version 7: a Caster document as plain data, in the order the writer paints it, written by hand from Caster's `src/displayList.ts`, whose TSDoc each member's doc comment carries. This module is the schema's authority on the Rust side; Caster's `displayList.ts` and `displayList/contract.ts` are the authority it follows.
//!
//! Every struct is read by hand ([`crate::read`]) and refuses a field it does not name, so a field added on the JavaScript side is a parse failure here and never a silent omission, and `version` is read before anything else and refused unless it is 7 ([`parse`]). The list arrives as the JSON `JSON.stringify` makes of it with `resources.blobs` removed, which is the fixture format's list file; the blobs arrive beside it, keyed by their names ([`Blobs`]). JSON hides seven sharp edges, and each binds here:
//!
//! - **Offsets are UTF-16 code units.** `text`, `segments.start` and `glyphs.clusters` count UTF-16 code units, so a text the writer writes is a [`Utf16`], the one string type that takes the lone surrogate `JSON.stringify` writes as `\udXXX` and a DOM text can hold ([`crate::read`]).
//! - **Numbers are doubles, parsed exactly.** Rust's correctly rounded `f64::from_str` on each literal, as serde_json with `float_roundtrip` read it ([`crate::read`]), so every number is the double JavaScript parsed, and every number of the list is a [`Num`], which writes an integral value back as JSON writes it, `-0` as `0`.
//! - **`round3` is JavaScript's rounding.** `Math.round(n * 1000) / 1000`, and every other `Math.round(n * k) / k` the writer uses, rounds a half toward positive infinity, where Rust's `f64::round` rounds it away from zero; forme's is [`crate::content::js_round`].
//! - **A number token is written as pdf-lib writes it**, by [`crate::content::js_number`]: the shortest round-trip decimal with no exponent, and `-0` written `0`.
//! - **Order is data in four arrays and in no object.** `resources.fonts` is the order of `F0`, `F1`, …; `structure.elements` is the allocation order; an element's `children` is its `/K`; a page's `items` is its painting order. No object's key order is read: the integer-keyed tables (`images`, `rasters`, `emoji`) are a [`Table`], iterated in ascending numeric order as a JavaScript engine iterates them, and an attribute object is written `/O` first and then in the schema's key order (`ATTRIBUTE_KEYS` in `validate.rs`).
//! - **Blobs are named by their SHA-256**, in lowercase hex, and a blob whose bytes are not its name is refused by the writer.
//! - **Widths are the authority.** `widths.gids` ascending, one unit each; `/W` is those numbers and the embedded `hmtx` must equal them to the unit; glyph 0 has no width (*the widths are HarfBuzz's, the shaper's own*, Caster's DESIGN.md S99).

use std::collections::BTreeMap;

#[cfg(feature = "serde")]
use serde::{Serialize, Serializer};

use crate::Refusal;
use crate::read::{Read, Value, invalid, object, str_of};
use crate::{read_enum, read_struct};

/// The bytes the list names, keyed by their SHA-256 in lowercase hex: font programs, image files, decoded pixels, a facsimile's PDF, the colour story's ICC profiles. The pool's order is not data.
pub type Blobs = BTreeMap<String, Vec<u8>>;

/// The list's integer-keyed tables (`resources.images`, `rasters`, `emoji`), keyed by the index a JavaScript object holds as a string and iterated in ascending numeric order, as a JavaScript engine iterates an object's array-index keys.
pub type Table<T> = BTreeMap<u32, T>;

/// A PDF matrix, `[a b c d e f]`: x′ = ax + cy + e, y′ = bx + dy + f.
pub type Matrix = [Num; 6];

/// A PDF rectangle, `[llx lly urx ury]`, in points.
pub type BoxPt = [Num; 4];

/// A path as a flat list of segments: a subpath begins at an `m` and a closed one ends in an `h`.
pub type Path = Vec<PathSegment>;

/// A number of the list: the double JavaScript holds, parsed exactly, written back as `JSON.stringify` writes it (an integral value without a fraction, `-0` as `0`).
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Num(pub f64);

impl Num {
    /// The value as an index, when it is one: a non-negative integer, as `Number.isInteger` and an array's bounds would take it.
    pub fn index(self) -> Option<usize> {
        (self.0 >= 0.0 && self.0.fract() == 0.0 && self.0 < 9.0e15).then_some(self.0 as usize)
    }
}

#[cfg(feature = "serde")]
impl Serialize for Num {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.0.fract() == 0.0 && self.0.abs() < 9.2e18 { s.serialize_i64(self.0 as i64) } else { s.serialize_f64(self.0) }
    }
}

impl Read for Num {
    fn read(v: &Value) -> Result<Self, String> {
        match v {
            Value::Num(n) => Ok(Num(*n)),
            other => Err(invalid(other, "a number")),
        }
    }
}

/// A text as JavaScript holds it, in UTF-16 code units, a lone surrogate included: the strings the writer writes as text (a run's `text`, an `/ActualText`, a title, a link's `/Contents`, an `/Alt`, an `/E`), whose offsets the list counts in code units.
///
/// The one string type that takes a lone surrogate, which `JSON.stringify` writes as `\udXXX` and the reader keeps as the code unit it names ([`Value::Lone`]); every other string field refuses one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Utf16(pub Vec<u16>);

impl Utf16 {
    /// The text as Rust holds it, each lone surrogate replaced by U+FFFD, as `TextEncoder` does.
    pub fn lossy(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }
}

#[cfg(feature = "serde")]
impl Serialize for Utf16 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match String::from_utf16(&self.0) {
            Ok(text) => s.serialize_str(&text),
            Err(_) => serde_json::value::RawValue::from_string(json_string(&self.0)).map_err(serde::ser::Error::custom)?.serialize(s),
        }
    }
}

impl Read for Utf16 {
    fn read(v: &Value) -> Result<Self, String> {
        match v {
            Value::Str(s) => Ok(Utf16(s.encode_utf16().collect())),
            Value::Lone(units) => Ok(Utf16(units.clone())),
            other => Err(invalid(other, "a string")),
        }
    }
}

/// `JSON.stringify` of a string held as UTF-16: quote, backslash and the control characters escaped as it escapes them, a lone surrogate as `\udXXX` in lowercase, everything else as itself.
pub fn json_string(units: &[u16]) -> String {
    let mut out = String::from("\"");
    for unit in char::decode_utf16(units.iter().copied()) {
        match unit {
            Ok('"') => out.push_str("\\\""),
            Ok('\\') => out.push_str("\\\\"),
            Ok('\u{08}') => out.push_str("\\b"),
            Ok('\u{0C}') => out.push_str("\\f"),
            Ok('\n') => out.push_str("\\n"),
            Ok('\r') => out.push_str("\\r"),
            Ok('\t') => out.push_str("\\t"),
            Ok(c) if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            Ok(c) => out.push(c),
            Err(lone) => out.push_str(&format!("\\u{:04x}", lone.unpaired_surrogate())),
        }
    }
    out.push('"');
    out
}

/// A string-valued field that holds one literal: written as it, and anything else refused.
macro_rules! literal {
    ($name:ident, $text:literal) => {
        #[doc = concat!("The literal `", $text, "`.")]
        #[derive(Clone, Copy, Debug, Default, PartialEq)]
        pub struct $name;
        #[cfg(feature = "serde")]
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str($text)
            }
        }
        impl Read for $name {
            fn read(v: &Value) -> Result<Self, String> {
                let found = str_of(v)?;
                if found == $text { Ok($name) } else { Err(format!("expected {:?}, found {:?}", $text, found)) }
            }
        }
    };
}

literal!(KindText, "text");
literal!(KindShape, "shape");
literal!(KindGroup, "group");
literal!(KindForm, "form");
literal!(KindImage, "image");
literal!(KindRaster, "raster");
literal!(KindMarks, "marks");
literal!(KindLink, "link");
literal!(KindTag, "tag");
literal!(TableRasters, "rasters");
literal!(TableEmoji, "emoji");

/// The whole document, as the writer needs it and nothing else.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct DisplayList {
    /// 7; [`parse`] refuses any other before it reads the rest.
    pub version: Num,
    pub info: DocumentInfo,
    pub writing: Writing,
    pub resources: Resources,
    /// The structure tree, resolved. Version 4.
    pub structure: Structure,
    /// The headings a bookmark points at, in order; empty where the document asked for no outline.
    pub outline: Vec<OutlineEntry>,
    /// The runs of `/PageLabels`; empty where no page declared one.
    pub page_labels: Vec<PageLabelRun>,
    pub pages: Vec<Page>,
}

impl DisplayList {
    /// Every blob key the list names, in the order the validator asks for them: the fonts, the images, the rasters, the colour clusters, the colour story's profiles.
    pub fn blob_keys(&self) -> Vec<&str> {
        let r = &self.resources;
        let mut keys: Vec<&str> = r.fonts.iter().map(|f| f.blob.as_str()).collect();
        for image in r.images.values() {
            keys.extend(image.blob.as_deref());
            keys.extend(image.jpeg.as_deref());
            if let Some(samples) = &image.samples {
                keys.push(&samples.blob);
                keys.extend(samples.palette.as_deref());
                keys.extend(samples.icc.as_deref());
            }
        }
        for raster in r.rasters.values() {
            keys.extend(raster.blob.as_deref());
            keys.extend(raster.jpeg.as_deref());
        }
        keys.extend(r.emoji.values().filter_map(|e| e.blob.as_deref()));
        if let Some(colour) = &self.writing.colour {
            keys.push(&colour.profiles.cmyk);
            keys.push(&colour.profiles.srgb);
        }
        keys
    }
}

/// The version this module reads.
pub const LIST_VERSION: f64 = 7.0;

/// Read a list from its JSON: the version first, with the validator's sentence, then the whole schema.
///
/// # Errors
/// A list that is not JSON; then a list of another version, in the sentence `validateList` writes; then a list that is not this schema, naming where as a JSON pointer.
pub fn parse(json: &str) -> Result<DisplayList, Refusal> {
    let value = crate::read::parse(json).map_err(|e| Refusal::new(format!("forme: the display list is not JSON: {e}")))?;
    let found = match &value {
        Value::Object(_) => value.member("version").filter(|v| **v != Value::Null),
        other => return Err(Refusal::new(format!("forme: the display list is not JSON: {}, expected an object", other.what()))),
    };
    if !matches!(found, Some(Value::Num(n)) if *n == LIST_VERSION) {
        let shown = match found {
            None => "undefined".to_string(),
            Some(Value::Num(n)) => crate::content::js_string(*n),
            Some(Value::Str(s)) => s.clone(),
            Some(Value::Bool(b)) => b.to_string(),
            Some(other) => other.what().to_string(),
        };
        return Err(Refusal::new(format!("caster: the writer reads a display list of version {}, and this one is version {shown}; a list is written again by print(), never upgraded", crate::content::js_string(LIST_VERSION))));
    }
    DisplayList::read(&value).map_err(|e| Refusal::new(format!("forme: the display list is not the schema of version 7: at {e}")))
}

/// What the file says of itself.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct DocumentInfo {
    /// The catalog `/Lang`.
    pub lang: String,
    pub title: Utf16,
    /// `tagged` once a guess in the tree withdrew the claim (*the printer guesses, says so, and withdraws the claim*, S90).
    pub conformance: Conformance,
    pub page_count: Num,
    /// The moment the file says it was made: an ISO 8601 instant in UTC, `YYYY-MM-DDTHH:mm:ss` with optional fractional seconds and a final `Z`, as `Date.prototype.toISOString` writes it. The writer writes it at whole seconds into both `/CreationDate` and `/ModDate`, and reads no clock. Version 3.
    pub created: String,
}

/// Whether the file claims PDF/UA-2.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Conformance {
    Pdfua2,
    Tagged,
}

/// How the writer writes what the rest of the list holds: the choices `print()` made from the flags. They change bytes and are not facts about the document, so they are kept apart from `info`. Version 3.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct Writing {
    /// How every font is written: subset or whole, the decimals of `/W`, the CFF container, `/DW` from the most common width.
    pub font_profile: FontProfile,
    /// The colour story, written when `print()` decided the document has one. Absent, every colour is `rg` from the computed style, no page carries a `/ColorSpace`, the catalog carries no `/OutputIntents`, and a page with print marks is refused. The decision is the walk's; the list records it and nothing re-derives it.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub colour: Option<ColourWriting>,
}

/// The colour story's objects, as the writer is to write them.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct ColourWriting {
    /// The two ICC profiles, each a key into the blobs: the CMYK output profile of the output intent, and the sRGB profile `/DefaultRGB` points at.
    pub profiles: Profiles,
    /// The output intent's `/OutputConditionIdentifier`.
    pub condition: String,
    /// The output intent's `/Info`, a description of the condition for a person.
    pub info: String,
    /// The `blackText` flag: `100k` writes black text as `0 0 0 1 k`.
    pub black_text: BlackTextPolicy,
}

/// The colour story's two profiles, as blob keys.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Profiles {
    pub cmyk: String,
    pub srgb: String,
}

/// What the black-text option does with an unhinted `rgb(0 0 0)` text fill.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum BlackTextPolicy {
    #[cfg_attr(feature = "serde", serde(rename = "none"))]
    None,
    #[cfg_attr(feature = "serde", serde(rename = "100k"))]
    HundredK,
}

/// How every font is written.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct FontProfile {
    /// Whether to embed only the glyphs the document shows.
    pub subset: bool,
    /// Decimals in the /W array. The emitter's pen follows the written value.
    pub width_decimals: Num,
    /// Whether CFF goes in bare as /CIDFontType0C or wrapped as /OpenType.
    pub cff_container: CffContainer,
    /// Whether /DW is the most common width rather than the 1000 default.
    pub dw_from_mode: bool,
}

/// Whether CFF goes in bare or wrapped.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum CffContainer {
    Bare,
    Opentype,
}

/// What the items name by key. The bytes behind them are the [`Blobs`], which the list file does not carry.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct Resources {
    /// Every font instance the text is set in, each carrying its key. The order is data: the writer embeds the instances in this order and names them `F0`, `F1`, … on every page, so a reader keeps the array as it is and never re-sorts it.
    pub fonts: Vec<FontInstance>,
    /// Keyed by the walk's image index.
    pub images: Table<ImageSource>,
    /// Keyed by the form's name: content written once and drawn wherever a `form` item names it.
    pub forms: BTreeMap<String, FormDefinition>,
    /// Keyed by the index of the raster fallback's spec, in the order the print drew them.
    pub rasters: Table<RasterSource>,
    /// Keyed by the cluster's index in the measurement.
    pub emoji: Table<EmojiSource>,
}

/// Content written once into a form XObject.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct FormDefinition {
    /// `[llx lly urx ury]` in the form's own space.
    pub bbox: BoxPt,
    pub items: Vec<VectorItem>,
}

/// One font instance: a face pinned at its coordinates.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct FontInstance {
    /// The instance key (the face and the coordinates it is pinned at), which a text item's `font` names. Unique in `fonts`.
    pub key: String,
    /// Key into the blobs: the font program, unwrapped to an SFNT.
    pub blob: String,
    /// Where the page loaded it from; the report names the face by it.
    pub url: String,
    pub face_id: String,
    /// The CSS family, for the report.
    pub family: String,
    /// What the page loaded, which is what the report calls the source format.
    pub source_format: SourceFormat,
    pub coords: BTreeMap<String, Num>,
    /// The face's units per em, which a text item's `glyphs.advances` are in. Absent on an instance no text is set in.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub upem: Option<Num>,
    /// The instance's own advance of every glyph its text items show and of every space glyph they name: the authority for the font's widths.
    pub widths: FontWidths,
}

/// What the page loaded a face as.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum SourceFormat {
    Ttf,
    Otf,
    Woff,
    Woff2,
}

/// An instance's widths, as a struct of arrays sorted by glyph id, one entry per glyph: HarfBuzz's advance at the instance's coordinates, unkerned. The writer's `/W` is these numbers and nothing else, it refuses a glyph that is not here, and the program it embeds must carry an `hmtx` equal to them to the unit (*the widths are HarfBuzz's, the shaper's own*, S99). Version 3.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct FontWidths {
    /// Glyph ids, ascending.
    pub gids: Vec<Num>,
    /// The advance of the glyph at the same index, in the face's font units.
    pub units: Vec<Num>,
}

/// An image, as the image pass left it: a file passed through, decoded pixels, or the photo rule's JPEG.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct ImageSource {
    pub src: String,
    pub kind: ImageKind,
    pub tag: ImageTag,
    pub dom_path: String,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub alt: Option<String>,
    /// Key into the blobs: the image file as fetched, passed through as is (a JPEG, a PNG), or for a facsimile the PDF it is a page of. Absent where the pass decoded or re-encoded the file.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub blob: Option<String>,
    /// With a file `blob`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub mime: Option<String>,
    /// The pixels the image pass decoded, where no PDF filter reads the file (WebP, AVIF, GIF), or where no predictor describes a PNG's `IDAT` (version 6). Version 5.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub samples: Option<ImageSamples>,
    /// Key into the blobs: the JPEG the opaque-photo rule re-encoded the pixels as, which the writer carries under `/DCTDecode`. Version 5.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub jpeg: Option<String>,
    /// What the source was, where the list holds the pass's pixels or JPEG and not the file. Report only. Version 5.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub original: Option<Original>,
    /// The opaque-photo rule's evidence, wherever the pass consulted it. Version 5.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub photo: Option<PhotoEvidence>,
    /// Why the pass handed the file through when it asked the browser for its pixels, for the report. Version 5.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub refusal: Option<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub facsimile: Option<FacsimileRef>,
}

/// Where an image came from in the page.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum ImageKind {
    Img,
    Canvas,
    Background,
    Facsimile,
}

/// Whether an image is a figure's content or an artifact.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ImageTag {
    Figure,
    Artifact,
}

/// What the source was, where the list holds the pass's pixels or JPEG: its format as the report names it, its length, a PNG's `gAMA`-only flag, and why a PNG was decoded (version 6). Report only.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct Original {
    pub format: String,
    pub bytes: Num,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub gamma_only: Option<bool>,
    /// What in a decoded PNG `/Predictor 15` cannot describe: `alpha`, `interlace` or `palette-trns`. Version 6.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub why: Option<String>,
}

/// The page of a PDF a facsimile is.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct FacsimileRef {
    pub url: String,
    pub page: Num,
}

/// Decoded pixels, interleaved, row-major from the top: each pixel's colour components in `colorSpace`, then its alpha where `alpha` is set, `bitsPerComponent` bits each, big-endian at 16. The browser's are RGBA at eight bits in its sRGB (version 5); a PNG's are its own depth and colour space, a depth under 8 unpacked to one sample a byte, a palette's index with its `tRNS` alpha (version 6). The writer splits the colour from the alpha and writes the `/SMask` only where some pixel is not opaque.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct ImageSamples {
    /// Key into the blobs: the interleaved samples.
    pub blob: String,
    pub width: Num,
    pub height: Num,
    /// `DeviceRGB`, `DeviceGray`, `DeviceCMYK` (four components, never with `palette` or `icc`: the output intent is the profile, version 7) or `Indexed` (over `DeviceRGB`, with `palette`); a string, so the validator refuses another in its own sentence.
    pub color_space: String,
    /// 8 or 16; 16 never with `Indexed`.
    pub bits_per_component: Num,
    /// Each pixel carries an alpha sample after its colour. Version 6.
    pub alpha: bool,
    /// Key into the blobs: the palette as RGB triples, with `Indexed` only. Version 6.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub palette: Option<String>,
    /// Key into the blobs: the ICC profile the colour components are in, never with `Indexed`. Version 6.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub icc: Option<String>,
}

/// One quality the opaque-photo rule encoded and measured.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct PhotoCandidate {
    pub quality: Num,
    pub bytes: Num,
    pub psnr_db: Num,
    /// What the browser's own encoder chose at this quality, read back off the frame.
    pub subsampling: String,
}

/// What the opaque-photo rule saw and chose: the four gates, the sampled look, every quality it tried, and the one it chose or null. Report only; the writer's bytes follow from whether a `jpeg` is there. Version 5.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct PhotoEvidence {
    pub is_photo: bool,
    /// Why not, or `photographic` when it is.
    pub reason: String,
    pub pixels: Num,
    pub unique_ratio: Num,
    pub longest_run: Num,
    pub candidates: Vec<PhotoCandidate>,
    /// Null when none was chosen; never absent.
    pub chosen_quality: Option<Num>,
}

/// A raster fallback, as the print drew it: what PDF had no operator for.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct RasterSource {
    pub feature: String,
    pub reason: String,
    /// `[x y w h]` in page points.
    pub rect_pt: [Num; 4],
    pub dpi: Num,
    pub cm: Vec<Num>,
    /// Key into the blobs: the pixels as RGBA, row-major from the top. Absent where the print drew none.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub blob: Option<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub width: Option<Num>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub height: Option<Num>,
    /// The opaque-photo rule's evidence on the pixels, wherever the pass consulted it. Version 5.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub photo: Option<PhotoEvidence>,
    /// Key into the blobs: the JPEG the rule re-encoded the pixels as, which the writer carries instead of the RGBA. Version 5.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub jpeg: Option<String>,
}

/// A colour cluster, as the print drew it (dormant in `print()`, and refused by forme by name).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct EmojiSource {
    /// The cluster itself, which is also its `/ActualText`.
    pub text: Utf16,
    pub size_px: Num,
    /// Key into the blobs: the pixels as RGBA.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub blob: Option<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub width: Option<Num>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub height: Option<Num>,
}

/// A heading a bookmark points at.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct OutlineEntry {
    /// The heading's element, by its index in `structure.elements`. Version 4.
    pub element: Num,
    pub title: Utf16,
    pub level: Num,
    pub page: Num,
    /// The top of the heading's first line on its page, in points from the page's bottom, the `y` of the destination.
    pub top_pt: Num,
}

/// One run of `/PageLabels`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct PageLabelRun {
    /// The page the run opens on.
    pub page: Num,
    pub label: PageLabel,
}

/// A page label's style, prefix and start.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct PageLabel {
    pub style: String,
    pub prefix: String,
    pub start: Num,
}

/// One page.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Page {
    /// Width and height in points: the nominal size, which is the `MediaBox` before any marks.
    pub size: [Num; 2],
    /// `media` is `[0 0 w h]` and, where the page grew to hold its printer's marks, the grown box.
    pub boxes: Boxes,
    /// In painting order, the page's `link` items last. The order is data: the writer writes the items in it, and two items swapped are another file.
    pub items: Vec<Item>,
}

/// A page's boxes.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Boxes {
    pub media: BoxPt,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub trim: Option<BoxPt>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub bleed: Option<BoxPt>,
}

/// One item of a page, in painting order. Read by its `kind`, and where two items share a kind, by what tells them apart (`rect` for a rectangle, `table` for a raster, `start` or `end` for a tag), from the raw JSON, so a text's lone surrogate reaches [`Utf16`] unbuffered.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
#[allow(clippy::large_enum_variant, reason = "a list is read once and its items are never moved, so boxing would buy nothing")]
pub enum Item {
    Group(GroupItem),
    Text(TextItem),
    Rect(RectItem),
    Path(PathItem),
    Image(ImageItem),
    Raster(RasterItem),
    RasterFallback(RasterFallbackItem),
    Form(FormItem),
    Marks(MarksItem),
    Link(LinkItem),
    TagStart(TagStartItem),
    TagEnd(TagEndItem),
}

impl Item {
    /// The item's `kind`, as the list spells it.
    pub fn kind(&self) -> &'static str {
        match self {
            Item::Group(_) => "group",
            Item::Text(_) => "text",
            Item::Rect(_) | Item::Path(_) => "shape",
            Item::Image(_) => "image",
            Item::Raster(_) | Item::RasterFallback(_) => "raster",
            Item::Form(_) => "form",
            Item::Marks(_) => "marks",
            Item::Link(_) => "link",
            Item::TagStart(_) | Item::TagEnd(_) => "tag",
        }
    }
}

/// An item's `kind`, which picks the struct it is read as.
fn kind_of(v: &Value) -> Result<&str, String> {
    let Value::Object(_) = v else { return Err(invalid(v, "an object")) };
    v.member("kind").map_or(Err("missing field `kind`".into()), str_of)
}

/// Whether an item carries the member `key`, not null: what tells two items of one kind apart.
fn has(v: &Value, key: &str) -> bool {
    v.member(key).is_some_and(|m| *m != Value::Null)
}

impl Read for Item {
    fn read(v: &Value) -> Result<Self, String> {
        Ok(match kind_of(v)? {
            "group" => Item::Group(Read::read(v)?),
            "text" => Item::Text(Read::read(v)?),
            "shape" if has(v, "rect") => Item::Rect(Read::read(v)?),
            "shape" => Item::Path(Read::read(v)?),
            "image" => Item::Image(Read::read(v)?),
            "raster" if matches!(v.member("table"), Some(Value::Str(t)) if t == "emoji") => Item::Raster(Read::read(v)?),
            "raster" => Item::RasterFallback(Read::read(v)?),
            "form" => Item::Form(Read::read(v)?),
            "marks" => Item::Marks(Read::read(v)?),
            "link" => Item::Link(Read::read(v)?),
            "tag" if has(v, "start") => Item::TagStart(Read::read(v)?),
            "tag" => Item::TagEnd(Read::read(v)?),
            other => return Err(format!("an item of kind {other:?}, which the schema does not name")),
        })
    }
}

/// An untagged enum read as serde's derive read one: each variant tried in order, with serde's sentence when none matches.
macro_rules! untagged {
    ($name:ident { $($variant:ident($ty:ty)),+ }) => {
        impl Read for $name {
            fn read(v: &Value) -> Result<Self, String> {
                $(if let Ok(read) = <$ty>::read(v) {
                    return Ok($name::$variant(read));
                })+
                Err(concat!("data did not match any variant of untagged enum ", stringify!($name)).into())
            }
        }
    };
}

untagged!(Ink { Cmyk(CmykInk), Spot(SpotInk) });
untagged!(Paint { Rgb(RgbPaint), Cmyk(CmykInk), Gradient(GradientPaint) });
untagged!(StructChild { Element(Num), Content(ContentChild), Link(LinkChild) });
untagged!(TagStart { Content(ContentStart), Artifact(ArtifactStart) });

/// One vector item: what a collector measured, painted under a transform, clips, alpha and a mask.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
#[allow(clippy::large_enum_variant, reason = "as for Item")]
pub enum VectorItem {
    Path(PathItem),
    Group(GroupItem),
    Form(FormItem),
    RasterFallback(RasterFallbackItem),
}

impl Read for VectorItem {
    fn read(v: &Value) -> Result<Self, String> {
        Ok(match kind_of(v)? {
            "shape" => VectorItem::Path(Read::read(v)?),
            "group" => VectorItem::Group(Read::read(v)?),
            "form" => VectorItem::Form(Read::read(v)?),
            "raster" => VectorItem::RasterFallback(Read::read(v)?),
            other => return Err(format!("a vector item of kind {other:?}, which the schema does not name")),
        })
    }
}

/// A struct of a vector item's own fields followed by what every vector item carries whatever its kind (Caster's `Painting`): the transform, clips, alpha, blend and mask it is painted under. Names are the schema's; where the schema is silent they are krilla 0.8's `Surface`.
macro_rules! painted {
    ($(#[$meta:meta])* pub struct $name:ident { $($(#[$fmeta:meta])* pub $field:ident : $ty:ty,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize))]
        #[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
        pub struct $name {
            $($(#[$fmeta])* pub $field: $ty,)*
            /// The in-page CTM chain of every transformed HTML ancestor between the page element and this item, as the `cm` a content stream carries; absent when there is none. The writer concatenates it first, so the item's clips, paths and children are in the untransformed space they were measured in.
            #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
            pub transform: Option<Matrix>,
            /// Clips in force, already intersected by nesting: each is applied in turn inside the same graphics-state save, so the list is an intersection and cannot be one path.
            #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
            pub clips: Option<Vec<ClipPath>>,
            /// `/ca`; absent when 1.
            #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
            pub opacity: Option<Num>,
            /// `/CA`; absent when 1.
            #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
            pub stroke_opacity: Option<Num>,
            /// `/BM`, PDF's names (which are krilla's `BlendMode` variants); absent when `Normal`.
            #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
            pub blend: Option<String>,
            #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
            pub soft_mask: Option<SoftMask>,
            pub origin: VectorOrigin,
        }
    };
}

painted! {
    /// A path, filled and stroked: Typst's `Shape`. `strokeFirst` is SVG's `paint-order: stroke`: PDF fills first when one operator does both, so a path that wants the other order is written twice. Set only where fill and stroke are both there.
    pub struct PathItem {
        pub kind: KindShape,
        #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
        pub owner: Option<String>,
        pub path: Path,
        #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
        pub fill: Option<Paint>,
        pub rule: FillRule,
        #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
        pub stroke: Option<Stroke>,
        /// Only ever `true`.
        #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
        pub stroke_first: Option<bool>,
    }
}

painted! {
    /// Children painted together; a form XObject of their own when `isolated` (krilla's `push_isolated`).
    pub struct GroupItem {
        pub kind: KindGroup,
        #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
        pub owner: Option<String>,
        pub items: Vec<VectorItem>,
        pub bbox: BoxPt,
        /// Only ever `true`.
        #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
        pub isolated: Option<bool>,
    }
}

painted! {
    /// A reference to content written once and drawn many times: `form` names a `FormDefinition` in `resources.forms`, and `matrix` is the `cm` that places it.
    pub struct FormItem {
        pub kind: KindForm,
        #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
        pub owner: Option<String>,
        pub form: String,
        pub matrix: Matrix,
    }
}

painted! {
    /// A raster fallback, painted as an image where PDF has no vector that draws what the engine drew. Its pixels, rectangle, reason and placing `cm` are the resource's (`resources.rasters[source]`).
    pub struct RasterFallbackItem {
        pub kind: KindRaster,
        #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
        pub owner: Option<String>,
        pub table: TableRasters,
        pub source: Num,
    }
}

/// One run on one line, placed: Typst's `Text` item with Caster's additions. Lengths are points; a glyph's advance stays in the font's units, because the writer divides by `upem` itself and `(advance / upem) * 1000` is not always `advance * 1000 / upem`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct TextItem {
    pub kind: KindText,
    pub owner: String,
    /// Consecutive items with one `lineIndex` share a text object.
    pub line_index: Num,
    /// The `key` of an instance in `resources.fonts`.
    pub font: String,
    /// Points.
    pub size: Num,
    /// Points.
    pub letter_spacing: Num,
    pub fill: Fill,
    /// Points from the page's bottom.
    pub baseline: Num,
    /// The `cm` the line's text object is wrapped in, in page points.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub transform: Option<Matrix>,
    /// The clips the line's text object is painted under, outermost first, in the space inside `transform`; an intersection. Absent when nothing clips the line.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub clips: Option<Vec<ClipPath>>,
    /// The run's characters, in UTF-16 code units: the segments' texts, a space between two where DOM whitespace separated them, and a space first where `leadingSpace`.
    pub text: Utf16,
    /// The run owes the reader a space glyph before its first word: DOM whitespace stood across the boundary with the run before it, and this is not the first run of its line.
    pub leading_space: bool,
    /// The glyph id of U+0020 in the run's font under its features, which the writer shows for every space it writes.
    pub space: Num,
    pub glyphs: TextGlyphs,
    pub segments: TextSegments,
}

/// The run's glyphs as a struct of arrays, one entry per glyph, the segments' glyphs one after the other.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct TextGlyphs {
    pub ids: Vec<Num>,
    /// In font units, as the shaper positioned them.
    pub advances: Vec<Num>,
    /// The shaper's cluster value of each glyph, moved onto `text`: the offset, in UTF-16 code units, of the first character the glyph's cluster stands for. A cluster ends where the next distinct one begins, or where its segment does; in a right-to-left segment the values fall.
    pub clusters: Vec<Num>,
}

/// The run's measured segments (words), as a struct of arrays, one entry per segment.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct TextSegments {
    /// Index into `glyphs` of the segment's first glyph.
    pub first_glyph: Vec<Num>,
    /// Offset into `text`, in UTF-16 code units, of the segment's first character.
    pub start: Vec<Num>,
    /// Points from the page's left edge, where the browser put the segment: the one place the browser overrides the shaper, and the writer's pen jumps there.
    pub x: Vec<Num>,
    /// Whether DOM whitespace separated the segment from the one before it.
    pub space_before: Vec<bool>,
}

/// What a run or a rectangle is painted with, with the computed colour kept beside a declared ink because `colour: 'off'` paints a hinted run in its computed colour.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Fill {
    /// The computed colour as PDF `rg` operands, 0 to 1.
    pub rgb: [Num; 3],
    /// The colour the author declared, which wins when the colour story is on.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub ink: Option<Ink>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub overprint: Option<Overprint>,
}

/// A declared ink: process CMYK, or a spot colourant with its alternate and tint. The names `All` and `None` are the two colourants PDF predefines.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
pub enum Ink {
    Cmyk(CmykInk),
    Spot(SpotInk),
}

/// A process ink.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct CmykInk {
    pub cmyk: [Num; 4],
}

/// A spot ink.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct SpotInk {
    pub spot: String,
    pub alternate: [Num; 4],
    pub tint: Num,
}

/// Which painting overprints.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Overprint {
    Fill,
    Stroke,
    Both,
}

/// A rectangle: a measured paint, or the page's own ground. Typst's `Rect`, written as `re`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct RectItem {
    pub kind: KindShape,
    pub owner: String,
    /// `[x y w h]` in page points from the page's bottom.
    pub rect: [Num; 4],
    pub fill: Fill,
    /// What the colour report records of a measured paint. Absent on the page's own ground, which the colour emitter is not asked about, so this is also what tells the writer to ask.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub paint: Option<PaintOrigin>,
}

/// What the colour report records of a measured paint, and the colour emitter reads to decide.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct PaintOrigin {
    pub path: String,
    pub target: PaintTarget,
    pub page: Num,
    /// The `--pdf-color` hint verbatim.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub hint: Option<String>,
    /// The computed CSS colour the paint's `rgb` came from, or the empty string.
    pub color: String,
}

/// What a measured paint painted.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum PaintTarget {
    Background,
    Border,
    Fill,
}

/// A clip: a path and its rule.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ClipPath {
    pub path: Path,
    pub rule: FillRule,
}

/// The non-zero or the even-odd rule.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum FillRule {
    Nonzero,
    Evenodd,
}

/// A luminosity or alpha soft mask, as a transparency group of its own.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct SoftMask {
    #[cfg_attr(feature = "serde", serde(rename = "type"))]
    pub mask_type: MaskType,
    pub items: Vec<VectorItem>,
    pub bbox: BoxPt,
    /// `/BC`, the backdrop the group is composited against.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub backdrop: Option<Vec<Num>>,
}

/// A soft mask's type.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum MaskType {
    Luminosity,
    Alpha,
}

/// What a vector fill or stroke paints with.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
pub enum Paint {
    Rgb(RgbPaint),
    Cmyk(CmykInk),
    Gradient(GradientPaint),
}

/// A computed colour.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct RgbPaint {
    pub rgb: [Num; 3],
}

/// A gradient paint.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct GradientPaint {
    pub gradient: Gradient,
}

/// A gradient, in the terms the PDF dictionary uses (Caster's `ShadingSpec` with its type named). `coordsPt` and `source` are for the report.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct Gradient {
    #[cfg_attr(feature = "serde", serde(rename = "type"))]
    pub gradient_type: GradientType,
    /// `/Coords`, in the space `matrix` maps onto the page.
    pub coords: Vec<Num>,
    pub extend: [bool; 2],
    pub stops: Vec<GradientStop>,
    pub color_space: ShadingSpace,
    /// The `cm` concatenated before `sh`, or folded into a pattern's `/Matrix`.
    pub matrix: Matrix,
    /// The same coordinates mapped onto the page, for a human reading the report.
    pub coords_pt: Vec<Num>,
    /// `pad`, or the spread the stops were unrolled for.
    pub spread: Spread,
    /// How many periods the unrolling covers; 1 when the spread is `pad`.
    pub periods: Num,
    /// True when any stop is not fully opaque, which is what asks for a soft mask.
    pub has_alpha: bool,
    /// What the gradient was written as, for the record.
    pub source: String,
    /// Set when PDF has no shading that draws what the engine drew, with the reason.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub degenerate: Option<String>,
}

/// Axial or radial.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum GradientType {
    Axial,
    Radial,
}

/// A shading's colour space.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ShadingSpace {
    #[cfg_attr(feature = "serde", serde(rename = "DeviceRGB"))]
    DeviceRgb,
    #[cfg_attr(feature = "serde", serde(rename = "DeviceCMYK"))]
    DeviceCmyk,
}

/// A gradient's spread.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Spread {
    Pad,
    Reflect,
    Repeat,
}

/// One stop of a gradient, with the alpha kept beside the colour rather than folded in.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct GradientStop {
    pub offset: Num,
    pub rgb: [Num; 3],
    /// Present when the hint declared the stop in CMYK.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub cmyk: Option<[Num; 4]>,
    pub alpha: Num,
}

/// A stroke.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct Stroke {
    pub paint: Paint,
    /// Points.
    pub width: Num,
    /// `J`'s numbering: 0 butt, 1 round, 2 projecting square.
    pub cap: Num,
    /// `j`'s numbering: 0 miter, 1 round, 2 bevel.
    pub join: Num,
    pub miter_limit: Num,
    /// Points.
    pub dash: Vec<Num>,
    /// Points.
    pub dash_phase: Num,
}

/// Where a vector item came from, for the report. Every nested item has `domPath` and `label`; the top-level item also has the page and the paint index, and the reasons the report gives for what the item asked for.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct VectorOrigin {
    /// DOM path of the element the item came from.
    pub dom_path: String,
    /// A short name for the report, e.g. `background`, `border-top`, `circle`.
    pub label: String,
    /// Top-level item only.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub page: Option<Num>,
    /// Top-level item only.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub paint_index: Option<Num>,
    /// What asked for each clip, parallel to `clips`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub clip_reasons: Option<Vec<String>>,
    /// What asked for the soft mask.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub mask_reason: Option<String>,
    /// A `group`: why it is a form of its own.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub isolated_reason: Option<String>,
    /// A `form`: why it is one.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub reason: Option<String>,
    /// A `shape` with a stroke: `vector-effect: non-scaling-stroke`, whose points were pre-transformed.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub non_scaling: Option<bool>,
}

/// A placed image: the unit square onto the page, clipped, drawn.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ImageItem {
    pub kind: KindImage,
    pub owner: String,
    /// Key into `resources.images`.
    pub source: Num,
    /// The `cm` that puts the unit square on the painted rectangle, in page points.
    pub transform: Matrix,
    /// The path the image is clipped to, in page points, non-zero.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub clip: Option<Path>,
    pub interpolate: bool,
}

/// A colour-emoji cluster drawn as pixels, whose words are its `actualText` (dormant in `print()`, and refused by forme by name).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct RasterItem {
    pub kind: KindRaster,
    pub owner: String,
    pub table: TableEmoji,
    /// Key into `resources.emoji`.
    pub source: Num,
    /// The cluster's text.
    pub actual_text: Utf16,
    /// The `cm` that puts the unit square on the cluster's box.
    pub transform: Matrix,
    /// The `cm` of the line's chain, when the cluster's line carries one.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub chain: Option<Matrix>,
    pub emoji_origin: EmojiOrigin,
}

/// What the report records of a colour cluster. Untested: no print reaches it.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct EmojiOrigin {
    /// The font the canvas ended up with.
    pub face: String,
    pub rect_left_pt: Num,
    pub rect_width_pt: Num,
    pub ascent_px: Num,
    pub descent_px: Num,
    /// The box `[x y w h]`.
    pub rect_pt: [Num; 4],
}

/// The page's printer's marks, stroked in the one colourant that reaches every plate. They belong to no element; the media box they grew is the page's `boxes.media`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct MarksItem {
    pub kind: KindMarks,
    pub shapes: Vec<MarkShape>,
}

/// One stroked shape of one mark, as a list of subpaths in page points.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct MarkShape {
    pub kind: MarkKind,
    /// Which corner or side the shape belongs to.
    pub at: String,
    /// Straight segments, `[x0, y0, x1, y1]`.
    pub segments: Vec<[Num; 4]>,
    /// A circle, `[cx, cy, r]`, present on a registration target.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub circle: Option<[Num; 3]>,
}

/// A crop mark or a registration cross.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum MarkKind {
    Crop,
    Cross,
}

/// One link annotation on one page, and the `Link` element whose `/OBJR` points at it. Not painted: the writer collects the page's link items after its content.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct LinkItem {
    pub kind: KindLink,
    /// The `Link` element that owns the annotation, by its index in `structure.elements`; its `children` end with this link's `/OBJR`. Version 4.
    pub element: Num,
    /// The annotation's place among all the document's links, 0, 1, 2 … over the document: its `/StructParent` is the page count plus this.
    pub order: Num,
    /// `[left, top, right, bottom]` in page points, y up (so `top` is the larger).
    pub rects: Vec<[Num; 4]>,
    pub target: LinkTarget,
    /// The link's text, for the annotation's `/Contents`.
    pub contents: Utf16,
}

/// Where a link goes.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", deny_unknown_fields))]
pub struct LinkTarget {
    pub href: String,
    /// An `href` beginning `#` is a place in this file and becomes a `/Dest`; anything else is an address and becomes a `/URI`.
    pub kind: LinkKind,
    /// Internal links only: the id the fragment named.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub target_id: Option<String>,
    /// Internal links only: where the target is in points, and the element the `/Dest` names, by its index in `structure.elements`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub dest: Option<Dest>,
    /// Every link without a `dest`: the `/URI` its action carries, an absolute, ASCII, percent-encoded URI. Version 3.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub uri: Option<String>,
}

/// What kind of address a link's `href` is.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum LinkKind {
    Internal,
    External,
    Mailto,
}

/// An internal link's destination. `struct` is the path of the element the fragment named, which only the report reads.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Dest {
    pub page: Num,
    pub x: Num,
    pub y: Num,
    pub element: Num,
    #[cfg_attr(feature = "serde", serde(rename = "struct"))]
    pub struct_path: String,
}

/// The structure tree as the writer writes it: every element the file carries, pruned, ordered and resolved by the builder. Element 0 is the root (`Document`). Version 4.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Structure {
    /// The order is data: the order the writer allocates the elements' objects in, which every index into the array names.
    pub elements: Vec<StructElement>,
}

/// One structure element, as its dictionary is written.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct StructElement {
    /// The PDF 2.0 standard-namespace structure type, `/S`.
    #[cfg_attr(feature = "serde", serde(rename = "type"))]
    pub element_type: String,
    /// The walk's structure path, e.g. `Document/Sect[0]/P[2]`. Only the report reads it.
    pub path: String,
    /// `/Lang`, a byte string. Each of the four entries is present only when it is written.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub lang: Option<String>,
    /// `/Alt`, a text string.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub alt: Option<Utf16>,
    /// `/E`, the expansion of an abbreviation, a text string.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub expansion: Option<Utf16>,
    /// `/ID`, a byte string, which `/Headers` and the IDTree name the element by.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub id: Option<String>,
    /// `/A`: the attribute objects in the order they are written; one is written as the dictionary itself, more than one as an array. Absent when there is none.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub attributes: Option<Vec<Attribute>>,
    /// `/K` in reading order. The order is data: the writer writes `/K` in it.
    pub children: Vec<StructChild>,
}

/// One attribute object of `/A`, its keys as the list wrote them, in the order it wrote them: the validator refuses a key the schema does not name, with the sentence that names the first such key, and the writer writes `/O` first and then the schema's key order. Every string value is a PDF name except each entry of `Headers`, a byte string. `BBox` is a PDF rectangle in page points, to thousandths. A `Table` object is a cell's: `Scope` on a `TH`, `Headers` where its header set is not empty, `RowSpan` and `ColSpan` only above 1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attribute(pub Vec<(String, Value)>);

impl Attribute {
    /// The value under `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

impl Read for Attribute {
    fn read(v: &Value) -> Result<Self, String> {
        match v {
            Value::Object(_) if v.has_lone() => Err("a lone surrogate in a string that is not a text".into()),
            Value::Object(members) => Ok(Attribute(members.clone())),
            other => Err(invalid(other, "an attribute object")),
        }
    }
}

#[cfg(feature = "serde")]
impl Serialize for Attribute {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, &Json(value))?;
        }
        map.end()
    }
}

/// A reader's value written back as serde_json wrote it, for the tests' round trip.
#[cfg(feature = "serde")]
struct Json<'a>(&'a Value);

#[cfg(feature = "serde")]
impl Serialize for Json<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self.0 {
            Value::Null => s.serialize_unit(),
            Value::Bool(b) => s.serialize_bool(*b),
            Value::Num(n) => Num(*n).serialize(s),
            Value::Str(text) => s.serialize_str(text),
            Value::Lone(units) => Utf16(units.clone()).serialize(s),
            Value::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(&Json(item))?;
                }
                seq.end()
            }
            Value::Object(members) => {
                let mut map = s.serialize_map(Some(members.len()))?;
                for (key, value) in members {
                    map.serialize_entry(key, &Json(value))?;
                }
                map.end()
            }
        }
    }
}

/// One entry of an element's `/K`: another element by its index in `elements`; a marked-content sequence by the page and the index in that page's `items` of the `tag` item that opens it; or the annotation of the `link` item whose `order` this is, as an `/OBJR`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
pub enum StructChild {
    Element(Num),
    Content(ContentChild),
    Link(LinkChild),
}

/// A marked-content sequence a structure element owns.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ContentChild {
    pub page: Num,
    pub item: Num,
}

/// A link annotation a structure element owns, by the link's `order`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct LinkChild {
    pub link: Num,
}

/// `start` opens one marked-content sequence; it pairs with an [`TagEndItem`] in one item list.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct TagStartItem {
    pub kind: KindTag,
    pub start: TagStart,
}

/// `end` closes the marked-content sequence open in the item list.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct TagEndItem {
    pub kind: KindTag,
    /// Only ever `true`.
    pub end: bool,
}

/// What a marked-content sequence is: an element's content, or an artifact.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
pub enum TagStart {
    Content(ContentStart),
    Artifact(ArtifactStart),
}

/// A content start.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ContentStart {
    /// The structure element that owns the sequence, by its index in `structure.elements`, whose `children` name this item; the sequence is tagged with the element's `type`. Its MCID is the writer's: the content starts of a page are numbered 0, 1, 2 … in item order. Version 4.
    pub element: Num,
}

/// An artifact start.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ArtifactStart {
    pub artifact: Artifact,
    /// The artifact's owner key; absent on the page's own ground, which the report does not list.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub path: Option<String>,
    /// What the group's first unit was, for the report.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub kind: Option<ArtifactKind>,
}

/// An artifact's type and subtype.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Artifact {
    #[cfg_attr(feature = "serde", serde(rename = "type"))]
    pub artifact_type: ArtifactType,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub subtype: Option<String>,
}

/// An artifact's `/Type`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ArtifactType {
    Pagination,
    Layout,
    Page,
}

/// What an artifact group's first unit was.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum ArtifactKind {
    Paint,
    Image,
    Text,
}

/// One segment of a path, in PDF points. `c` carries two control points.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(tag = "op"))]
pub enum PathSegment {
    #[cfg_attr(feature = "serde", serde(rename = "m"))]
    Move { to: [Num; 2] },
    #[cfg_attr(feature = "serde", serde(rename = "l"))]
    Line { to: [Num; 2] },
    #[cfg_attr(feature = "serde", serde(rename = "c"))]
    Curve { c1: [Num; 2], c2: [Num; 2], to: [Num; 2] },
    #[cfg_attr(feature = "serde", serde(rename = "h"))]
    Close,
}

/// A path segment read by its `op`, with the sentences serde's derive gave: a missing `op` or point, an `op` the schema does not name, a point the `op` does not take.
impl Read for PathSegment {
    fn read(v: &Value) -> Result<Self, String> {
        let m = object(v, &["op", "to", "c1", "c2"])?;
        let op: String = m.get::<Option<String>>("op")?.ok_or("missing field `op`")?;
        let only = |allowed: &[&str]| -> Result<(), String> {
            match ["to", "c1", "c2"].into_iter().find(|k| has(v, k) && !allowed.contains(k)) {
                Some(stray) => Err(format!("unknown field `{stray}`")),
                None => Ok(()),
            }
        };
        Ok(match op.as_str() {
            "m" | "l" => {
                only(&["to"])?;
                let to = m.get("to")?;
                if op == "m" { PathSegment::Move { to } } else { PathSegment::Line { to } }
            }
            "c" => PathSegment::Curve { c1: m.get("c1")?, c2: m.get("c2")?, to: m.get("to")? },
            "h" => {
                only(&[])?;
                PathSegment::Close
            }
            other => return Err(format!("unknown variant `{other}`, expected one of `m`, `l`, `c`, `h`")),
        })
    }
}

// The readers: every struct with each field's key as the list spells it, every enum of unit variants with its strings. The key lists are checked against the schema's names by the corpus round trip (`tests/corpus.rs`), which writes each list back through serde's `Serialize` under the tests' `serde` feature.

read_struct!(DisplayList { version: "version", info: "info", writing: "writing", resources: "resources", structure: "structure", outline: "outline", page_labels: "pageLabels", pages: "pages" });
read_struct!(DocumentInfo { lang: "lang", title: "title", conformance: "conformance", page_count: "pageCount", created: "created" });
read_enum!(Conformance { Pdfua2: "pdfua2", Tagged: "tagged" });
read_struct!(Writing { font_profile: "fontProfile", colour: "colour" });
read_struct!(ColourWriting { profiles: "profiles", condition: "condition", info: "info", black_text: "blackText" });
read_struct!(Profiles { cmyk: "cmyk", srgb: "srgb" });
read_enum!(BlackTextPolicy { None: "none", HundredK: "100k" });
read_struct!(FontProfile { subset: "subset", width_decimals: "widthDecimals", cff_container: "cffContainer", dw_from_mode: "dwFromMode" });
read_enum!(CffContainer { Bare: "bare", Opentype: "opentype" });
read_struct!(Resources { fonts: "fonts", images: "images", forms: "forms", rasters: "rasters", emoji: "emoji" });
read_struct!(FormDefinition { bbox: "bbox", items: "items" });
read_struct!(FontInstance { key: "key", blob: "blob", url: "url", face_id: "faceId", family: "family", source_format: "sourceFormat", coords: "coords", upem: "upem", widths: "widths" });
read_enum!(SourceFormat { Ttf: "ttf", Otf: "otf", Woff: "woff", Woff2: "woff2" });
read_struct!(FontWidths { gids: "gids", units: "units" });
read_struct!(ImageSource { src: "src", kind: "kind", tag: "tag", dom_path: "domPath", alt: "alt", blob: "blob", mime: "mime", samples: "samples", jpeg: "jpeg", original: "original", photo: "photo", refusal: "refusal", facsimile: "facsimile" });
read_enum!(ImageKind { Img: "img", Canvas: "canvas", Background: "background", Facsimile: "facsimile" });
read_enum!(ImageTag { Figure: "Figure", Artifact: "Artifact" });
read_struct!(Original { format: "format", bytes: "bytes", gamma_only: "gammaOnly", why: "why" });
read_struct!(FacsimileRef { url: "url", page: "page" });
read_struct!(ImageSamples { blob: "blob", width: "width", height: "height", color_space: "colorSpace", bits_per_component: "bitsPerComponent", alpha: "alpha", palette: "palette", icc: "icc" });
read_struct!(PhotoCandidate { quality: "quality", bytes: "bytes", psnr_db: "psnrDb", subsampling: "subsampling" });
read_struct!(PhotoEvidence { is_photo: "isPhoto", reason: "reason", pixels: "pixels", unique_ratio: "uniqueRatio", longest_run: "longestRun", candidates: "candidates", chosen_quality: "chosenQuality" });
read_struct!(RasterSource { feature: "feature", reason: "reason", rect_pt: "rectPt", dpi: "dpi", cm: "cm", blob: "blob", width: "width", height: "height", photo: "photo", jpeg: "jpeg" });
read_struct!(EmojiSource { text: "text", size_px: "sizePx", blob: "blob", width: "width", height: "height" });
read_struct!(OutlineEntry { element: "element", title: "title", level: "level", page: "page", top_pt: "topPt" });
read_struct!(PageLabelRun { page: "page", label: "label" });
read_struct!(PageLabel { style: "style", prefix: "prefix", start: "start" });
read_struct!(Page { size: "size", boxes: "boxes", items: "items" });
read_struct!(Boxes { media: "media", trim: "trim", bleed: "bleed" });
read_struct!(PathItem { kind: "kind", owner: "owner", path: "path", fill: "fill", rule: "rule", stroke: "stroke", stroke_first: "strokeFirst", transform: "transform", clips: "clips", opacity: "opacity", stroke_opacity: "strokeOpacity", blend: "blend", soft_mask: "softMask", origin: "origin" });
read_struct!(GroupItem { kind: "kind", owner: "owner", items: "items", bbox: "bbox", isolated: "isolated", transform: "transform", clips: "clips", opacity: "opacity", stroke_opacity: "strokeOpacity", blend: "blend", soft_mask: "softMask", origin: "origin" });
read_struct!(FormItem { kind: "kind", owner: "owner", form: "form", matrix: "matrix", transform: "transform", clips: "clips", opacity: "opacity", stroke_opacity: "strokeOpacity", blend: "blend", soft_mask: "softMask", origin: "origin" });
read_struct!(RasterFallbackItem { kind: "kind", owner: "owner", table: "table", source: "source", transform: "transform", clips: "clips", opacity: "opacity", stroke_opacity: "strokeOpacity", blend: "blend", soft_mask: "softMask", origin: "origin" });
read_struct!(TextItem { kind: "kind", owner: "owner", line_index: "lineIndex", font: "font", size: "size", letter_spacing: "letterSpacing", fill: "fill", baseline: "baseline", transform: "transform", clips: "clips", text: "text", leading_space: "leadingSpace", space: "space", glyphs: "glyphs", segments: "segments" });
read_struct!(TextGlyphs { ids: "ids", advances: "advances", clusters: "clusters" });
read_struct!(TextSegments { first_glyph: "firstGlyph", start: "start", x: "x", space_before: "spaceBefore" });
read_struct!(Fill { rgb: "rgb", ink: "ink", overprint: "overprint" });
read_struct!(CmykInk { cmyk: "cmyk" });
read_struct!(SpotInk { spot: "spot", alternate: "alternate", tint: "tint" });
read_enum!(Overprint { Fill: "fill", Stroke: "stroke", Both: "both" });
read_struct!(RectItem { kind: "kind", owner: "owner", rect: "rect", fill: "fill", paint: "paint" });
read_struct!(PaintOrigin { path: "path", target: "target", page: "page", hint: "hint", color: "color" });
read_enum!(PaintTarget { Background: "background", Border: "border", Fill: "fill" });
read_struct!(ClipPath { path: "path", rule: "rule" });
read_enum!(FillRule { Nonzero: "nonzero", Evenodd: "evenodd" });
read_struct!(SoftMask { mask_type: "type", items: "items", bbox: "bbox", backdrop: "backdrop" });
read_enum!(MaskType { Luminosity: "luminosity", Alpha: "alpha" });
read_struct!(RgbPaint { rgb: "rgb" });
read_struct!(GradientPaint { gradient: "gradient" });
read_struct!(Gradient { gradient_type: "type", coords: "coords", extend: "extend", stops: "stops", color_space: "colorSpace", matrix: "matrix", coords_pt: "coordsPt", spread: "spread", periods: "periods", has_alpha: "hasAlpha", source: "source", degenerate: "degenerate" });
read_enum!(GradientType { Axial: "axial", Radial: "radial" });
read_enum!(ShadingSpace { DeviceRgb: "DeviceRGB", DeviceCmyk: "DeviceCMYK" });
read_enum!(Spread { Pad: "pad", Reflect: "reflect", Repeat: "repeat" });
read_struct!(GradientStop { offset: "offset", rgb: "rgb", cmyk: "cmyk", alpha: "alpha" });
read_struct!(Stroke { paint: "paint", width: "width", cap: "cap", join: "join", miter_limit: "miterLimit", dash: "dash", dash_phase: "dashPhase" });
read_struct!(VectorOrigin { dom_path: "domPath", label: "label", page: "page", paint_index: "paintIndex", clip_reasons: "clipReasons", mask_reason: "maskReason", isolated_reason: "isolatedReason", reason: "reason", non_scaling: "nonScaling" });
read_struct!(ImageItem { kind: "kind", owner: "owner", source: "source", transform: "transform", clip: "clip", interpolate: "interpolate" });
read_struct!(RasterItem { kind: "kind", owner: "owner", table: "table", source: "source", actual_text: "actualText", transform: "transform", chain: "chain", emoji_origin: "emojiOrigin" });
read_struct!(EmojiOrigin { face: "face", rect_left_pt: "rectLeftPt", rect_width_pt: "rectWidthPt", ascent_px: "ascentPx", descent_px: "descentPx", rect_pt: "rectPt" });
read_struct!(MarksItem { kind: "kind", shapes: "shapes" });
read_struct!(MarkShape { kind: "kind", at: "at", segments: "segments", circle: "circle" });
read_enum!(MarkKind { Crop: "crop", Cross: "cross" });
read_struct!(LinkItem { kind: "kind", element: "element", order: "order", rects: "rects", target: "target", contents: "contents" });
read_struct!(LinkTarget { href: "href", kind: "kind", target_id: "targetId", dest: "dest", uri: "uri" });
read_enum!(LinkKind { Internal: "internal", External: "external", Mailto: "mailto" });
read_struct!(Dest { page: "page", x: "x", y: "y", element: "element", struct_path: "struct" });
read_struct!(Structure { elements: "elements" });
read_struct!(StructElement { element_type: "type", path: "path", lang: "lang", alt: "alt", expansion: "expansion", id: "id", attributes: "attributes", children: "children" });
read_struct!(ContentChild { page: "page", item: "item" });
read_struct!(LinkChild { link: "link" });
read_struct!(TagStartItem { kind: "kind", start: "start" });
read_struct!(TagEndItem { kind: "kind", end: "end" });
read_struct!(ContentStart { element: "element" });
read_struct!(ArtifactStart { artifact: "artifact", path: "path", kind: "kind" });
read_struct!(Artifact { artifact_type: "type", subtype: "subtype" });
read_enum!(ArtifactType { Pagination: "Pagination", Layout: "Layout", Page: "Page" });
read_enum!(ArtifactKind { Paint: "paint", Image: "image", Text: "text" });

#[cfg(test)]
mod tests {
    use super::*;

    fn read<T: Read>(json: &str) -> Result<T, String> {
        T::read(&crate::read::parse(json)?)
    }

    #[test]
    fn a_lone_surrogate_is_read_into_a_text_and_refused_elsewhere() {
        assert_eq!(read::<Utf16>(r#""a\ud800bé\n""#).unwrap().0, vec![0x61, 0xD800, 0x62, 0xE9, 0x0A]);
        assert_eq!(read::<Utf16>(r#""a😀""#).unwrap().0, vec![0x61, 0xD83D, 0xDE00]);
        assert_eq!(json_string(&[0x61, 0xD800, 0x62]), r#""a\ud800b""#);
        assert_eq!(read::<String>(r#""a\ud800b""#).unwrap_err(), "a lone surrogate in a string that is not a text");
    }

    #[test]
    fn a_text_item_with_a_lone_surrogate_is_read_where_it_sits() {
        let json = r#"{"kind":"text","owner":"P","lineIndex":0,"font":"F","size":12,"letterSpacing":0,"fill":{"rgb":[0,0,0]},"baseline":10,"text":"x\udc00y","leadingSpace":false,"space":1,"glyphs":{"ids":[3],"advances":[500],"clusters":[0]},"segments":{"firstGlyph":[0],"start":[0],"x":[1.5],"spaceBefore":[false]}}"#;
        match read::<Item>(json).unwrap() {
            Item::Text(text) => assert_eq!(text.text.0, vec![0x78, 0xDC00, 0x79]),
            other => panic!("read as {}", other.kind()),
        }
        let renamed = json.replace("\"baseline\"", "\"baseLine\"");
        assert_eq!(read::<Item>(&renamed).unwrap_err(), "unknown field `baseLine`");
        assert_eq!(read::<Item>(&json.replace(r#""advances":[500],"#, "")).unwrap_err(), "/glyphs: missing field `advances`");
        assert_eq!(read::<Item>(&json.replace("[1.5]", r#"["1.5"]"#)).unwrap_err(), "/segments/x/0: invalid type: a string, expected a number");
    }

    #[test]
    fn the_hand_readers_read_and_refuse_as_the_derives_did() {
        let path = r#"[{"op":"m","to":[0,1]},{"op":"l","to":[2,3]},{"op":"c","c1":[1,1],"c2":[2,2],"to":[3,3]},{"op":"h"}]"#;
        let segments: Path = read(path).unwrap();
        assert_eq!(segments[2], PathSegment::Curve { c1: [Num(1.0); 2], c2: [Num(2.0); 2], to: [Num(3.0); 2] });
        assert_eq!(segments.len(), 4);
        for (bad, says) in [(r#"{"to":[0,0]}"#, "missing field `op`"), (r#"{"op":"z"}"#, "unknown variant `z`"), (r#"{"op":"h","to":[0,0]}"#, "unknown field `to`"), (r#"{"op":"c","to":[0,0]}"#, "missing field `c1`"), (r#"{"op":"m","to":[0,0],"x":1}"#, "unknown field `x`"), (r#"{"op":"m","to":[0]}"#, "invalid length 1")] {
            let error = read::<PathSegment>(bad).unwrap_err();
            assert!(error.contains(says), "{bad}: {error}");
        }
        assert_eq!(read::<Paint>(r#"{"cmyk":[0,0,0,1]}"#).unwrap(), Paint::Cmyk(CmykInk { cmyk: [Num(0.0), Num(0.0), Num(0.0), Num(1.0)] }));
        assert_eq!(read::<StructChild>("7").unwrap(), StructChild::Element(Num(7.0)));
        assert!(read::<Ink>(r#"{"rgb":[0,0,0]}"#).unwrap_err().starts_with("data did not match any variant of untagged enum Ink"));
        assert_eq!(read::<Table<Num>>(r#"{"0":1,"12":2}"#).unwrap().keys().copied().collect::<Vec<_>>(), vec![0, 12]);
        assert!(read::<Table<Num>>(r#"{"01":1}"#).is_err() && read::<Table<Num>>(r#"{"-1":1}"#).is_err());
        assert_eq!(read::<CmykInk>(r#"{"cmyk":[0,0,0,1],"cmyk":[0,0,0,1]}"#).unwrap_err(), "duplicate field `cmyk`");
        assert_eq!(read::<Conformance>(r#""pdfua1""#).unwrap_err(), "unknown variant `pdfua1`");
        assert_eq!(read::<KindText>(r#""shape""#).unwrap_err(), r#"expected "text", found "shape""#);
    }
}
