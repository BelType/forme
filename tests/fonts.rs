//! The fonts reader: every instance of every committed list, read back out of the file forme writes by a PDF parser that is not pdf-writer (hayro-syntax) and a font parser (skrifa, read-fonts), held to the list and, where the oracle printed the list, to the oracle's file and its report snapshot.

use std::collections::BTreeMap;
use std::path::PathBuf;

use forme::content::js_round;
use forme::fonts::{FaceRecord, program};
use forme::list::{self, DisplayList, FontInstance, Num};
use forme::{read_blobs, write};
use hayro_syntax::Pdf;
use hayro_syntax::object::{Array, Dict, Name, Object, Stream};
use serde_json::Value;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::TableProvider;
use skrifa::raw::ps::cff::CffFontRef;
use skrifa::{FontRef, GlyphId, MetadataProvider, Tag};

fn lists() -> PathBuf {
    let caster = std::env::var_os("CASTER").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../caster"));
    caster.join("fixtures/lists")
}

fn corpus() -> Vec<(String, bool)> {
    let corpus: Value = serde_json::from_str(&std::fs::read_to_string(lists().join("corpus.json")).unwrap()).unwrap();
    corpus["documents"].as_array().unwrap().iter().map(|d| (d["name"].as_str().unwrap().to_string(), d.get("refused").is_some())).collect()
}

fn list_of(name: &str) -> (DisplayList, list::Blobs) {
    let list = list::parse(&std::fs::read_to_string(lists().join(format!("{name}.json"))).unwrap()).unwrap();
    let blobs = read_blobs(&list, &lists().join("blobs")).unwrap();
    (list, blobs)
}

/// The Type 0 fonts page 1 names, `F0`, `F1`, ….
fn page_fonts<'a>(pdf: &'a Pdf) -> Dict<'a> {
    pdf.pages()[0].raw().get::<Dict>(b"Resources").unwrap().get::<Dict>(b"Font").unwrap()
}

/// `/W` as CID to width.
fn widths_of(cid_font: &Dict) -> BTreeMap<u32, f32> {
    let mut out = BTreeMap::new();
    let w = cid_font.get::<Array>(b"W").unwrap();
    let mut items = w.iter::<Object>();
    while let Some(Object::Number(start)) = items.next() {
        match items.next() {
            Some(Object::Array(run)) => {
                for (i, v) in run.iter::<f32>().enumerate() {
                    out.insert(start.as_i64() as u32 + i as u32, v);
                }
            }
            other => panic!("a /W run is a start and an array, found {other:?}"),
        }
    }
    out
}

/// The ToUnicode CMap's `bfchar` entries as CID to UTF-16, and how many blocks hold them.
fn to_unicode(type0: &Dict) -> (BTreeMap<u32, Vec<u16>>, Vec<usize>) {
    let data = type0.get::<Stream>(b"ToUnicode").unwrap().decoded().unwrap().into_owned();
    let text = String::from_utf8(data).unwrap();
    let mut map = BTreeMap::new();
    let mut blocks = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if let Some(count) = line.strip_suffix(" beginbfchar") {
            blocks.push(count.parse().unwrap());
            inside = true;
        }
        inside &= line != "endbfchar";
        if !inside {
            continue;
        }
        if let Some((cid, dst)) = line.strip_prefix('<').and_then(|l| l.strip_suffix('>')).and_then(|l| l.split_once("> <")) {
            if cid.len() != 4 {
                continue;
            }
            let units = (0..dst.len()).step_by(4).map(|i| u16::from_str_radix(&dst[i..i + 4], 16).unwrap()).collect();
            map.insert(u32::from_str_radix(cid, 16).unwrap(), units);
        }
    }
    (map, blocks)
}

/// A drawn glyph's box and point count, off-curve points included.
#[derive(Default, Debug)]
struct Pen(usize, [f32; 4]);

impl Pen {
    fn at(&mut self, x: f32, y: f32) {
        if self.0 == 0 {
            self.1 = [x, y, x, y];
        }
        self.0 += 1;
        self.1 = [self.1[0].min(x), self.1[1].min(y), self.1[2].max(x), self.1[3].max(y)];
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.at(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.at(x, y);
    }
    fn quad_to(&mut self, a: f32, b: f32, x: f32, y: f32) {
        self.at(a, b);
        self.at(x, y);
    }
    fn curve_to(&mut self, a: f32, b: f32, c: f32, d: f32, x: f32, y: f32) {
        self.at(a, b);
        self.at(c, d);
        self.at(x, y);
    }
    fn close(&mut self) {}
}

fn draw(font: &FontRef, gid: u32, location: LocationRef) -> Pen {
    let mut pen = Pen::default();
    if let Some(glyph) = font.outline_glyphs().get(GlyphId::new(gid)) {
        glyph.draw(DrawSettings::unhinted(Size::unscaled(), location), &mut pen).unwrap();
    }
    pen
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, word| {
        let mut w = [0u8; 4];
        w[..word.len()].copy_from_slice(word);
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

/// The list's width of a glyph, in font units.
fn width(instance: &FontInstance, gid: u16) -> u16 {
    let at = instance.widths.gids.iter().position(|g| g.0 == f64::from(gid)).unwrap();
    instance.widths.units[at].0 as u16
}

/// The plan's rule: a glyph's CID is its place in the subset, `.notdef` first and then the set ascending, so the k-th glyph of a set without `.notdef` is CID k + 1; a set holding `.notdef` itself (a space the face has no glyph for) keeps it at CID 0.
fn cid_of(gids: &[u16], gid: u16) -> u32 {
    let at = gids.iter().position(|&g| g == gid).unwrap() as u32;
    if gids.first() == Some(&0) { at } else { at + 1 }
}

/// The members of a face record the two writers share by rule: every member but the program's sizes, its glyph count and outlines, its table list and the timings.
const SHARED: [&str; 21] = ["id", "postScriptName", "usedGlyphs", "multiCharToUnicode", "toUnicodeConflicts", "faceId", "family", "sourceFormat", "upem", "coords", "glyphsUsed", "sourceBytes", "hmtxInstanced", "staleAdvances", "subsetTag", "cffContainer", "cidToText", "ambiguousCids", "charsetRetried", "fsType", "path"];

#[derive(Default, Debug)]
struct Tally {
    lists: usize,
    instances: usize,
    glyphs: usize,
    widths_checked: usize,
    patches: usize,
    tags_equal: usize,
    descriptors_equal: usize,
    bboxes_equal: usize,
    bboxes_inside: Vec<String>,
    to_unicode_equal: usize,
    w_equal: usize,
    cff: usize,
    cff2: usize,
    max_outline_box_difference: f32,
}

#[test]
fn every_instance_of_every_list_reads_back_as_the_list_says_and_as_the_oracle_wrote_it() {
    let mut tally = Tally::default();
    for (name, refused) in corpus() {
        tally.lists += 1;
        let (list, blobs) = list_of(&name);
        let written = write(&list, &blobs).unwrap_or_else(|r| panic!("{name}: {r}"));
        let pdf = Pdf::new(written.bytes.clone()).unwrap_or_else(|e| panic!("{name}: hayro-syntax cannot open forme's file: {e:?}"));
        let fonts = page_fonts(&pdf);
        let oracle_pdf = (!refused).then(|| Pdf::new(std::fs::read(lists().join(format!("{name}.pdf"))).unwrap()).unwrap());
        let oracle_fonts = oracle_pdf.as_ref().map(page_fonts);
        let report: Option<Value> = (!refused).then(|| serde_json::from_str(&std::fs::read_to_string(lists().join(format!("{name}.report.json"))).unwrap()).unwrap());
        assert_eq!(written.records.faces.len(), list.resources.fonts.len(), "{name}: one record per instance");
        let records: Value = serde_json::from_str(&forme::json::records(&written.records)).unwrap_or_else(|e| panic!("{name}: the records are not JSON: {e}"));
        for (i, instance) in list.resources.fonts.iter().enumerate() {
            let at = format!("{name} {}", instance.key);
            let record: &FaceRecord = &written.records.faces[i];
            let gids = &record.glyphs_used;
            tally.instances += 1;
            tally.glyphs += gids.len();
            let key = format!("F{i}");
            let type0 = fonts.get::<Dict>(key.as_bytes()).unwrap_or_else(|| panic!("{at}: page 1 names no /{key}"));
            assert_eq!(&*type0.get::<Name>(b"Subtype").unwrap(), b"Type0", "{at}");
            assert_eq!(&*type0.get::<Name>(b"Encoding").unwrap(), b"Identity-H", "{at}");
            let cid_font = type0.get::<Array>(b"DescendantFonts").unwrap().iter::<Dict>().next().unwrap();
            let descriptor = cid_font.get::<Dict>(b"FontDescriptor").unwrap();
            let glyf = &*cid_font.get::<Name>(b"Subtype").unwrap() == b"CIDFontType2";
            assert_eq!(glyf, record.outline_type == "glyf", "{at}: the subtype is the outline's");
            assert_eq!(glyf, cid_font.get::<Name>(b"CIDToGIDMap").is_some_and(|n| &*n == b"Identity"), "{at}: /CIDToGIDMap /Identity on a CIDFontType2 alone");

            // The program in the file, and the SFNT forme cut it from, which a bare CFF is the `CFF ` table of.
            let file = descriptor.get::<Stream>(if glyf { b"FontFile2".as_slice() } else { b"FontFile3".as_slice() }).unwrap_or_else(|| panic!("{at}: no font file"));
            let in_file = file.decoded().unwrap().into_owned();
            let source = &blobs[&instance.blob];
            let cut = program(instance, source, gids).unwrap();
            tally.patches += cut.patched.len();
            let sfnt = FontRef::new(&cut.bytes).unwrap();
            if glyf {
                assert_eq!(in_file, cut.bytes, "{at}: the FontFile2 is the program");
                assert_eq!(file.dict().get::<i64>(b"Length1"), Some(in_file.len() as i64), "{at}: /Length1");
            } else {
                assert_eq!(&*file.dict().get::<Name>(b"Subtype").unwrap(), b"CIDFontType0C", "{at}");
                assert_eq!(in_file, sfnt.table_data(Tag::new(b"CFF ")).unwrap().as_bytes(), "{at}: the bare CFF is the program's CFF table");
                let cff = CffFontRef::new_cff(&in_file, 0, None).unwrap_or_else(|e| panic!("{at}: read-fonts cannot read the bare CFF: {e:?}"));
                assert!(cff.is_cid(), "{at}: the CFF is CID-keyed");
                let charset = cff.charset().unwrap();
                assert!((0..cff.num_glyphs()).all(|g| u32::from(charset.string_id(GlyphId::new(g)).unwrap().to_u16()) == g), "{at}: the charset is the identity, so CID = glyph");
                tally.cff += 1;
            }
            if FontRef::new(source).unwrap().table_data(Tag::new(b"CFF2")).is_some() {
                assert_eq!(record.outline_type, "glyf", "{at}: a CFF2 face is instanced to glyf");
                tally.cff2 += 1;
            }
            let tags: Vec<[u8; 4]> = sfnt.table_directory.table_records().iter().map(|r| r.tag().to_be_bytes()).collect();
            for forbidden in [b"fvar", b"gvar", b"avar", b"HVAR", b"MVAR", b"CFF2", b"cvt ", b"fpgm", b"prep"] {
                assert!(!tags.contains(forbidden), "{at}: the program carries {}", String::from_utf8_lossy(forbidden));
            }
            let sum = checksum(&cut.bytes);
            assert_eq!(sum, 0xB1B0_AFBA, "{at}: the whole program sums to the magic");

            // The CID rule: glyph k + 1 of the program is the k-th glyph of the set, drawn as the source draws it at the instance; its hmtx is the list's width.
            let hmtx = sfnt.hmtx().unwrap();
            assert!(gids.is_empty() || sfnt.maxp().unwrap().num_glyphs() as u32 > cid_of(gids, *gids.last().unwrap_or(&0)), "{at}: .notdef and the set, at least");
            let location = FontRef::new(source).unwrap().axes().location(instance.coords.iter().map(|(t, v)| (Tag::new_checked(t.as_bytes()).unwrap(), v.0 as f32)));
            let original = FontRef::new(source).unwrap();
            for &gid in gids {
                let cid = cid_of(gids, gid);
                assert_eq!(cut.cids[&gid], cid as u16, "{at}: glyph {gid} is CID {cid}");
                assert_eq!(hmtx.advance(GlyphId::new(cid)), Some(width(instance, gid)), "{at}: glyph {gid}'s hmtx");
                tally.widths_checked += 1;
                let (a, b) = (draw(&sfnt, cid, LocationRef::default()), draw(&original, u32::from(gid), LocationRef::from(&location)));
                assert_eq!(a.0 == 0, b.0 == 0, "{at}: glyph {gid} draws in the program as in the source");
                let off = (0..4).map(|j| (a.1[j] - b.1[j]).abs()).fold(0.0, f32::max);
                tally.max_outline_box_difference = tally.max_outline_box_difference.max(off);
                assert!(off <= 2.0, "{at}: glyph {gid}'s box in the program is {:?}, the source's at the instance {:?}", a.1, b.1);
            }

            // /W is the list's widths at the profile's decimals, by CID.
            let w = widths_of(&cid_font);
            let upem = f64::from(record.upem);
            let factor = 10f64.powf(list.writing.font_profile.width_decimals.0);
            assert_eq!(w.len(), gids.len(), "{at}: one /W entry per glyph");
            for &gid in gids {
                let want = js_round(f64::from(width(instance, gid)) * 1000.0 / upem * factor) / factor;
                assert_eq!(w[&cid_of(gids, gid)], want as f32, "{at}: /W of glyph {gid}");
            }

            // ToUnicode: every glyph that stands for text maps to the text the record chose for it, in blocks of at most 100.
            let (map, blocks) = to_unicode(&type0);
            assert!(blocks.iter().all(|&b| b <= 100), "{at}: a bfchar block past 100");
            for &gid in gids {
                let chosen = &record.cid_to_text[&gid][0].0;
                assert_eq!(map.get(&cid_of(gids, gid)).map(Vec::as_slice).unwrap_or(&[]), chosen.as_slice(), "{at}: the ToUnicode entry of glyph {gid}");
            }

            let (Some(oracle_fonts), Some(report)) = (&oracle_fonts, &report) else { continue };
            // The oracle's file: CID = glyph id there, so each entry is mapped through the rule.
            let o_type0 = oracle_fonts.get::<Dict>(key.as_bytes()).unwrap();
            let o_cid = o_type0.get::<Array>(b"DescendantFonts").unwrap().iter::<Dict>().next().unwrap();
            let o_desc = o_cid.get::<Dict>(b"FontDescriptor").unwrap();
            assert_eq!(type0.get::<Name>(b"BaseFont").unwrap(), o_type0.get::<Name>(b"BaseFont").unwrap(), "{at}: /BaseFont");
            // A pinned face's box is the union of what its program draws, and HarfBuzz's program also keeps the glyphs its layout closure reaches (ligatures, alternates), which forme's does not: the box is the oracle's or inside it.
            let (bbox, o_bbox) = (descriptor.get::<[f32; 4]>(b"FontBBox").unwrap(), o_desc.get::<[f32; 4]>(b"FontBBox").unwrap());
            assert!(bbox[0] >= o_bbox[0] && bbox[1] >= o_bbox[1] && bbox[2] <= o_bbox[2] && bbox[3] <= o_bbox[3], "{at}: the box {bbox:?} is not inside the oracle's {o_bbox:?}");
            if bbox == o_bbox {
                tally.bboxes_equal += 1;
            } else {
                assert!(!instance.coords.is_empty(), "{at}: a static face's box is its head's, the oracle's");
                tally.bboxes_inside.push(format!("{at}: {bbox:?} inside {o_bbox:?}"));
            }
            for k in ["FontName", "Flags", "ItalicAngle", "Ascent", "Descent", "CapHeight", "StemV"] {
                let (ours, theirs) = (descriptor.get::<Object>(k.as_bytes()).unwrap(), o_desc.get::<Object>(k.as_bytes()).unwrap());
                let same = match (&ours, &theirs) {
                    (Object::Array(a), Object::Array(b)) => a.iter::<f32>().collect::<Vec<_>>() == b.iter::<f32>().collect::<Vec<_>>(),
                    (Object::Number(a), Object::Number(b)) => a.as_f64() == b.as_f64(),
                    _ => ours == theirs,
                };
                assert!(same, "{at}: the descriptor's /{k} is {ours:?}, the oracle's {theirs:?}");
            }
            tally.descriptors_equal += 1;
            let o_w = widths_of(&o_cid);
            assert!(gids.iter().all(|&g| w[&cid_of(gids, g)] == o_w[&u32::from(g)]), "{at}: /W is the oracle's, glyph for glyph");
            assert_eq!(cid_font.get::<f32>(b"DW"), o_cid.get::<f32>(b"DW"), "{at}: /DW");
            tally.w_equal += 1;
            let (o_map, _) = to_unicode(&o_type0);
            let ours: BTreeMap<u32, Vec<u16>> = map.iter().map(|(&cid, t)| (u32::from(*gids.iter().find(|&&g| cid_of(gids, g) == cid).unwrap()), t.clone())).collect();
            assert_eq!(ours, o_map, "{at}: the ToUnicode map is the oracle's, mapped to glyph ids");
            tally.to_unicode_equal += 1;

            // The record, member for member, where the writers share a rule; the program's own members are named as the expected differences.
            let ours = &records["faces"][i];
            let theirs = &report["emission"]["faces"][i];
            for member in SHARED {
                assert_eq!(ours[member], theirs[member], "{at}: the record's {member}");
            }
            if glyf {
                assert_eq!(ours["outlineType"], theirs["outlineType"], "{at}: outlineType");
                assert_eq!(ours["charsetIdentity"], theirs["charsetIdentity"], "{at}: charsetIdentity");
            } else {
                assert_eq!((theirs["outlineType"].as_str(), &theirs["charsetIdentity"]), (Some("cff"), &Value::Null), "{at}: the oracle keeps a SID-keyed CFF");
                assert_eq!((ours["outlineType"].as_str(), &ours["charsetIdentity"]), (Some("cff-cid"), &Value::Bool(true)), "{at}: subsetter writes it CID-keyed, with the identity charset");
            }
            assert_eq!(record.subset_tag, theirs["subsetTag"].as_str().unwrap());
            tally.tags_equal += 1;
        }
    }
    println!("fonts reader: {tally:?}");
    assert_eq!((tally.lists, tally.cff, tally.cff2), (19, 1, 1), "nineteen lists, one CFF instance, one CFF2 instance");
    assert_eq!(tally.patches, 0, "subsetter agrees with HarfBuzz on every glyph the corpus shows");
}

#[test]
fn the_widths_patch_fires_and_corrects_where_subsetter_rounds_a_half_unit_otherwise() {
    // Source Serif 4 at wght 650, opsz 16: the HVAR delta of glyphs 51, 1017 and 1057 is 51.4999, which HarfBuzz rounds to 577 and read-fonts' fixed point to 578 (the instancing trial, `.claude/plans/the-rust-writer/trial-instancing.md` in the BelType monorepo).
    let (paper, blobs) = list_of("paper");
    let mut instance = paper.resources.fonts.iter().find(|f| f.face_id == "Source Serif 4#1").unwrap().clone();
    instance.key = "Source Serif 4#1|opsz=16,wght=650".into();
    instance.coords = BTreeMap::from([("opsz".into(), Num(16.0)), ("wght".into(), Num(650.0))]);
    let gids = [51u16, 1017, 1057];
    instance.widths.gids = gids.iter().map(|&g| Num(f64::from(g))).collect();
    instance.widths.units = vec![Num(577.0); 3];
    let cut = program(&instance, &blobs[&instance.blob], &gids).unwrap();
    assert_eq!(cut.patched, gids, "the patch fired on the three glyphs");
    let font = FontRef::new(&cut.bytes).unwrap();
    for (k, _) in gids.iter().enumerate() {
        assert_eq!(font.hmtx().unwrap().advance(GlyphId::new(k as u32 + 1)), Some(577), "read back as the list's width");
    }
    let num_glyphs = font.maxp().unwrap().num_glyphs();
    assert_eq!(font.hhea().unwrap().number_of_h_metrics(), num_glyphs, "every glyph a long metric");
    assert_eq!(checksum(&cut.bytes), 0xB1B0_AFBA, "checkSumAdjustment recomputed");
    for record in font.table_directory.table_records() {
        let data = font.table_data(record.tag()).unwrap();
        let mut bytes = data.as_bytes().to_vec();
        if record.tag() == Tag::new(b"head") {
            bytes[8..12].fill(0);
        }
        assert_eq!(checksum(&bytes), record.checksum(), "the {} checksum", record.tag());
    }
    // Asked for subsetter's own 578, the patch does not fire: it rewrites only where the widths differ.
    let unpatched = forme::fonts::program(&FontInstance { widths: forme::list::FontWidths { gids: vec![Num(51.0)], units: vec![Num(578.0)] }, ..instance.clone() }, &blobs[&instance.blob], &[51]).unwrap();
    assert!(unpatched.patched.is_empty(), "578 is subsetter's own width, so nothing is patched");
}
