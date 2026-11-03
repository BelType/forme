//! The facsimile: a page of another PDF carried into the file as a Form XObject, vectors, fonts and all, ported from Caster's `src/images/facsimile.ts` (`reopenFacsimile`, `embedFacsimiles`) and *a `<beltype-facsimile>` is a figure whose content is a Form XObject* (Caster's DESIGN.md S80).
//!
//! - **One form per distinct page of a distinct file**: the oracle's dedupe key, a SHA-256 over the file's bytes and `facsimile|page=<index>`, in the images' one table, so a facsimile is named `Fc<n>`, n the count of XObjects before it, after every image of the list and before the raster fallbacks.
//! - **One copy per file**: the pages of one source are copied by one call of `hayro-write`, so a font two pages share is written once, as the oracle's one `embedPages` call per file does.
//! - **The form is pdf-lib's**: `/BBox` the crop box (the media box where there is none), `/Matrix` the move of that box to the origin, the content wrapped in two `q … Q` pairs, no `/Group`; forme's copy of `hayro-write` writes it so (`vendor/hayro-write`, `NOTICE`). The page draws it with the unit-square scale of `imageOperators`, from the box's width and height as the source file writes them.
//! - **Refusals**: an encrypted file, read off the trailer's `/Encrypt` (forme's copy of `hayro-syntax` has no crypto), a file that is not a PDF, and a page past the file's end, in the sentences of the oracle's `loadFacsimiles`, which refuses all three before the list. The rotation and the unembedded fonts are that inspection's alone, as in the oracle's writer.

use std::collections::BTreeMap;

use hayro_syntax::object::dict::keys::{CROP_BOX, MEDIA_BOX, PARENT};
use hayro_syntax::object::{Array, Dict};
use hayro_syntax::{LoadPdfError, Pdf as Source};
use hayro_write::{ChunkSettings, ExtractionQuery};
use pdf_writer::{Pdf, Ref};
use sha2::{Digest, Sha256};

use crate::Refusal;
use crate::content::js_string;
use crate::images::{ImageInfo, Images, XObject};
use crate::list::{Blobs, ImageSource};

/// One page wanted from one file, with the list's images that paint it.
struct Wanted {
    hash: String,
    index: usize,
    keys: Vec<u32>,
}

/// Embed the facsimiles among `sources`, in their order, each list key mapped to its form in `images`.
///
/// # Errors
/// An encrypted file, a file that is not a PDF, a page past the file's end, or a page with no box, each naming the element.
pub fn write(pdf: &mut Pdf, next: &mut Ref, blobs: &Blobs, sources: &[(u32, &ImageSource)], images: &mut Images, by_hash: &mut BTreeMap<String, usize>) -> Result<(), Refusal> {
    // By the source's URL, in the order of first appearance, as the oracle's `wanted` map.
    let mut files: Vec<(&str, &ImageSource, Vec<Wanted>)> = Vec::new();
    for &(key, source) in sources {
        let facsimile = source.facsimile.as_ref().ok_or_else(|| Refusal::new(format!("forme: the facsimile at {} names no page", source.dom_path)))?;
        let bytes = &blobs[source.blob.as_deref().unwrap_or_default()];
        let index = facsimile.page.0 - 1.0;
        let mut h = Sha256::new();
        h.update(bytes);
        h.update(format!("facsimile|page={}", js_string(index)).as_bytes());
        let hash = crate::hex(&h.finalize());
        if let Some(&at) = by_hash.get(&hash) {
            images.of_source.insert(key, at);
            continue;
        }
        let at = match files.iter().position(|(url, _, _)| *url == facsimile.url) {
            Some(at) => at,
            None => {
                files.push((&facsimile.url, source, Vec::new()));
                files.len() - 1
            }
        };
        let group = &mut files[at].2;
        match group.iter_mut().find(|w| w.hash == hash) {
            Some(same) => same.keys.push(key),
            None => group.push(Wanted { hash, index: index as usize, keys: vec![key] }),
        }
    }
    for (url, first, group) in files {
        let at = &first.dom_path;
        let bytes = blobs[first.blob.as_deref().unwrap_or_default()].clone();
        let file = Source::new(bytes).map_err(|e| match e {
            LoadPdfError::Decryption(_) => Refusal::new(format!("caster: the facsimile at {at} names {url}, which is encrypted; an encrypted file cannot be carried into a PDF/UA-2 file, and the printer asks for no password. Ship the page from an unencrypted copy")),
            LoadPdfError::Invalid => Refusal::new(format!("caster: the facsimile at {at} names {url}, which cannot be read as a PDF: no cross-reference table and no page tree could be read")),
        })?;
        let pages = file.pages();
        let mut sizes = Vec::with_capacity(group.len());
        for wanted in &group {
            let count = pages.len();
            let page = pages.get(wanted.index).ok_or_else(|| Refusal::new(format!("caster: the facsimile at {at} asks for page {} of {url}, which has {count} {}", wanted.index + 1, if count == 1 { "page" } else { "pages" })))?;
            sizes.push(size(page.raw()).ok_or_else(|| Refusal::new(format!("caster: page {} of {url} (the facsimile at {at}) has no MediaBox", wanted.index + 1)))?);
        }
        let queries: Vec<ExtractionQuery> = group.iter().map(|w| ExtractionQuery::new_xobject(w.index)).collect();
        let extracted = hayro_write::extract(&file, Box::new(|| next.bump()), ChunkSettings::default(), |_| {}, &queries).map_err(|e| Refusal::new(format!("caster: the facsimile at {at} names {url}, which cannot be copied: {e:?}")))?;
        pdf.extend(&extracted.chunk);
        for ((wanted, root), (width, height)) in group.into_iter().zip(extracted.root_refs).zip(sizes) {
            let id = root.map_err(|e| Refusal::new(format!("caster: the facsimile at {at} names {url}, which cannot be copied: {e:?}")))?;
            let info = ImageInfo { path: "form", filter: "FlateDecode", bits_per_component: 0, color_space: "n/a", has_s_mask: false, decode: None, width, height, interpolate: false, bytes_out: 0, decode_ms: 0.0, encode_ms: 0.0, icc: "none", gamma_only: false };
            let index = images.xobjects.len();
            images.xobjects.push(XObject { id, name: format!("Fc{}", by_hash.len()), hash: wanted.hash.clone(), info });
            by_hash.insert(wanted.hash, index);
            for key in wanted.keys {
                images.of_source.insert(key, index);
            }
        }
    }
    Ok(())
}

/// The width and height of the crop box, the media box where there is none, each inherited through `/Parent`, as pdf-lib's `getCropBox` reads them: the right edge less the left and the top less the bottom, in doubles, as the source file writes the numbers.
fn size(page: &Dict<'_>) -> Option<(f64, f64)> {
    let inherited = |key: &[u8]| {
        let mut dict = page.clone();
        // ponytail: 64 levels of page tree, a cycle's guard; no real file is deeper.
        for _ in 0..64 {
            if let Some(array) = dict.get::<Array<'_>>(key) {
                return Some(array);
            }
            dict = dict.get::<Dict<'_>>(PARENT)?;
        }
        None
    };
    let array = inherited(CROP_BOX).or_else(|| inherited(MEDIA_BOX))?;
    let n: Vec<f64> = array.iter::<f64>().collect();
    (n.len() == 4).then(|| (n[2] - n[0], n[3] - n[1]))
}
