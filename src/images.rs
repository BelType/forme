//! Image XObjects: the file's own bytes wherever PDF has a filter for them, and the list's samples deflated where it has none, ported from Caster's `src/images/imageEmbed.ts`, `jpeg.ts`, `png.ts` and `samples.ts`, and the image loops of `src/emit/structureEmit.ts`.
//!
//! - **One decision per image**: is there a filter that reads this file? A JPEG goes to `/DCTDecode` as it is, its component count the colour space, its APP14 Adobe marker a CMYK `/Decode [1 0 1 0 1 0 1 0]`, its APP2 profile an `/ICCBased` stream, an arithmetic-coded one handed through unread; a PNG whose `IDAT` `/Predictor 15` describes goes to `/FlateDecode` as it is, with `/DecodeParms`, its palette an `/Indexed` space, its `iCCP` an `/ICCBased` stream, its grey or RGB `tRNS` a colour-key `/Mask`. Any other PNG reached the list as samples (version 6, Egildo's ruling of 2026-11-03), so forme decodes nothing and links no PNG decoder.
//! - **Samples** are split from their interleaved alpha, at one or two bytes a sample, and the `/SMask` is written only where some pixel is not opaque; their colour space is the list's (`DeviceRGB`, `DeviceGray`, a palette's `/Indexed`, a profile's `/ICCBased`). The photo rule's JPEG is a JPEG under `/DCTDecode`.
//! - **The dedupe is the oracle's**: a SHA-256 over the payload, the alpha plane, the palette, the profile and a parameter string naming the road, its MIME type or label, the samples' size and `interpolate`, so the XObjects and their names `Im0`, `Im1`, … are the oracle's, the list's images in ascending key, then the facsimiles' Form XObjects ([`crate::facsimile`], `Fc<n>`), then the raster fallbacks'.

use std::collections::BTreeMap;
use std::io::Read;

use pdf_writer::{Filter, Name, Pdf, Ref};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::Refusal;
use crate::content::js_string;
use crate::fonts::deflate;
use crate::list::{Blobs, DisplayList, ImageKind, ImageSource, Item};

/// What one image XObject became in the file, as Caster's `WrittenImageInfo` names it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    pub path: &'static str,
    pub filter: &'static str,
    pub bits_per_component: u32,
    pub color_space: &'static str,
    pub has_s_mask: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decode: Option<[u8; 8]>,
    pub width: f64,
    pub height: f64,
    pub interpolate: bool,
    /// Stream bytes out, the `/SMask` included: forme's deflate, not the oracle's.
    pub bytes_out: usize,
    pub decode_ms: f64,
    pub encode_ms: f64,
    pub icc: &'static str,
    pub gamma_only: bool,
}

/// One image item painted, and the XObject it was painted with: Caster's `ImageUse`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ImageUse {
    pub page: usize,
    pub item: usize,
    pub source: u32,
    pub name: String,
    pub hash: String,
    pub info: ImageInfo,
}

/// One image XObject in the file.
pub struct XObject {
    pub id: Ref,
    /// Its resource name on every page that paints it.
    pub name: String,
    pub hash: String,
    pub info: ImageInfo,
}

/// Every image XObject the list's images and raster fallbacks made, and which each one became.
#[derive(Default)]
pub struct Images {
    pub xobjects: Vec<XObject>,
    /// The XObject of each painted image, by its key in `resources.images`.
    pub of_source: BTreeMap<u32, usize>,
    /// The XObject of each raster fallback with pixels, by its key in `resources.rasters`.
    pub of_raster: BTreeMap<u32, usize>,
}

/// What an image hands the embed: a file, the photo rule's JPEG, or samples split into planes.
enum Source<'a> {
    Bytes { bytes: &'a [u8], mime: &'a str },
    Jpeg { jpeg: &'a [u8], format: &'a str, gamma_only: bool },
    Samples(Samples<'a>),
}

/// Samples split from their alpha, with the shape the list gave them.
struct Samples<'a> {
    width: f64,
    height: f64,
    colour: Vec<u8>,
    alpha: Option<Vec<u8>>,
    space: &'a str,
    bits: u32,
    palette: Option<&'a [u8]>,
    icc: Option<&'a [u8]>,
    label: String,
    gamma_only: bool,
}

/// Embed every painted image of the list, in ascending key, then every painted facsimile, then every raster fallback with pixels, deduplicating by the oracle's key.
///
/// # Errors
/// A JPEG or a PNG the oracle refuses, in its sentence, wrapped as the oracle wraps it with the element; every refusal of [`crate::facsimile::write`].
pub fn write(pdf: &mut Pdf, next: &mut Ref, list: &DisplayList, blobs: &Blobs) -> Result<Images, Refusal> {
    let mut images = Images::default();
    let mut by_hash: BTreeMap<String, usize> = BTreeMap::new();
    // What each painted image is interpolated by: the last item to paint it decides, as in the oracle.
    let mut interpolate_of: BTreeMap<u32, bool> = BTreeMap::new();
    for page in &list.pages {
        for item in &page.items {
            if let Item::Image(image) = item {
                interpolate_of.insert(image.source.0 as u32, image.interpolate);
            }
        }
    }
    let mut facsimiles = Vec::new();
    for (&key, source) in &list.resources.images {
        let Some(&interpolate) = interpolate_of.get(&key) else { continue };
        if source.blob.is_none() && source.samples.is_none() && source.jpeg.is_none() {
            continue;
        }
        // Embedded after the loop, all together, one copy per source file, as the oracle's `embedFacsimiles`.
        if source.kind == ImageKind::Facsimile {
            facsimiles.push((key, source));
            continue;
        }
        let wrap = |r: Refusal| {
            let what = source.mime.as_deref().or(source.original.as_ref().map(|o| o.format.as_str())).unwrap_or("undefined");
            Refusal::new(format!("caster: the {what} at {} ({}) could not be carried into the file: {}", source.dom_path, source.src, r.message))
        };
        let embed = source_of(blobs, source);
        let at = embed_one(pdf, next, &mut images, &mut by_hash, &embed, interpolate).map_err(wrap)?;
        images.of_source.insert(key, at);
    }
    crate::facsimile::write(pdf, next, blobs, &facsimiles, &mut images, &mut by_hash)?;
    for (&key, raster) in &list.resources.rasters {
        let Some(blob) = &raster.blob else { continue };
        let embed = match &raster.jpeg {
            Some(jpeg) => Source::Jpeg { jpeg: &blobs[jpeg], format: "canvas", gamma_only: false },
            None => {
                let (width, height) = (raster.width.map_or(0.0, |w| w.0), raster.height.map_or(0.0, |h| h.0));
                let (colour, alpha) = split(&blobs[blob], width * height, 3, true, 1);
                Source::Samples(Samples { width, height, colour, alpha, space: "DeviceRGB", bits: 8, palette: None, icc: None, label: format!("{}@{}dpi", raster.feature, js_string(raster.dpi.0)), gamma_only: false })
            }
        };
        let at = embed_one(pdf, next, &mut images, &mut by_hash, &embed, true)?;
        images.of_raster.insert(key, at);
    }
    Ok(images)
}

/// What one list image hands the embed: the photo rule's JPEG, the samples, or the file.
fn source_of<'a>(blobs: &'a Blobs, source: &'a ImageSource) -> Source<'a> {
    let format = source.original.as_ref().map(|o| o.format.as_str()).or(source.mime.as_deref()).unwrap_or("");
    let gamma_only = source.original.as_ref().and_then(|o| o.gamma_only) == Some(true);
    if let Some(jpeg) = &source.jpeg {
        return Source::Jpeg { jpeg: &blobs[jpeg], format, gamma_only };
    }
    if let Some(s) = &source.samples {
        let components = if s.color_space == "DeviceRGB" { 3 } else { 1 };
        let bytes = (s.bits_per_component.0 / 8.0) as usize;
        let (colour, alpha) = split(&blobs[&s.blob], s.width.0 * s.height.0, components, s.alpha, bytes);
        return Source::Samples(Samples {
            width: s.width.0,
            height: s.height.0,
            colour,
            alpha,
            space: match s.color_space.as_str() {
                "DeviceGray" => "DeviceGray",
                "Indexed" => "Indexed",
                _ => "DeviceRGB",
            },
            bits: s.bits_per_component.0 as u32,
            palette: s.palette.as_ref().map(|p| blobs[p].as_slice()),
            icc: s.icc.as_ref().map(|p| blobs[p].as_slice()),
            label: format.to_string(),
            gamma_only,
        });
    }
    Source::Bytes { bytes: &blobs[source.blob.as_deref().unwrap_or_default()], mime: source.mime.as_deref().unwrap_or("") }
}

/// `samplesOf`: the colour plane and, where some pixel is not opaque, the alpha plane, from interleaved samples of `components` colour samples and an alpha where `alpha` is set, each `bytes` bytes. An opaque pixel's alpha bytes are all `0xff`, at either depth.
fn split(data: &[u8], pixels: f64, components: usize, alpha: bool, bytes: usize) -> (Vec<u8>, Option<Vec<u8>>) {
    if !alpha {
        return (data.to_vec(), None);
    }
    let pixels = pixels as usize;
    let colour_bytes = components * bytes;
    let stride = colour_bytes + bytes;
    let mut colour = Vec::with_capacity(pixels * colour_bytes);
    let mut plane = Vec::with_capacity(pixels * bytes);
    for pixel in data.chunks_exact(stride).take(pixels) {
        colour.extend_from_slice(&pixel[..colour_bytes]);
        plane.extend_from_slice(&pixel[colour_bytes..]);
    }
    let opaque = plane.iter().all(|&a| a == 0xff);
    (colour, (!opaque).then_some(plane))
}

/// `embedImage`: the XObject for a source, found by its hash or built and named `Im<n>`, n the count of XObjects before it.
fn embed_one(pdf: &mut Pdf, next: &mut Ref, images: &mut Images, by_hash: &mut BTreeMap<String, usize>, source: &Source, interpolate: bool) -> Result<usize, Refusal> {
    let hash = key(source, interpolate);
    if let Some(&at) = by_hash.get(&hash) {
        return Ok(at);
    }
    let id = next.bump();
    let info = match source {
        Source::Bytes { bytes, mime: "image/jpeg" } => jpeg_passthrough(pdf, next, id, bytes, interpolate)?,
        Source::Bytes { bytes, mime: "image/png" } => png_passthrough(pdf, next, id, bytes, interpolate)?,
        Source::Bytes { mime, .. } => return Err(Refusal::new(format!("imageEmbed: no PDF filter reads {mime}, and the display list holds the file and not its pixels; the image pass decodes such a file before the list"))),
        Source::Jpeg { jpeg, gamma_only, .. } => photo_jpeg(pdf, id, jpeg, interpolate, *gamma_only)?,
        Source::Samples(samples) => from_samples(pdf, next, id, samples, interpolate),
    };
    let at = images.xobjects.len();
    images.xobjects.push(XObject { id, name: format!("Im{}", by_hash.len()), hash: hash.clone(), info });
    by_hash.insert(hash, at);
    Ok(at)
}

/// The oracle's dedupe key: a SHA-256 over the payload, the alpha plane, the palette and the profile, then the parameters that changed how they are read, in hex.
fn key(source: &Source, interpolate: bool) -> String {
    let mut h = Sha256::new();
    let parameters = match source {
        Source::Bytes { bytes, mime } => {
            h.update(bytes);
            format!("bytes|{mime}|interpolate={interpolate}")
        }
        Source::Jpeg { jpeg, format, .. } => {
            h.update(jpeg);
            format!("jpeg|{format}|interpolate={interpolate}")
        }
        Source::Samples(s) => {
            h.update(&s.colour);
            h.update(s.alpha.as_deref().unwrap_or_default());
            h.update(s.palette.unwrap_or_default());
            h.update(s.icc.unwrap_or_default());
            // `shapeKey`: nothing for 8-bit RGB with no palette or profile, so that key is what it was before version 6.
            let shape = if s.space == "DeviceRGB" && s.bits == 8 && s.palette.is_none() && s.icc.is_none() {
                String::new()
            } else {
                format!("|{}|{}{}{}", s.space, s.bits, if s.palette.is_some() { "|palette" } else { "" }, if s.icc.is_some() { "|icc" } else { "" })
            };
            format!("samples|{}|{}x{}|interpolate={interpolate}{shape}", s.label, js_string(s.width), js_string(s.height))
        }
    };
    h.update(parameters.as_bytes());
    crate::hex(&h.finalize())
}

/// An `/ICCBased` profile stream, Flate-compressed, with `/N`.
pub(crate) fn icc_stream(pdf: &mut Pdf, next: &mut Ref, profile: &[u8], components: i32) -> Ref {
    let id = next.bump();
    pdf.icc_profile(id, &deflate(profile)).n(components).filter(Filter::FlateDecode);
    id
}

/// A colour space as the image's dictionary writes it.
enum Space<'a> {
    Device(&'a str),
    Indexed(&'a [u8]),
    Icc(Ref),
}

/// An image XObject's dictionary beside its stream: the colour space, the depth, the filter, `/Interpolate` (which an `/SMask` does not carry), and what each road adds.
#[allow(clippy::too_many_arguments)]
fn image(pdf: &mut Pdf, id: Ref, data: &[u8], width: f64, height: f64, space: &Space, bits: u32, filter: Filter, interpolate: Option<bool>, extra: impl FnOnce(&mut pdf_writer::writers::ImageXObject)) {
    let mut x = pdf.image_xobject(id, data);
    x.width(width as i32).height(height as i32);
    match space {
        Space::Device(name) => {
            x.color_space_name(Name(name.as_bytes()));
        }
        Space::Indexed(palette) => x.color_space().indexed(Name(b"DeviceRGB"), (palette.len() / 3) as i32 - 1, palette),
        Space::Icc(icc) => x.color_space().icc_based(*icc),
    }
    x.bits_per_component(bits as i32).filter(filter);
    if let Some(interpolate) = interpolate {
        x.interpolate(interpolate);
    }
    extra(&mut x);
}

/// A JPEG's header, as `parseJpeg` reads it, with what the writer needs.
pub struct JpegInfo {
    pub width: u32,
    pub height: u32,
    pub components: u8,
    pub precision: u8,
    pub sof_marker: u8,
    pub arithmetic: bool,
    pub unsupported_frame: bool,
    pub icc_profile: Option<Vec<u8>>,
    pub adobe: bool,
}

/// `parseJpeg`: the frame header, the APP2 `ICC_PROFILE` chunks reassembled by their index, the APP14 Adobe marker, up to the first scan.
///
/// # Errors
/// No SOI, a byte that is not a marker where one is due, and no frame before the first scan, in the oracle's sentences.
pub fn parse_jpeg(bytes: &[u8]) -> Result<JpegInfo, Refusal> {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return Err(Refusal::new("jpeg: no SOI marker".into()));
    }
    let (mut width, mut height, mut components, mut precision, mut sof) = (0u32, 0u32, 0u8, 0u8, 0u8);
    let mut adobe = false;
    let mut chunks: Vec<(u8, &[u8])> = Vec::new();
    let mut at = 2;
    while at + 1 < bytes.len() {
        if bytes[at] != 0xff {
            return Err(Refusal::new(format!("jpeg: expected a marker at byte {at}, found 0x{:x}", bytes[at])));
        }
        let mut marker = bytes[at + 1];
        while marker == 0xff && at + 2 < bytes.len() {
            at += 1;
            marker = bytes[at + 1];
        }
        if matches!(marker, 0x01 | 0xd0..=0xd9) {
            at += 2;
            continue;
        }
        if at + 4 > bytes.len() {
            break;
        }
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        let payload = bytes.get(at + 4..(at + 2 + length).min(bytes.len())).unwrap_or_default();
        let byte = |i: usize| payload.get(i).copied().unwrap_or(0);
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            sof = marker;
            precision = byte(0);
            height = u32::from(byte(1)) << 8 | u32::from(byte(2));
            width = u32::from(byte(3)) << 8 | u32::from(byte(4));
            components = byte(5);
        } else if marker == 0xe2 && payload.starts_with(b"ICC_PROFILE\0") {
            chunks.push((byte(12), payload.get(14..).unwrap_or_default()));
        } else if marker == 0xee && payload.starts_with(b"Adobe") {
            adobe = true;
        } else if marker == 0xda {
            break;
        }
        at += 2 + length;
    }
    if sof == 0 {
        return Err(Refusal::new("jpeg: no start-of-frame marker before the first scan".into()));
    }
    // A stable sort by index, as `Array.prototype.sort` is.
    chunks.sort_by_key(|&(index, _)| index);
    let icc_profile = (!chunks.is_empty()).then(|| chunks.iter().flat_map(|&(_, b)| b.iter().copied()).collect());
    let arithmetic = sof == 0xc9 || sof == 0xca;
    let supported = sof == 0xc0 || sof == 0xc1 || sof == 0xc2 || arithmetic;
    Ok(JpegInfo { width, height, components, precision, sof_marker: sof, arithmetic, unsupported_frame: !supported || sof == 0xc3, icc_profile, adobe })
}

/// `jpegColorSpaceName`.
fn jpeg_space(components: u8) -> Result<&'static str, Refusal> {
    match components {
        1 => Ok("DeviceGray"),
        3 => Ok("DeviceRGB"),
        4 => Ok("DeviceCMYK"),
        n => Err(Refusal::new(format!("jpeg: a frame with {n} components has no PDF colour space"))),
    }
}

/// A JPEG file under `/DCTDecode`, as it is.
fn jpeg_passthrough(pdf: &mut Pdf, next: &mut Ref, id: Ref, bytes: &[u8], interpolate: bool) -> Result<ImageInfo, Refusal> {
    let info = parse_jpeg(bytes)?;
    if info.unsupported_frame {
        return Err(Refusal::new(format!("imageEmbed: a JPEG frame marker 0x{:x} PDF cannot carry", info.sof_marker)));
    }
    if info.precision != 8 {
        return Err(Refusal::new(format!("imageEmbed: a JPEG at {} bits per sample cannot be carried by /DCTDecode, which is defined over 8-bit samples; re-save it at 8 bits", info.precision)));
    }
    let name = jpeg_space(info.components)?;
    let space = match &info.icc_profile {
        Some(profile) => Space::Icc(icc_stream(pdf, next, profile, i32::from(info.components))),
        None => Space::Device(name),
    };
    // Adobe writes CMYK with the samples inverted, and the APP14 marker is the whole signal.
    let decode = (info.components == 4 && info.adobe).then_some([1, 0, 1, 0, 1, 0, 1, 0]);
    image(pdf, id, bytes, f64::from(info.width), f64::from(info.height), &space, 8, Filter::DctDecode, Some(interpolate), |x| {
        if let Some(d) = decode {
            x.decode(d.map(f32::from));
        }
    });
    let icc = info.icc_profile.is_some();
    Ok(ImageInfo { path: "passthrough", filter: "DCTDecode", bits_per_component: 8, color_space: if icc { "ICCBased" } else { name }, has_s_mask: false, decode, width: f64::from(info.width), height: f64::from(info.height), interpolate, bytes_out: bytes.len(), decode_ms: 0.0, encode_ms: 0.0, icc: if icc { "embedded" } else { "none" }, gamma_only: false })
}

/// The photo rule's JPEG under `/DCTDecode`, in `DeviceRGB`.
fn photo_jpeg(pdf: &mut Pdf, id: Ref, jpeg: &[u8], interpolate: bool, gamma_only: bool) -> Result<ImageInfo, Refusal> {
    let frame = parse_jpeg(jpeg)?;
    image(pdf, id, jpeg, f64::from(frame.width), f64::from(frame.height), &Space::Device("DeviceRGB"), 8, Filter::DctDecode, Some(interpolate), |_| {});
    Ok(ImageInfo { path: "decode-jpeg", filter: "DCTDecode", bits_per_component: 8, color_space: "DeviceRGB", has_s_mask: false, decode: None, width: f64::from(frame.width), height: f64::from(frame.height), interpolate, bytes_out: jpeg.len(), decode_ms: 0.0, encode_ms: 0.0, icc: "none", gamma_only })
}

/// A PNG's chunks as `parsePng` reads them, with what the writer needs.
pub struct PngInfo {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub color_type: u8,
    pub interlace: u8,
    pub palette: Option<Vec<u8>>,
    pub trns: Option<Vec<u8>>,
    pub gama: bool,
    pub iccp_deflated: Option<Vec<u8>>,
    pub srgb: bool,
    pub idat: Vec<u8>,
}

/// `parsePng`: `IHDR`, `PLTE`, `tRNS`, `gAMA`, `sRGB`, `iCCP`, and every `IDAT` concatenated, up to `IEND`.
///
/// # Errors
/// No PNG signature, or no `IHDR`, in the oracle's sentences.
pub fn parse_png(bytes: &[u8]) -> Result<PngInfo, Refusal> {
    if !bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Err(Refusal::new("png: not a PNG signature".into()));
    }
    let mut info = PngInfo { width: 0, height: 0, bit_depth: 0, color_type: 2, interlace: 0, palette: None, trns: None, gama: false, iccp_deflated: None, srgb: false, idat: Vec::new() };
    let mut at = 8;
    while at + 8 <= bytes.len() {
        let length = u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let kind = &bytes[at + 4..at + 8];
        let payload = bytes.get(at + 8..(at + 8 + length).min(bytes.len())).unwrap_or_default();
        let byte = |i: usize| payload.get(i).copied().unwrap_or(0);
        let word = |i: usize| u32::from_be_bytes([byte(i), byte(i + 1), byte(i + 2), byte(i + 3)]);
        match kind {
            b"IHDR" => {
                info.width = word(0);
                info.height = word(4);
                info.bit_depth = byte(8);
                info.color_type = byte(9);
                info.interlace = byte(12);
            }
            b"PLTE" => info.palette = Some(payload.to_vec()),
            b"tRNS" => info.trns = Some(payload.to_vec()),
            b"gAMA" => info.gama = true,
            b"sRGB" => info.srgb = true,
            b"iCCP" => {
                let nul = payload.iter().position(|&b| b == 0).unwrap_or(payload.len());
                info.iccp_deflated = Some(payload.get(nul + 2..).unwrap_or_default().to_vec());
            }
            b"IDAT" => info.idat.extend_from_slice(payload),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + length;
    }
    if info.width == 0 || info.height == 0 {
        return Err(Refusal::new("png: no IHDR".into()));
    }
    Ok(info)
}

/// `canPassThroughIdat`: no alpha channel, no Adam7, no palette with `tRNS`.
pub fn can_pass_through_idat(info: &PngInfo) -> bool {
    info.color_type != 4 && info.color_type != 6 && info.interlace == 0 && !(info.color_type == 3 && info.trns.is_some())
}

/// `colourKeyMask`: the `/Mask` min/max pairs a grey or RGB `tRNS` names, two bytes a value at every depth.
fn colour_key_mask(info: &PngInfo) -> Option<Vec<i32>> {
    let trns = info.trns.as_ref()?;
    let at = |i: usize| i32::from(trns[i * 2]) << 8 | i32::from(trns[i * 2 + 1]);
    match info.color_type {
        0 if trns.len() >= 2 => Some(vec![at(0), at(0)]),
        2 if trns.len() >= 6 => Some(vec![at(0), at(0), at(1), at(1), at(2), at(2)]),
        _ => None,
    }
}

/// A PNG whose `IDAT` `/Predictor 15` describes, under `/FlateDecode` as it is, with `/DecodeParms`.
fn png_passthrough(pdf: &mut Pdf, next: &mut Ref, id: Ref, bytes: &[u8], interpolate: bool) -> Result<ImageInfo, Refusal> {
    let info = parse_png(bytes)?;
    let gamma_only = info.gama && info.iccp_deflated.is_none() && !info.srgb;
    if !can_pass_through_idat(&info) {
        return Err(Refusal::new("imageEmbed: a PNG whose IDAT /Predictor 15 cannot describe (an alpha channel, Adam7 interlace, a palette with tRNS) reaches the display list as samples; the image pass decodes it before the list".into()));
    }
    let (space, name, components, icc) = if info.color_type == 3 {
        let palette = info.palette.as_deref().ok_or_else(|| Refusal::new("imageEmbed: a palettised PNG with no PLTE".into()))?;
        (Space::Indexed(palette), "Indexed", 1, false)
    } else {
        let components = if info.color_type == 2 { 3 } else { 1 };
        match &info.iccp_deflated {
            Some(deflated) => {
                let mut profile = Vec::new();
                flate2::read::ZlibDecoder::new(deflated.as_slice()).read_to_end(&mut profile).map_err(|e| Refusal::new(format!("png: the iCCP profile does not inflate: {e}")))?;
                (Space::Icc(icc_stream(pdf, next, &profile, components)), "ICCBased", components, true)
            }
            None => {
                let device = if components == 3 { "DeviceRGB" } else { "DeviceGray" };
                (Space::Device(device), device, components, false)
            }
        }
    };
    let mask = colour_key_mask(&info);
    let bits = u32::from(info.bit_depth);
    image(pdf, id, &info.idat, f64::from(info.width), f64::from(info.height), &space, bits, Filter::FlateDecode, Some(interpolate), |x| {
        x.decode_parms().predictor(pdf_writer::types::Predictor::PngOptimum).colors(components).bits_per_component(bits as i32).columns(info.width as i32);
        if let Some(mask) = mask {
            x.color_mask(mask);
        }
    });
    Ok(ImageInfo { path: "passthrough", filter: "FlateDecode", bits_per_component: bits, color_space: name, has_s_mask: false, decode: None, width: f64::from(info.width), height: f64::from(info.height), interpolate, bytes_out: info.idat.len(), decode_ms: 0.0, encode_ms: 0.0, icc: if icc { "embedded" } else { "none" }, gamma_only })
}

/// Samples, deflated, with an `/SMask` where some pixel is not opaque; the colour space is made first, so a profile is written before the streams, as in the oracle.
fn from_samples(pdf: &mut Pdf, next: &mut Ref, id: Ref, s: &Samples, interpolate: bool) -> ImageInfo {
    let (space, name, icc) = match (s.space, s.palette, s.icc) {
        ("Indexed", Some(palette), _) => (Space::Indexed(palette), "Indexed", false),
        (space, _, Some(profile)) => (Space::Icc(icc_stream(pdf, next, profile, if space == "DeviceRGB" { 3 } else { 1 })), "ICCBased", true),
        (space, _, None) => (Space::Device(space), if space == "DeviceGray" { "DeviceGray" } else { "DeviceRGB" }, false),
    };
    let colour = deflate(&s.colour);
    let mask = s.alpha.as_ref().map(|alpha| {
        let mask_id = next.bump();
        let compressed = deflate(alpha);
        image(pdf, mask_id, &compressed, s.width, s.height, &Space::Device("DeviceGray"), s.bits, Filter::FlateDecode, None, |_| {});
        (mask_id, compressed.len())
    });
    image(pdf, id, &colour, s.width, s.height, &space, s.bits, Filter::FlateDecode, Some(interpolate), |x| {
        if let Some((mask_id, _)) = mask {
            x.s_mask(mask_id);
        }
    });
    ImageInfo { path: "decode-flate", filter: "FlateDecode", bits_per_component: s.bits, color_space: name, has_s_mask: mask.is_some(), decode: None, width: s.width, height: s.height, interpolate, bytes_out: colour.len() + mask.map_or(0, |(_, n)| n), decode_ms: 0.0, encode_ms: 0.0, icc: if icc { "embedded" } else { "none" }, gamma_only: s.gamma_only }
}
