//! forme: a PDF 2.0 writer that reads Caster's display list and nothing else, held to the files Caster's TypeScript writer prints from the same lists (*the same list prints the same bytes*, Caster's DESIGN.md S102).
//!
//! The whole API is [`write`]: a list and its blobs in, the file out, or a [`Refusal`] in a sentence. The list's schema is [`list`], its refusals before a byte are [`validate`], the content-stream writer is [`content`], the fonts are [`fonts`], the image XObjects are [`images`], the facsimiles' Form XObjects are [`facsimile`], the colour story is [`colour`], the vector items are [`vectors`], the page's marked content, text, images, rectangles and print marks are [`text`], and the structure tree and what points into it are [`tree`]. The writer writes the document's scaffolding (the header, the catalog with `/Version /2.0`, `/Lang`, `/DisplayDocTitle`, `/StructTreeRoot`, `/MarkInfo`, `/Outlines` and `/PageLabels`, every page with its boxes, `/Tabs /S`, `/StructParents` and its link annotations, every font instance as `F0`, `F1`, … in its resources and its content stream, the Info dictionary, the XMP packet and the trailer `/ID`), every font instance's objects, every image XObject the list's images and raster fallbacks make and every facsimile's Form XObject (each page naming those it paints), each page's marked content, text, images, rectangles, print marks and vector items (paths, shadings, patterns, graphics states, groups, forms and raster fallbacks), the colour story (the output intent, `/DefaultRGB`, the separations and the overprint states), and the tree; it returns the `mcids`, `pens`, `kernAdjustments`, `actualTextSpans`, `faces`, `images`, `xobjects`, `paint`, `vectorResources` and `colour` records beside the bytes. It writes every item kind of the list but the dormant colour cluster, which it refuses by name.

pub mod colour;
pub mod content;
pub mod facsimile;
pub mod fonts;
pub mod images;
pub mod list;
pub mod text;
pub mod tree;
pub mod validate;
pub mod vectors;

use std::collections::BTreeMap;
use std::fmt;

use pdf_writer::{Finish, Name, Pdf, Rect, Ref, Str, TextStr};
use sha2::{Digest, Sha256};

use list::{Blobs, Conformance, DisplayList, Num, Utf16};

/// What the file names as its producer and creator: the printer, as the oracle names it, so the Info dictionary and the packet say what they always said.
pub const PRODUCER: &str = "BelType Caster";

/// A list the writer will not write, in a sentence; one that concerns an item carries the page and the item.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    pub message: String,
    /// `[page, item]`, both counted from 0, as Caster's `listItem` is.
    pub list_item: Option<[f64; 2]>,
}

impl Refusal {
    /// A refusal that concerns the list as a whole.
    pub fn new(message: String) -> Self {
        Self { message, list_item: None }
    }

    /// A refusal that concerns item `item` of page `page`.
    pub fn at(message: String, page: usize, item: usize) -> Self {
        Self { message, list_item: Some([page as f64, item as f64]) }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refusal {}

/// The file a list makes, and the records of what the writer chose, as Caster's `WriterRecords` (`src/displayList/contract.ts`) names them.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub bytes: Vec<u8>,
    pub records: Records,
}

/// The writer's records, serialised as Caster's `WriterRecords` JSON; the members this stage does not make yet are left out.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Records {
    /// Per page, the index of the content start each MCID opens, by MCID: 0, 1, 2 … in item order.
    pub mcids: Vec<Vec<usize>>,
    /// One entry per text item, in painting order.
    pub pens: Vec<text::Pen>,
    /// The glyphs whose shaped advance was written as its own adjustment.
    pub kern_adjustments: usize,
    /// The glyph occurrences wrapped in `/ActualText`.
    pub actual_text_spans: usize,
    /// What each font instance became, one per instance in the order of `resources.fonts`.
    pub faces: Vec<fonts::FaceRecord>,
    /// One entry per image item painted, in painting order.
    pub images: Vec<images::ImageUse>,
    /// How many distinct image XObjects the file holds, the raster fallbacks' included, by the dedupe rule.
    pub xobjects: usize,
    /// One row per vector node painted, in painting order: Caster's `ReportPaint`, as JSON text.
    #[serde(serialize_with = "raw_json")]
    pub paint: String,
    /// How many shadings, patterns, graphics states and transparency-group forms the vectors made, as JSON text.
    #[serde(serialize_with = "raw_json")]
    pub vector_resources: String,
    /// The colour record, when the list has a colour story.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colour: Option<colour::Record>,
}

/// A record member held as JSON text, written into the records as it is.
fn raw_json<S: serde::Serializer>(text: &str, s: S) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(&serde_json::value::RawValue::from_string(text.to_string()).map_err(serde::ser::Error::custom)?, s)
}

/// The file a list makes.
///
/// # Errors
/// Every refusal of [`validate::validate`], first; then a blob whose bytes are not its name, an `info.created` that is not an instant in UTC, every refusal of [`fonts::write_fonts`], of [`images::write`] and of [`colour::Colour`], and a colour-cluster item, by name.
pub fn write(list: &DisplayList, blobs: &Blobs) -> Result<Written, Refusal> {
    validate::validate(list, blobs)?;
    for (name, bytes) in blobs {
        let hash = hex(&Sha256::digest(bytes));
        if &hash != name {
            return Err(Refusal::new(format!("forme: the blob {name} holds bytes whose SHA-256 is {hash}; a blob is named by its bytes")));
        }
    }
    let date = pdf_date(&list.info.created)?;

    let mut pdf = Pdf::new();
    pdf.set_version(2, 0);
    let mut next = Ref::new(1);
    let catalog_id = next.bump();
    let pages_id = next.bump();
    let page_ids: Vec<(Ref, Ref)> = list.pages.iter().map(|_| (next.bump(), next.bump())).collect();
    let info_id = next.bump();
    let xmp_id = next.bump();
    let (embedded, faces) = fonts::write_fonts(&mut pdf, &mut next, list, blobs)?;
    let mut colour = colour::Colour::new(list, blobs)?;
    let images = images::write(&mut pdf, &mut next, list, blobs)?;
    let font_names = text::font_names(&embedded);
    let mut vectors = vectors::Vectors::new(&mut pdf, &mut next, list, &images);
    let (contents, counts) = text::pages(list, &embedded, &images, colour.as_mut(), &mut vectors)?;
    let (paint, vector_resources) = (format!("[{}]", vectors.paint.join(",")), vectors.counts());

    let page_refs: Vec<Ref> = page_ids.iter().map(|&(page, _)| page).collect();
    let tree = tree::write(&mut pdf, &mut next, list, &page_refs, &counts.mcids)?;
    // The colour objects go on last, as the oracle's `finish`: a name is allocated on first use, so the set is known only once every page is written.
    let colour = colour.map(|k| k.finish(&mut pdf, &mut next));

    let mut catalog = pdf.catalog(catalog_id);
    catalog.pages(pages_id);
    catalog.lang(TextStr(&list.info.lang));
    catalog.viewer_preferences().display_doc_title(true);
    catalog.metadata(xmp_id);
    catalog.pair(Name(b"Version"), Name(b"2.0"));
    catalog.pair(Name(b"StructTreeRoot"), tree.root);
    catalog.mark_info().marked(true);
    if let Some(outlines) = tree.outlines {
        catalog.pair(Name(b"Outlines"), outlines);
    }
    if let Some(labels) = tree.page_labels {
        catalog.pair(Name(b"PageLabels"), labels);
    }
    if let Some((_, intent, _)) = &colour {
        catalog.insert(Name(b"OutputIntents")).array().item(*intent);
    }
    catalog.finish();

    pdf.pages(pages_id).kids(page_ids.iter().map(|&(page, _)| page)).count(page_ids.len() as i32);
    for (index, ((page, &(page_id, content_id)), content)) in list.pages.iter().zip(&page_ids).zip(&contents).enumerate() {
        let mut writer = pdf.page(page_id);
        writer.parent(pages_id).media_box(rect(&page.boxes.media));
        if let Some(trim) = &page.boxes.trim {
            writer.trim_box(rect(trim));
        }
        if let Some(bleed) = &page.boxes.bleed {
            writer.bleed_box(rect(bleed));
        }
        // Every page names every instance, `F0`, `F1`, … in the order of `resources.fonts`, as the oracle's `setupPages` does.
        let mut resources = writer.resources();
        let mut fonts = resources.fonts();
        for (name, id) in &font_names {
            fonts.pair(Name(name.as_bytes()), *id);
        }
        fonts.finish();
        // The image items' XObjects and the vector items' in one dictionary, as the oracle's `applyToPage` merges them.
        let xobjects: Vec<_> = content.xobjects.iter().chain(content.scope.xobjects.iter().filter(|(n, _)| !content.xobjects.iter().any(|(m, _)| m == n))).collect();
        if !xobjects.is_empty() {
            let mut dict = resources.x_objects();
            for (name, id) in xobjects {
                dict.pair(Name(name.as_bytes()), *id);
            }
            dict.finish();
        }
        if !content.scope.shadings.is_empty() {
            let mut dict = resources.shadings();
            for (name, id) in &content.scope.shadings {
                dict.pair(Name(name.as_bytes()), *id);
            }
            dict.finish();
        }
        if !content.scope.patterns.is_empty() {
            let mut dict = resources.patterns();
            for (name, id) in &content.scope.patterns {
                dict.pair(Name(name.as_bytes()), *id);
            }
            dict.finish();
        }
        // `/DefaultRGB` on every page, and every separation and overprint state the document allocated, as the oracle's `ensurePageResources`.
        if let Some((names, _, _)) = &colour {
            let mut spaces = resources.color_spaces();
            spaces.insert(Name(b"DefaultRGB")).array().item(Name(b"ICCBased")).item(names.srgb);
            for (name, id) in &names.spaces {
                spaces.pair(Name(name.as_bytes()), *id);
            }
            spaces.finish();
        }
        // One `/ExtGState` per page: the colour story's overprint states, every one on every page, and the vector items' `GT<n>` where the page uses them.
        let colour_states = colour.as_ref().map_or(&[][..], |(names, _, _)| &names.states[..]);
        if !colour_states.is_empty() || !content.scope.ext_g_states.is_empty() {
            let mut states = resources.ext_g_states();
            for (name, id) in colour_states.iter().chain(&content.scope.ext_g_states) {
                states.pair(Name(name.as_bytes()), *id);
            }
            states.finish();
        }
        resources.finish();
        writer.contents(content_id);
        // `/Tabs /S` on every page, as the oracle's `setupPages` sets it: tab order follows the structure tree. `/StructParents` is the page's key in the ParentTree.
        writer.pair(Name(b"Tabs"), Name(b"S"));
        writer.pair(Name(b"StructParents"), index as i32);
        if !tree.annots[index].is_empty() {
            writer.insert(Name(b"Annots")).array().items(tree.annots[index].iter().copied());
        }
        writer.finish();
        pdf.stream(content_id, &fonts::deflate(&content.bytes)).filter(pdf_writer::Filter::FlateDecode);
    }

    let mut info = pdf.document_info(info_id);
    text_pair(&mut info, b"Title", &list.info.title);
    info.producer(TextStr(PRODUCER));
    info.creator(TextStr(PRODUCER));
    info.pair(Name(b"CreationDate"), Str(date.as_bytes()));
    info.pair(Name(b"ModDate"), Str(date.as_bytes()));
    info.finish();

    // Metadata streams stay uncompressed so they remain readable without the document, which is the whole point of the packet.
    pdf.metadata(xmp_id, xmp(list).as_bytes());

    // ISO 32000-2 §7.5.5 requires an `/ID` of every PDF 2.0 file, and the oracle writes none: both entries are the first 16 bytes of a SHA-256 over the file as written up to the cross-reference table, so the identifier is a function of the list.
    let id = Sha256::digest(pdf.as_bytes())[..16].to_vec();
    pdf.set_file_id((id.clone(), id));
    Ok(Written { bytes: pdf.finish(), records: Records { mcids: counts.mcids, pens: counts.pens, kern_adjustments: counts.kern_adjustments, actual_text_spans: counts.actual_text_spans, faces, images: counts.images, xobjects: images.xobjects.len(), paint, vector_resources, colour: colour.map(|(_, _, record)| record) } })
}

/// A box of the list as pdf-writer's rectangle, whose reals are `f32`.
fn rect(b: &[Num; 4]) -> Rect {
    Rect::new(b[0].0 as f32, b[1].0 as f32, b[2].0 as f32, b[3].0 as f32)
}

/// A text string under `key`: pdf-writer's `TextStr` where the text is Unicode, and UTF-16BE with its byte-order mark where it holds a lone surrogate, which `TextStr` cannot carry.
fn text_pair(dict: &mut pdf_writer::Dict, key: &[u8], text: &Utf16) {
    match String::from_utf16(&text.0) {
        Ok(s) => {
            dict.pair(Name(key), TextStr(&s));
        }
        Err(_) => {
            let bytes: Vec<u8> = [0xFE, 0xFF].into_iter().chain(text.0.iter().flat_map(|u| u.to_be_bytes())).collect();
            dict.pair(Name(key), Str(&bytes));
        }
    }
}

/// Lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// An ISO 8601 instant in UTC as a PDF date string (ISO 32000-2 §7.9.4) at whole seconds, `D:YYYYMMDDHHmmssZ`, made from the text alone, with no clock.
///
/// # Errors
/// A value that is not `YYYY-MM-DDTHH:mm:ss`, optional fractional seconds, `Z`, in the oracle's sentence.
pub fn pdf_date(iso: &str) -> Result<String, Refusal> {
    let b = iso.as_bytes();
    let digits = |range: std::ops::Range<usize>| b.get(range).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    let fraction = b.get(19..b.len().saturating_sub(1)).unwrap_or(&[]);
    let fraction_ok = fraction.is_empty() || (fraction.len() > 1 && fraction[0] == b'.' && fraction[1..].iter().all(u8::is_ascii_digit));
    let ok = b.len() >= 20 && digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10) && b[10] == b'T' && digits(11..13) && b[13] == b':' && digits(14..16) && b[16] == b':' && digits(17..19) && fraction_ok && b[b.len() - 1] == b'Z';
    if !ok {
        return Err(Refusal::new(format!("caster: the display list's info.created is {}, which is not an ISO 8601 instant in UTC (YYYY-MM-DDTHH:mm:ssZ)", list::json_string(&iso.encode_utf16().collect::<Vec<_>>()))));
    }
    Ok(format!("D:{}{}{}{}{}{}Z", &iso[0..4], &iso[5..7], &iso[8..10], &iso[11..13], &iso[14..16], &iso[17..19]))
}

/// `&`, `<` and `>` escaped for XML text content.
fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The XMP packet as Caster's `addXmpMetadata` builds it: dc:title, the producing tool twice, the PDF/UA-2 identification when the file claims it, the language; UTF-8, its title through `TextEncoder`'s rule (a lone surrogate becomes U+FFFD).
fn xmp(list: &DisplayList) -> String {
    let language = &list.info.lang;
    let title = escape_xml(&list.info.title.lossy());
    let tool = escape_xml(PRODUCER);
    let ua = if list.info.conformance == Conformance::Pdfua2 {
        "\n    <rdf:Description rdf:about=\"\" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\">\n      <pdfuaid:part>2</pdfuaid:part>\n      <pdfuaid:rev>2024</pdfuaid:rev>\n    </rdf:Description>"
    } else {
        ""
    };
    format!(
        "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">
  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">
    <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">
      <dc:title>
        <rdf:Alt>
          <rdf:li xml:lang=\"{language}\">{title}</rdf:li>
        </rdf:Alt>
      </dc:title>
    </rdf:Description>
    <rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">
      <xmp:CreatorTool>{tool}</xmp:CreatorTool>
    </rdf:Description>
    <rdf:Description rdf:about=\"\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">
      <pdf:Producer>{tool}</pdf:Producer>
    </rdf:Description>{ua}
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end=\"w\"?>"
    )
}

/// The blobs a list names, read from a folder of files named by their SHA-256: each name the list gives that is a SHA-256 in lowercase hex and names a file there. A name that is not, or names no file, is left out, and the validator refuses the list for it in its own sentence.
///
/// # Errors
/// A file that is there and cannot be read.
pub fn read_blobs(list: &DisplayList, dir: &std::path::Path) -> std::io::Result<Blobs> {
    let mut blobs = BTreeMap::new();
    for key in list.blob_keys() {
        let named = key.len() == 64 && key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let path = dir.join(key);
        if named && !blobs.contains_key(key) && path.is_file() {
            blobs.insert(key.to_string(), std::fs::read(path)?);
        }
    }
    Ok(blobs)
}

/// The tab's face is stage 9's (wasm-bindgen); until then one raw export keeps [`write`] reachable in the module, so its size can be measured: a list's JSON in linear memory, the length of the file it makes and of its records serialised out, as the tab's face will serialise them, or 0 for a refusal. Its empty blobs pass through `black_box`, so the optimiser cannot read the pool as empty and drop every font program, as it does under LTO.
#[cfg(target_arch = "wasm32")]
mod wasm_probe {
    /// Room for `len` bytes, leaked to the caller.
    #[unsafe(no_mangle)]
    pub extern "C" fn forme_alloc(len: usize) -> *mut u8 {
        let mut buf = Vec::<u8>::with_capacity(len);
        let ptr = buf.as_mut_ptr();
        std::mem::forget(buf);
        ptr
    }

    /// Write the list at `ptr`, `len` bytes of JSON, with no blobs, and serialise the records.
    ///
    /// # Safety
    /// `ptr` holds `len` initialised bytes the caller owns.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn forme_write(ptr: *const u8, len: usize) -> usize {
        let json = unsafe { std::slice::from_raw_parts(ptr, len) };
        let Ok(json) = std::str::from_utf8(json) else { return 0 };
        crate::list::parse(json).and_then(|list| crate::write(&list, std::hint::black_box(&Default::default()))).map_or(0, |written| written.bytes.len() + serde_json::to_string(&written.records).map_or(0, |r| r.len()))
    }
}
