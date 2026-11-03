//! The fonts: one embedded program per instance of `resources.fonts`, in that order, each written as a Type 0 font over a CID font, its descriptor, its program and its ToUnicode CMap, and what each became, as Caster's `CidFontStats` names it ([`FaceRecord`]). Ported from Caster's `src/fonts/fontEmbed.ts`, `toUnicode.ts` and `clusters.ts`, and `textEmit.ts`'s `listSegments` and `textForGlyph`.
//!
//! - **The glyph set is the oracle's**: every glyph a text item shows and every space glyph it writes (a leading space, and one before each segment after the first that DOM whitespace separated), in painting order, which is also the order the first-seen text and its conflicts are counted in.
//! - **A glyph's CID is its id in the subset.** subsetter cannot keep glyph ids, so the subset is cut over the glyph set ascending with `.notdef` first: the CID of the k-th glyph of `glyphsUsed` is k + 1, for TrueType and CFF alike, with no `/CIDToGIDMap` stream; a set holding `.notdef` itself (a space the face has no glyph for, as the CFF2 list's run shows) keeps it at CID 0 and the rest follow from 1. subsetter appends the components of a composite glyph after the set, where no CID names them. Every member of the record that lists glyphs (`glyphsUsed`, `cidToText`, `ambiguousCids`) holds the list's glyph ids, as the oracle's does, so the two records compare member for member.
//! - **The widths are the list's** (*the widths are HarfBuzz's, the shaper's own*, Caster's DESIGN.md S99, amended 2026-11-02). `/W` is `widths.units` scaled to 1000 at the profile's decimals, and the program's `hmtx` must carry them to the unit: where subsetter's instancing rounds a glyph otherwise, the table is rewritten with every glyph a long metric, `numberOfHMetrics` the glyph count, and the checksums and `checkSumAdjustment` recomputed (the widths trial, `.claude/plans/the-rust-writer/trial-widths-patch.md` in the BelType monorepo). Then every glyph of the set is read back through skrifa, and a program that does not carry the list's width is refused in the oracle's sentence.
//! - **No hinting tables**: the `cvt `, `fpgm` and `prep` subsetter keeps for a face it does not instance are left out, as the oracle asks HarfBuzz for `NO_HINTING`.
//! - **CFF2 is written**, where the oracle refuses it: subsetter instances it to `glyf`, so it is a `CIDFontType2` held to the list's widths like any other.
//! - **The program is CFF bare or wrapped** as `writing.fontProfile.cffContainer` says: `/FontFile3` with `/CIDFontType0C` holds the `CFF ` table alone, `/OpenType` the whole SFNT.
//! - **No clock**: the record's `ms` is zero in every member, because the writer reads none.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::str::FromStr;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use pdf_writer::types::{CidFontType, FontFlags, SystemInfo};
use pdf_writer::{Filter, Finish, Name, Pdf, Rect, Ref, Str};
use skrifa::instance::{Location, LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::TableProvider;
use skrifa::raw::ps::cff::CffFontRef;
use skrifa::string::StringId;
use skrifa::{FontRef, GlyphId, MetadataProvider};
use subsetter::{GlyphRemapper, Tag};

use crate::content::{js_round, js_string};
use crate::list::{Blobs, CffContainer, DisplayList, FontInstance, Item, Num, SourceFormat, TextItem, Utf16, json_string};
use crate::Refusal;

/// Glyph-space units per em in a PDF font dictionary.
const PDF_GLYPH_UNITS: f64 = 1000.0;
/// The one descriptor number no font table carries, the oracle's conventional upright text weight.
const STEM_V: f32 = 80.0;
/// bfchar blocks cap at 100 entries each, per the CMap specification.
const BFCHAR_BLOCK: usize = 100;
/// The tables a program written into a PDF 2.0 file may not keep: no reader takes a variable font.
const VARIABLE_TABLES: [&[u8; 4]; 6] = [b"fvar", b"gvar", b"avar", b"HVAR", b"MVAR", b"CFF2"];
/// The hinting tables left out of every program.
const HINTING_TABLES: [&[u8; 4]; 3] = [b"cvt ", b"fpgm", b"prep"];

/// What one instance became, as Caster's `CidFontStats` (`src/displayList/contract.ts`) names it, serialised as that type's JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceRecord {
    pub id: String,
    /// The source's PostScript name (`name` ID 6), as the face declares it.
    pub post_script_name: String,
    pub used_glyphs: usize,
    /// Bytes of the embedded program as the stream holds it, before Flate.
    pub font_file_bytes: usize,
    pub multi_char_to_unicode: usize,
    pub to_unicode_conflicts: usize,
    pub face_id: String,
    pub family: String,
    pub source_format: SourceFormat,
    /// `glyf`, `cff` or `cff-cid`: what the embedded program carries. subsetter writes every CFF as CID-keyed.
    pub outline_type: &'static str,
    pub upem: u16,
    pub coords: BTreeMap<String, Num>,
    /// The list's glyph ids shown, ascending; the CID of the k-th is k + 1 (k when the set holds `.notdef`).
    pub glyphs_used: Vec<u16>,
    pub source_bytes: usize,
    pub subset_bytes: usize,
    pub subset_flate_bytes: usize,
    pub num_glyphs: u16,
    pub non_empty_glyphs: usize,
    pub hmtx_instanced: bool,
    pub stale_advances: u32,
    pub subset_tag: String,
    pub cff_container: CffContainer,
    /// By the list's glyph id.
    pub cid_to_text: BTreeMap<u16, Vec<Utf16>>,
    pub ambiguous_cids: Vec<u16>,
    pub tables: Vec<String>,
    pub charset_identity: Option<bool>,
    pub charset_retried: bool,
    pub fs_type: String,
    pub path: &'static str,
    pub ms: Timing,
}

/// The record's timings: zero, since the writer reads no clock.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timing {
    pub instance: f64,
    pub parse: f64,
    pub subset: f64,
    pub write: f64,
}

/// One instance's program, cut and checked, before it is written into the file.
pub struct Program {
    /// The SFNT, as the `glyf` stream or the `/OpenType` stream holds it, and as the bare CFF is cut from.
    pub bytes: Vec<u8>,
    /// The list's glyph id to its CID.
    pub cids: BTreeMap<u16, u16>,
    /// The glyphs whose advance subsetter wrote otherwise and the patch corrected, ascending.
    pub patched: Vec<u16>,
}

/// One segment of a run, as the writer and the ToUnicode map read it: its text and, for each glyph, its id and what it stands for (the cluster's characters on the cluster's first glyph, `None` on the rest).
pub(crate) struct Segment<'a> {
    pub(crate) text: &'a [u16],
    pub(crate) gids: Vec<u16>,
    pub(crate) texts: Vec<Option<Vec<u16>>>,
    pub(crate) space_before: bool,
}

impl Segment<'_> {
    /// `textForGlyph`: the characters glyph `j` stands for, empty where it is not its cluster's first.
    pub(crate) fn text_for(&self, j: usize) -> &[u16] {
        self.texts[j].as_deref().unwrap_or(&[])
    }
}

/// JavaScript's `slice(from, to)` on code units.
fn slice(text: &[u16], from: i64, to: i64) -> &[u16] {
    let clamp = |i: i64| i.clamp(0, text.len() as i64) as usize;
    let (a, b) = (clamp(from), clamp(to));
    if b > a { &text[a..b] } else { &[] }
}

/// `segmentViews` and `clusterGroups`: a text item's segments, each glyph's text given by its cluster.
pub(crate) fn segments(item: &TextItem) -> Vec<Segment<'_>> {
    let text = &item.text.0[..];
    let s = &item.segments;
    let count = s.start.len();
    (0..count)
        .map(|index| {
            let from = s.start[index].0 as i64;
            let to = if index + 1 < count { s.start[index + 1].0 as i64 - i64::from(s.space_before[index + 1]) } else { text.len() as i64 };
            let word = slice(text, from, to);
            let first = s.first_glyph[index].0 as usize;
            let end = if index + 1 < count { s.first_glyph[index + 1].0 as usize } else { item.glyphs.ids.len() };
            let gids: Vec<u16> = item.glyphs.ids[first..end].iter().map(|g| g.0 as u16).collect();
            let clusters: Vec<i64> = item.glyphs.clusters[first..end].iter().map(|c| c.0 as i64 - from).collect();
            let rtl = clusters.len() > 1 && clusters[0] > clusters[clusters.len() - 1];
            let logical: Vec<usize> = if rtl { (0..clusters.len()).rev().collect() } else { (0..clusters.len()).collect() };
            let mut groups: Vec<(i64, usize)> = Vec::new();
            for gi in logical {
                if groups.last().is_none_or(|&(start, _)| start != clusters[gi]) {
                    groups.push((clusters[gi], gi));
                }
            }
            let mut texts = vec![None; gids.len()];
            for (g, &(start, glyph)) in groups.iter().enumerate() {
                let end = groups.get(g + 1).map_or(word.len() as i64, |&(next, _)| next);
                texts[glyph] = Some(if end > start { slice(word, start, end).to_vec() } else { Vec::new() });
            }
            Segment { text: word, gids, texts, space_before: s.space_before[index] }
        })
        .collect()
}

/// What `use(gid, text)` collects for one instance as the runs are written: the glyph set, the first text each glyph was shown for, and how many later uses showed another.
#[derive(Default)]
struct Usage {
    used: BTreeSet<u16>,
    first_seen: BTreeMap<u16, Vec<u16>>,
    conflicts: usize,
}

impl Usage {
    fn record(&mut self, gid: u16, text: &[u16]) {
        self.used.insert(gid);
        match self.first_seen.get(&gid) {
            None => {
                self.first_seen.insert(gid, text.to_vec());
            }
            Some(seen) if seen != text => self.conflicts += 1,
            Some(_) => {}
        }
    }
}

/// The document-wide ToUnicode map of one instance: each glyph's chosen text, every text it stood for (most frequent first), and the glyphs shown for more than one.
#[derive(Default)]
struct InstanceMap {
    text: BTreeMap<u16, Vec<u16>>,
    cid_to_text: BTreeMap<u16, Vec<Utf16>>,
    ambiguous: Vec<u16>,
}

/// `buildToUnicode` over `listSegments`: per instance, per glyph, the string most of the document shows it for, ties broken on the string.
///
/// # Errors
/// A segment that painted `.notdef`, or a cluster whose first glyph stands for no characters, in the oracle's sentences, naming the item.
fn to_unicode(list: &DisplayList) -> Result<BTreeMap<&str, InstanceMap>, Refusal> {
    let mut counts: BTreeMap<&str, BTreeMap<u16, BTreeMap<Vec<u16>, usize>>> = BTreeMap::new();
    for (p, page) in list.pages.iter().enumerate() {
        for (at, item) in page.items.iter().enumerate() {
            let Item::Text(text) = item else { continue };
            let per = counts.entry(text.font.as_str()).or_default();
            for segment in segments(text) {
                for (j, &gid) in segment.gids.iter().enumerate() {
                    if gid == 0 {
                        return Err(Refusal::at(format!("caster: {} painted .notdef in {} — no face covers that cluster, and a glyph a reader cannot map is a document a reader cannot extract", text.font, json_string(segment.text)), p, at));
                    }
                    let glyph_text = segment.text_for(j);
                    if glyph_text.is_empty() && segment.texts[j].is_some() {
                        return Err(Refusal::at(format!("caster: glyph {gid} of {} begins a cluster that maps to no characters in {}", text.font, json_string(segment.text)), p, at));
                    }
                    if !glyph_text.is_empty() {
                        *per.entry(gid).or_default().entry(glyph_text.to_vec()).or_default() += 1;
                    }
                }
            }
        }
    }
    Ok(counts
        .into_iter()
        .map(|(key, per)| {
            let mut map = InstanceMap::default();
            for (gid, per_cid) in per {
                let mut variants: Vec<(Vec<u16>, usize)> = per_cid.into_iter().collect();
                // By count, most first, and then by the string in code-unit order, as JavaScript's `<` compares two strings.
                variants.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                if variants.len() > 1 {
                    map.ambiguous.push(gid);
                }
                map.text.insert(gid, variants[0].0.clone());
                map.cid_to_text.insert(gid, variants.into_iter().map(|(t, _)| Utf16(t)).collect());
            }
            (key, map)
        })
        .collect())
}

/// The glyph set and first-seen texts of every instance, walking the runs as the writer shows them.
fn usages(list: &DisplayList) -> BTreeMap<&str, Usage> {
    let mut out: BTreeMap<&str, Usage> = BTreeMap::new();
    let space: [u16; 1] = [0x20];
    for page in &list.pages {
        for item in &page.items {
            let Item::Text(text) = item else { continue };
            let usage = out.entry(text.font.as_str()).or_default();
            let space_gid = text.space.0 as u16;
            if text.leading_space {
                usage.record(space_gid, &space);
            }
            for (index, segment) in segments(text).iter().enumerate() {
                if index > 0 && segment.space_before {
                    usage.record(space_gid, &space);
                }
                for (j, &gid) in segment.gids.iter().enumerate() {
                    usage.record(gid, segment.text_for(j));
                }
            }
        }
    }
    out
}

/// The six-letter subset tag, the oracle's `subsetTagFor`: FNV-1a-like over 64 bits as two 32-bit halves, of the face, the coordinates and the glyph set.
pub fn subset_tag(face_id: &str, coords: &BTreeMap<String, Num>, gids: &[u16]) -> String {
    let joined: Vec<String> = gids.iter().map(u16::to_string).collect();
    let source: Vec<u16> = format!("{face_id}|{}|{}", coords_key(coords), joined.join(",")).encode_utf16().collect();
    let (mut hi, mut lo): (u32, u32) = (0x811c_9dc5, 0x0100_0193);
    for (i, &c) in source.iter().enumerate() {
        hi = (hi ^ u32::from(c)).wrapping_mul(0x0100_0193);
        lo = ((u64::from(lo) + u64::from(c) * (i as u64 + 1)) as u32).wrapping_mul(0x85eb_ca6b);
    }
    let mut value = u64::from(hi ^ lo);
    let mut tag = String::new();
    for i in 0..6 {
        tag.push(char::from(b'A' + (value % 26) as u8));
        value = value / 26 + if i == 2 { u64::from(hi % 26) } else { 0 };
    }
    tag
}

/// `coordsKey`: `opsz=12,wght=400`, the tags in order and each value at three decimals.
fn coords_key(coords: &BTreeMap<String, Num>) -> String {
    coords.iter().map(|(tag, v)| format!("{tag}={}", js_string(js_round(v.0 * 1000.0) / 1000.0))).collect::<Vec<_>>().join(",")
}

/// `JSON.stringify(coords)`, the tags in order.
fn coords_json(coords: &BTreeMap<String, Num>) -> String {
    let entries: Vec<String> = coords.iter().map(|(tag, v)| format!("{}:{}", json_string(&tag.encode_utf16().collect::<Vec<_>>()), js_string(v.0))).collect();
    format!("{{{}}}", entries.join(","))
}

/// The list's width of a glyph of an instance, in font units, as an `hmtx` advance.
fn list_width(instance: &FontInstance, gid: u16) -> Result<u16, Refusal> {
    let w = &instance.widths;
    let at = w.gids.iter().position(|g| g.0 == f64::from(gid)).ok_or_else(|| Refusal::new(format!("caster: the display list holds no width for glyph {gid} of {}; the builder writes one for every glyph a text item shows and every space glyph it names", instance.key)))?;
    let units = w.units[at].0;
    if units.fract() != 0.0 || !(0.0..=65535.0).contains(&units) {
        return Err(Refusal::new(format!("forme: {} glyph {gid} has the width {}, which an hmtx advance cannot carry", instance.key, js_string(units))));
    }
    Ok(units as u16)
}

/// The tables of an SFNT, in directory order.
fn sfnt_tables(bytes: &[u8]) -> Option<Vec<([u8; 4], Vec<u8>)>> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    (0..u16_at(4)? as usize)
        .map(|i| {
            let at = 12 + i * 16;
            let tag: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
            let (offset, length) = (u32_at(at + 8)? as usize, u32_at(at + 12)? as usize);
            Some((tag, bytes.get(offset..offset + length)?.to_vec()))
        })
        .collect()
}

fn table<'a>(tables: &'a [([u8; 4], Vec<u8>)], tag: &[u8; 4]) -> Option<&'a [u8]> {
    tables.iter().find(|(t, _)| t == tag).map(|(_, data)| data.as_slice())
}

/// The OpenType checksum: the table as big-endian words, zero-padded, summed with wrapping.
fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, word| {
        let mut w = [0u8; 4];
        w[..word.len()].copy_from_slice(word);
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

/// An SFNT from its tables: sorted by tag, each four-byte aligned, the directory's search fields and checksums computed, and `head.checkSumAdjustment` set so the whole file sums to `0xB1B0AFBA`.
fn build_sfnt(version: &[u8], mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by_key(|(tag, _)| *tag);
    if let Some((_, head)) = tables.iter_mut().find(|(t, _)| t == b"head")
        && head.len() >= 12
    {
        head[8..12].fill(0);
    }
    let n = tables.len() as u16;
    let selector = 15 - n.max(1).leading_zeros() as u16;
    let range = 16u16 << selector;
    let mut out = Vec::new();
    out.extend_from_slice(&version[..4]);
    for v in [n, range, selector, n * 16 - range] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut offset = 12 + 16 * tables.len();
    for (tag, data) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += data.len().next_multiple_of(4);
    }
    let mut head_at = None;
    for (tag, data) in &tables {
        if tag == b"head" {
            head_at = Some(out.len());
        }
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    if let Some(at) = head_at {
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[at + 8..at + 12].copy_from_slice(&adjustment.to_be_bytes());
    }
    out
}

/// The advance and left side bearing of every glyph of a `hmtx`, read with the two-part rule: glyphs from `numberOfHMetrics` on take the last long advance and a bearing from the trailing array.
fn metrics(hhea: &[u8], hmtx: &[u8], num_glyphs: u16) -> Option<Vec<(u16, i16)>> {
    let long = usize::from(u16::from_be_bytes(hhea.get(34..36)?.try_into().ok()?));
    let u16_at = |at: usize| hmtx.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    (0..usize::from(num_glyphs))
        .map(|g| {
            let advance = u16_at(g.min(long.checked_sub(1)?) * 4)?;
            let lsb = if g < long { u16_at(g * 4 + 2)? } else { u16_at(long * 4 + (g - long) * 2)? };
            Some((advance, lsb as i16))
        })
        .collect()
}

/// Cut one instance's program over a glyph set: the subset at the instance's coordinates over a sorted remapper, the hinting tables left out, the `hmtx` patched to the list's widths where subsetter's differ, and every glyph of the set read back through skrifa.
///
/// # Errors
/// A coordinate that is not an axis tag, a face subsetter cannot cut, a width an `hmtx` cannot carry, a variable table left in the program, and a glyph whose read-back advance is not the list's, in the oracle's sentence.
pub fn program(instance: &FontInstance, source: &[u8], gids: &[u16]) -> Result<Program, Refusal> {
    let id = &instance.key;
    let mut coords = Vec::new();
    for (tag, value) in &instance.coords {
        let tag = Tag::from_str(tag).map_err(|_| Refusal::new(format!("forme: {id} is pinned on the axis {tag:?}, which is not a four-letter axis tag")))?;
        coords.push((tag, value.0 as f32));
    }
    let remapper = GlyphRemapper::new_from_glyphs_sorted(gids);
    let cut = subsetter::subset_with_variations(source, 0, &coords, &remapper).map_err(|e| Refusal::new(format!("forme: {id} could not be subset: {e}")))?;
    let unreadable = || Refusal::new(format!("forme: the subset of {id} is not a font program skrifa reads"));
    let mut tables = sfnt_tables(&cut).ok_or_else(unreadable)?;
    if let Some((tag, _)) = tables.iter().find(|(t, _)| VARIABLE_TABLES.contains(&t)) {
        return Err(Refusal::new(format!("forme: {id} still carries a {} table after pinning {}; PDF 2.0 has no variable font program", String::from_utf8_lossy(tag), coords_json(&instance.coords))));
    }
    let hinted = tables.len();
    tables.retain(|(t, _)| !HINTING_TABLES.contains(&t));
    let hinted = hinted != tables.len();

    let cids: BTreeMap<u16, u16> = gids.iter().map(|&g| (g, remapper.get(g).expect("every glyph of the set is remapped"))).collect();
    let wanted: BTreeMap<u16, u16> = cids.iter().map(|(&g, &cid)| list_width(instance, g).map(|w| (cid, w))).collect::<Result<_, _>>()?;
    let num_glyphs = table(&tables, b"maxp").and_then(|m| m.get(4..6)).map(|b| u16::from_be_bytes([b[0], b[1]])).ok_or_else(unreadable)?;
    let old = metrics(table(&tables, b"hhea").ok_or_else(unreadable)?, table(&tables, b"hmtx").ok_or_else(unreadable)?, num_glyphs).ok_or_else(unreadable)?;
    let patched: Vec<u16> = cids.iter().filter(|&(_, cid)| old.get(usize::from(*cid)).is_some_and(|m| m.0 != wanted[cid])).map(|(&g, _)| g).collect();
    let bytes = if patched.is_empty() && !hinted {
        cut
    } else {
        if !patched.is_empty() {
            // Every glyph a long metric, so a run of trailing equal advances needs no case of its own.
            let hmtx: Vec<u8> = old.iter().enumerate().flat_map(|(g, &(advance, lsb))| {
                let advance = wanted.get(&(g as u16)).copied().unwrap_or(advance);
                [advance.to_be_bytes(), lsb.to_be_bytes()].concat()
            }).collect();
            for (tag, data) in tables.iter_mut() {
                match &*tag {
                    b"hmtx" => *data = hmtx.clone(),
                    b"hhea" => data[34..36].copy_from_slice(&num_glyphs.to_be_bytes()),
                    _ => {}
                }
            }
        }
        build_sfnt(&cut, tables)
    };

    let font = FontRef::new(&bytes).map_err(|_| unreadable())?;
    let hmtx = font.hmtx().map_err(|_| unreadable())?;
    for (&gid, &cid) in &cids {
        let have = hmtx.advance(GlyphId::new(u32::from(cid))).unwrap_or(0);
        let want = wanted[&cid];
        if have != want {
            let pinned = if instance.coords.is_empty() { String::new() } else { format!(" after pinning {}", coords_json(&instance.coords)) };
            return Err(Refusal::new(format!("caster: {id} glyph {gid}: layout used an advance of {want} but the embedded program's hmtx carries {have}{pinned}")));
        }
    }
    Ok(Program { bytes, cids, patched })
}

/// A pen that counts what a glyph draws and the box of every point it passes, off-curve points included, which is the box a `glyf` header carries.
#[derive(Default)]
struct Bounds {
    points: usize,
    min: (f32, f32),
    max: (f32, f32),
}

impl Bounds {
    fn point(&mut self, x: f32, y: f32) {
        if self.points == 0 {
            (self.min, self.max) = ((x, y), (x, y));
        }
        self.points += 1;
        self.min = (self.min.0.min(x), self.min.1.min(y));
        self.max = (self.max.0.max(x), self.max.1.max(y));
    }
}

impl OutlinePen for Bounds {
    fn move_to(&mut self, x: f32, y: f32) {
        self.point(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.point(x, y);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.point(cx0, cy0);
        self.point(x, y);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.point(cx0, cy0);
        self.point(cx1, cy1);
        self.point(x, y);
    }
    fn close(&mut self) {}
}

/// What a glyph draws, unhinted, in font units, at a location.
fn drawn(font: &FontRef, gid: u32, location: LocationRef) -> Bounds {
    let mut pen = Bounds::default();
    if let Some(glyph) = font.outline_glyphs().get(GlyphId::new(gid)) {
        let _ = glyph.draw(DrawSettings::unhinted(Size::unscaled(), location), &mut pen);
    }
    pen
}

/// fontkit's reading of `OS/2.fsType`, as the oracle's `fsTypeText` writes it.
fn fs_type_text(font: &FontRef) -> String {
    let Ok(os2) = font.os2() else { return "no OS/2 table".into() };
    let bits = [(1, "noEmbedding"), (2, "viewOnly"), (3, "editable"), (8, "noSubsetting"), (9, "bitmapOnly")];
    let set: Vec<&str> = bits.iter().filter(|(bit, _)| os2.fs_type() & (1 << bit) != 0).map(|&(_, name)| name).collect();
    if set.is_empty() { "installable".into() } else { set.join("+") }
}

/// Four uppercase hex digits of one code unit or CID.
fn hex4(unit: u16) -> String {
    format!("{unit:04X}")
}

/// The ToUnicode CMap: one `bfchar` entry per shown glyph that stands for text, in blocks of at most [`BFCHAR_BLOCK`].
fn to_unicode_cmap(entries: &[(u16, &[u16])]) -> String {
    let lines: Vec<String> = entries.iter().filter(|(_, text)| !text.is_empty()).map(|(cid, text)| format!("<{}> <{}>", hex4(*cid), text.iter().map(|&u| hex4(u)).collect::<String>())).collect();
    let blocks: Vec<String> = lines.chunks(BFCHAR_BLOCK).map(|block| format!("{} beginbfchar\n{}\nendbfchar", block.len(), block.join("\n"))).collect();
    format!(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo <</Registry (Adobe) /Ordering (UCS) /Supplement 0>> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n{}\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n",
        blocks.join("\n")
    )
}

/// Flate at level 6, the one compressor of every stream.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(data).expect("writing to a vector cannot fail");
    encoder.finish().expect("writing to a vector cannot fail")
}

/// The width that repeats most, the widest of equally common ones; `/DW` covers every CID `/W` does not.
fn most_common(widths: &[f64]) -> f64 {
    let mut counts: Vec<(f64, usize)> = Vec::new();
    for &w in widths {
        match counts.iter_mut().find(|(v, _)| *v == w) {
            Some((_, n)) => *n += 1,
            None => counts.push((w, 1)),
        }
    }
    counts.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut best = (PDF_GLYPH_UNITS, 0);
    for (value, count) in counts {
        if count > best.1 {
            best = (value, count);
        }
    }
    best.0
}

/// A name with no space or delimiter, as a PDF name and a PostScript name want it.
fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_whitespace() && !"()<>[]{}/%#".contains(*c)).collect()
}

/// What the page loop needs of one written instance: the Type 0 font a page names, and how a glyph of the list is shown in it.
pub struct Embedded {
    /// The Type 0 font.
    pub font: Ref,
    /// The list's glyph id to its CID.
    pub cids: BTreeMap<u16, u16>,
    /// The `/W` entry of each glyph of the set, by the list's glyph id, in thousandths of an em at the profile's decimals: what a viewer advances by, so what the pen is tracked by.
    pub widths: BTreeMap<u16, f64>,
    /// The text the ToUnicode map gives each glyph that stands for any, by the list's glyph id; an occurrence that stands for other text is wrapped in `/ActualText`.
    pub chosen: BTreeMap<u16, Vec<u16>>,
}

/// Every instance of `resources.fonts`, in that order, written into the file: the objects of each, and the Type 0 font a page names as `F0`, `F1`, ….
///
/// # Errors
/// The oracle's refusals of the ToUnicode map, first, over the whole document; then, instance by instance, every refusal of [`program`], a glyph the subset left with no outline that the face draws, and a list asking for whole fonts, which `print()` never does.
pub fn write_fonts(pdf: &mut Pdf, next: &mut Ref, list: &DisplayList, blobs: &Blobs) -> Result<(Vec<Embedded>, Vec<FaceRecord>), Refusal> {
    let profile = &list.writing.font_profile;
    if !profile.subset && !list.resources.fonts.is_empty() {
        return Err(Refusal::new("forme: the display list asks for whole fonts (writing.fontProfile.subset is false), which forme does not write; print() always subsets".into()));
    }
    let maps = to_unicode(list)?;
    let mut usages = usages(list);
    let (mut refs, mut records) = (Vec::new(), Vec::new());
    for instance in &list.resources.fonts {
        let id = &instance.key;
        let source_bytes = &blobs[&instance.blob];
        let usage = usages.remove(id.as_str()).unwrap_or_default();
        let gids: Vec<u16> = usage.used.iter().copied().collect();
        let map = maps.get(id.as_str());
        let empty: Vec<u16> = Vec::new();
        let text_of = |gid: u16| -> &[u16] { map.and_then(|m| m.text.get(&gid)).or_else(|| usage.first_seen.get(&gid)).unwrap_or(&empty) };

        let program = program(instance, source_bytes, &gids)?;
        let source = FontRef::new(source_bytes).map_err(|_| Refusal::new(format!("forme: {id} is not a font program skrifa reads")))?;
        let font = FontRef::new(&program.bytes).map_err(|_| Refusal::new(format!("forme: the subset of {id} is not a font program skrifa reads")))?;
        let unreadable = |what: &str| Refusal::new(format!("forme: {id} has no {what} table"));
        let pinned = !instance.coords.is_empty();
        let location: Location = source.axes().location(instance.coords.iter().map(|(tag, v)| (skrifa::Tag::new_checked(tag.as_bytes()).unwrap_or_default(), v.0 as f32)));
        let at_instance = if pinned { LocationRef::from(&location) } else { LocationRef::default() };

        // A glyph the face draws and the subset cannot, which is the defect; a face's own empty glyph is its design (*a face's own empty glyph was read as a dropped one*, Caster's DESIGN.md S45).
        let missing: Vec<String> = program.cids.iter().filter(|&(&gid, &cid)| {
            let shows = !String::from_utf16_lossy(text_of(gid)).trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}').is_empty();
            shows && drawn(&font, u32::from(cid), LocationRef::default()).points == 0 && drawn(&source, u32::from(gid), at_instance).points > 0
        }).map(|(gid, _)| gid.to_string()).collect();
        if !missing.is_empty() {
            return Err(Refusal::new(format!("caster: {id} shows glyph(s) {} that the subset left with no outline, and the face itself draws them", missing.join(","))));
        }

        let tables = sfnt_tables(&program.bytes).unwrap_or_default();
        let glyf = table(&tables, b"glyf").is_some();
        let cff = table(&tables, b"CFF ");
        let (outline_type, charset_identity) = match cff.map(|data| CffFontRef::new_cff(data, 0, None)) {
            None => ("glyf", None),
            Some(Ok(cff)) if cff.is_cid() => {
                let charset = cff.charset();
                let identity = (0..cff.num_glyphs()).all(|g| charset.as_ref().and_then(|c| c.string_id(GlyphId::new(g)).ok()).is_some_and(|sid| u32::from(sid.to_u16()) == g));
                ("cff-cid", Some(identity))
            }
            Some(Ok(_)) => ("cff", None),
            Some(Err(_)) => return Err(Refusal::new(format!("forme: the CFF of the subset of {id} is not one read-fonts reads"))),
        };
        let num_glyphs = font.maxp().map_err(|_| unreadable("maxp"))?.num_glyphs();
        let glyphs: Vec<Bounds> = (0..u32::from(num_glyphs)).map(|g| drawn(&font, g, LocationRef::default())).collect();
        let non_empty_glyphs = glyphs.iter().filter(|b| b.points > 0).count();

        let head = source.head().map_err(|_| unreadable("head"))?;
        let upem = head.units_per_em();
        let scale = PDF_GLYPH_UNITS / f64::from(upem);
        let round = |v: f64| js_round(v * scale) as f32;
        let factor = 10f64.powf(profile.width_decimals.0);
        let width = |gid: u16| -> f64 { list_width(instance, gid).map_or(0.0, |u| js_round(f64::from(u) * scale * factor) / factor) };
        let subset_tag = subset_tag(&instance.face_id, &instance.coords, &gids);
        let post_script_name = source.localized_strings(StringId::POSTSCRIPT_NAME).english_or_first().map(|s| s.to_string()).unwrap_or_default();
        let ps_name = format!("{subset_tag}+{}", clean_name(&post_script_name));

        // The box is the instance's own where the face is pinned: the union of what the subset's glyphs draw, `.notdef` aside, as HarfBuzz recomputes the instanced program's `head` (its subset keeps the glyphs its layout closure reaches too, so its box can be wider, measured 2026-11-02 on 18 of the corpus's 36 instances). A static face's is its `head`'s, the oracle's.
        let bbox = if pinned {
            let drawn: Vec<&Bounds> = glyphs.iter().skip(1).filter(|b| b.points > 0).collect();
            let fold = |pick: fn(&Bounds) -> f32, min: bool| drawn.iter().map(|b| pick(b)).reduce(if min { f32::min } else { f32::max }).unwrap_or(0.0);
            [fold(|b| b.min.0, true), fold(|b| b.min.1, true), fold(|b| b.max.0, false), fold(|b| b.max.1, false)].map(f64::from)
        } else {
            [head.x_min(), head.y_min(), head.x_max(), head.y_max()].map(f64::from)
        };
        let os2 = source.os2().ok();
        let italic_angle = source.post().map_err(|_| unreadable("post"))?.italic_angle().to_f64();
        let selection = os2.as_ref().map_or(0, |o| o.fs_selection().bits());
        let serif = os2.as_ref().is_some_and(|o| (1..=7).contains(&(o.s_family_class() >> 8)));
        let mut flags = FontFlags::SYMBOLIC;
        flags.set(FontFlags::SERIF, serif);
        flags.set(FontFlags::ITALIC, italic_angle != 0.0 || selection & 1 != 0);
        let hhea = source.hhea().map_err(|_| unreadable("hhea"))?;
        let (ascent, descent) = match &os2 {
            Some(o) if selection & (1 << 7) != 0 => (f64::from(o.s_typo_ascender()), f64::from(o.s_typo_descender())),
            _ => (f64::from(hhea.ascender().to_i16()), f64::from(hhea.descender().to_i16())),
        };
        let cap_height = match os2.as_ref().and_then(|o| o.s_cap_height()).filter(|&c| c > 0) {
            Some(c) => f64::from(c),
            // The top of `H`, which is what it means; fontkit maps a missing `H` to `.notdef`.
            None => f64::from(drawn(&source, source.charmap().map('H').map_or(0, |g| g.to_u32()), at_instance).max.1),
        };

        let opentype = profile.cff_container == CffContainer::Opentype;
        let stream_bytes = if glyf || opentype { program.bytes.as_slice() } else { cff.unwrap_or(&program.bytes) };
        let compressed = deflate(stream_bytes);
        let cid_widths: Vec<(u16, f64)> = program.cids.iter().map(|(&g, &cid)| (cid, width(g))).collect();
        let default_width = most_common(&cid_widths.iter().map(|&(_, w)| w).collect::<Vec<_>>());
        let entries: Vec<(u16, &[u16])> = program.cids.iter().map(|(&g, &cid)| (cid, text_of(g))).collect();
        let cmap = deflate(to_unicode_cmap(&entries).as_bytes());

        let [type0, cid_font, descriptor, file, to_unicode_ref] = [(); 5].map(|_| next.bump());
        pdf.type0_font(type0).base_font(Name(ps_name.as_bytes())).encoding_predefined(Name(b"Identity-H")).descendant_font(cid_font).to_unicode(to_unicode_ref);
        let mut cid = pdf.cid_font(cid_font);
        cid.subtype(if glyf { CidFontType::Type2 } else { CidFontType::Type0 });
        cid.base_font(Name(ps_name.as_bytes()));
        cid.system_info(SystemInfo { registry: Str(b"Adobe"), ordering: Str(b"Identity"), supplement: 0 });
        cid.font_descriptor(descriptor);
        cid.default_width(default_width as f32);
        {
            let mut w = cid.widths();
            let mut run_start = 0usize;
            for i in 1..=cid_widths.len() {
                if i == cid_widths.len() || cid_widths[i].0 != cid_widths[i - 1].0 + 1 {
                    w.consecutive(cid_widths[run_start].0, cid_widths[run_start..i].iter().map(|&(_, v)| v as f32));
                    run_start = i;
                }
            }
        }
        if glyf {
            cid.cid_to_gid_map_predefined(Name(b"Identity"));
        }
        cid.finish();
        let mut d = pdf.font_descriptor(descriptor);
        d.name(Name(ps_name.as_bytes()))
            .flags(flags)
            .bbox(Rect::new(round(bbox[0]), round(bbox[1]), round(bbox[2]), round(bbox[3])))
            .italic_angle(italic_angle as f32)
            .ascent(round(ascent))
            .descent(round(descent))
            .cap_height(round(cap_height))
            .stem_v(STEM_V);
        if glyf { d.font_file2(file) } else { d.font_file3(file) };
        d.finish();
        let mut stream = pdf.stream(file, &compressed);
        stream.filter(Filter::FlateDecode);
        if glyf {
            stream.pair(Name(b"Length1"), stream_bytes.len() as i32);
        } else {
            stream.pair(Name(b"Subtype"), Name(if opentype { b"OpenType".as_slice() } else { b"CIDFontType0C".as_slice() }));
        }
        stream.finish();
        pdf.stream(to_unicode_ref, &cmap).filter(Filter::FlateDecode);

        records.push(FaceRecord {
            id: id.clone(),
            post_script_name,
            used_glyphs: gids.len(),
            font_file_bytes: stream_bytes.len(),
            multi_char_to_unicode: gids.iter().filter(|&&g| text_of(g).len() > 1).count(),
            to_unicode_conflicts: usage.conflicts,
            face_id: instance.face_id.clone(),
            family: instance.family.clone(),
            source_format: instance.source_format,
            outline_type,
            upem,
            coords: instance.coords.clone(),
            glyphs_used: gids.clone(),
            source_bytes: source_bytes.len(),
            subset_bytes: stream_bytes.len(),
            subset_flate_bytes: compressed.len(),
            num_glyphs,
            non_empty_glyphs,
            hmtx_instanced: true,
            stale_advances: 0,
            subset_tag,
            cff_container: if glyf { CffContainer::Bare } else { profile.cff_container },
            cid_to_text: gids.iter().map(|&g| (g, map.and_then(|m| m.cid_to_text.get(&g).cloned()).unwrap_or_else(|| vec![Utf16(text_of(g).to_vec())]))).collect(),
            ambiguous_cids: map.map(|m| m.ambiguous.clone()).unwrap_or_default(),
            tables: if glyf || opentype { tables.iter().map(|(t, _)| String::from_utf8_lossy(t).into_owned()).collect() } else { vec!["CFF ".into()] },
            charset_identity,
            charset_retried: false,
            fs_type: fs_type_text(&source),
            path: "B",
            ms: Timing::default(),
        });
        refs.push(Embedded { font: type0, widths: program.cids.keys().map(|&g| (g, width(g))).collect(), cids: program.cids, chosen: map.map(|m| m.text.clone()).unwrap_or_default() });
    }
    Ok((refs, records))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subset_tag_is_the_oracles() {
        // Caster's `subsetTagFor` in node, 2026-11-02, for the first instance of the committed cast list.
        let coords = BTreeMap::from([("wght".to_string(), Num(700.0))]);
        assert_eq!(subset_tag("Source Sans 3#1", &coords, &[1, 2, 4, 15, 19, 32, 34, 36, 40, 41, 42, 43, 46, 47]), "LTYHKD");
    }

    #[test]
    fn a_rebuilt_sfnt_sums_to_the_magic_and_its_tables_to_their_checksums() {
        let head = vec![0u8; 54];
        let bytes = build_sfnt(&[0, 1, 0, 0], vec![(*b"maxp", vec![0, 0, 0x50, 0, 0, 3]), (*b"head", head)]);
        assert_eq!(checksum(&bytes), 0xB1B0_AFBA);
        let tables = sfnt_tables(&bytes).unwrap();
        assert_eq!(tables.iter().map(|(t, _)| *t).collect::<Vec<_>>(), [*b"head", *b"maxp"]);
        assert_eq!(&bytes[4..12], &[0, 2, 0, 32, 0, 1, 0, 0]);
    }
}
