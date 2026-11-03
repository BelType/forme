//! The writer's records as JSON text, written by hand: one small writer instead of serde's `Serialize` for every record type, which cost the module about 17 kB.
//!
//! - **The paint rows** are written as `JSON.stringify` writes them ([`n`]), as [`crate::vectors`] always wrote them.
//! - **Every other record** is written as serde_json wrote it before, byte for byte: a [`Num`] as an integer where it is one and as a real otherwise, a real as Rust's shortest round-trip form with its `.0`, `null` for what is not finite, a string escaped as `JSON.stringify` escapes it, a member left out where serde skipped it. Caster reads the records with `JSON.parse`, so only the values bind; the bytes are held so the records files stay what they were.

use std::fmt::Write;

use crate::Records;
use crate::colour::{Element, Place, Record, Space, Work};
use crate::content::js_string;
use crate::fonts::FaceRecord;
use crate::images::ImageInfo;
use crate::list::{CffContainer, Num, Overprint, PaintTarget, SourceFormat, json_string};

/// A number as `JSON.stringify` writes it: `-0` as `0`, and `null` for what is not finite.
pub(crate) fn n(x: f64) -> String {
    if x.is_finite() { js_string(x) } else { "null".into() }
}

pub(crate) fn array(items: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", items.into_iter().collect::<Vec<_>>().join(","))
}

/// A JSON string.
pub(crate) fn st(text: &str) -> String {
    json_string(&text.encode_utf16().collect::<Vec<_>>())
}

/// A string member that is left out where it is absent.
pub(crate) fn opt_st(text: Option<&str>) -> String {
    text.map_or(String::new(), st)
}

/// A JSON object of the members present: an empty string is an absent member.
pub(crate) fn object(members: &[(&str, String)]) -> String {
    format!("{{{}}}", members.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("\"{k}\":{v}")).collect::<Vec<_>>().join(","))
}

/// The records' writer: one string, written member by member, so a member costs one call. A key of `""` is an array's item.
struct J {
    out: String,
    comma: bool,
}

impl J {
    fn key(&mut self, k: &str) -> &mut String {
        if self.comma {
            self.out.push(',');
        }
        self.comma = true;
        if !k.is_empty() {
            let _ = write!(self.out, "\"{k}\":");
        }
        &mut self.out
    }

    fn open(&mut self, k: &str, bracket: char) {
        self.key(k).push(bracket);
        self.comma = false;
    }

    fn close(&mut self, bracket: char) {
        self.out.push(bracket);
        self.comma = true;
    }

    fn raw(&mut self, k: &str, v: &str) {
        self.key(k).push_str(v);
    }

    fn int(&mut self, k: &str, v: impl std::fmt::Display) {
        let _ = write!(self.key(k), "{v}");
    }

    fn st(&mut self, k: &str, v: &str) {
        let text = st(v);
        self.raw(k, &text);
    }

    /// A string member serde skipped where it was absent.
    fn opt_st(&mut self, k: &str, v: Option<&str>) {
        if let Some(v) = v {
            self.st(k, v);
        }
    }

    /// A real as serde_json writes an `f64`: the shortest round-trip form with `.0` on a whole number, `null` where it is not finite. ponytail: Rust's form, which writes an exponent below 1e-4 where serde_json's ryu does below 1e-5; no record holds such a real, and `JSON.parse` reads either as the same double.
    fn real(&mut self, k: &str, v: f64) {
        if v.is_finite() {
            let _ = write!(self.key(k), "{v:?}");
        } else {
            self.raw(k, "null");
        }
    }

    /// A [`Num`] as its `Serialize` wrote it: an integer where it is one, a real otherwise.
    fn num(&mut self, k: &str, v: Num) {
        if v.0.fract() == 0.0 && v.0.abs() < 9.2e18 {
            self.int(k, v.0 as i64);
        } else {
            self.real(k, v.0);
        }
    }

    fn nums(&mut self, k: &str, v: &[Num]) {
        self.open(k, '[');
        for &x in v {
            self.num("", x);
        }
        self.close(']');
    }

    fn ints<T: std::fmt::Display>(&mut self, k: &str, v: impl IntoIterator<Item = T>) {
        self.open(k, '[');
        for x in v {
            self.int("", x);
        }
        self.close(']');
    }

    fn place(&mut self, k: &str, p: &Place) {
        self.open(k, '{');
        self.int("page", p.page);
        self.int("item", p.item);
        self.close('}');
    }

    fn target(&mut self, t: PaintTarget) {
        self.st("target", match t {
            PaintTarget::Background => "background",
            PaintTarget::Border => "border",
            PaintTarget::Fill => "fill",
        });
    }
}

/// The records as Caster's `WriterRecords` JSON.
pub fn records(r: &Records) -> String {
    let mut j = J { out: String::new(), comma: false };
    j.open("", '{');
    j.open("mcids", '[');
    for page in &r.mcids {
        j.ints("", page);
    }
    j.close(']');
    j.open("pens", '[');
    for p in &r.pens {
        j.open("", '{');
        j.int("page", p.page);
        j.int("item", p.item);
        j.open("adjustments", '[');
        for a in &p.adjustments {
            match a {
                Some(a) => j.num("", *a),
                None => j.raw("", "null"),
            }
        }
        j.close(']');
        j.close('}');
    }
    j.close(']');
    j.int("kernAdjustments", r.kern_adjustments);
    j.int("actualTextSpans", r.actual_text_spans);
    j.open("faces", '[');
    for f in &r.faces {
        face(&mut j, f);
    }
    j.close(']');
    j.open("images", '[');
    for u in &r.images {
        j.open("", '{');
        j.int("page", u.page);
        j.int("item", u.item);
        j.int("source", u.source);
        j.st("name", &u.name);
        j.st("hash", &u.hash);
        image_info(&mut j, &u.info);
        j.close('}');
    }
    j.close(']');
    j.int("xobjects", r.xobjects);
    j.raw("paint", &r.paint);
    j.raw("vectorResources", &r.vector_resources);
    if let Some(c) = &r.colour {
        colour(&mut j, c);
    }
    j.close('}');
    j.out
}

fn face(j: &mut J, f: &FaceRecord) {
    j.open("", '{');
    j.st("id", &f.id);
    j.st("postScriptName", &f.post_script_name);
    j.int("usedGlyphs", f.used_glyphs);
    j.int("fontFileBytes", f.font_file_bytes);
    j.int("multiCharToUnicode", f.multi_char_to_unicode);
    j.int("toUnicodeConflicts", f.to_unicode_conflicts);
    j.st("faceId", &f.face_id);
    j.st("family", &f.family);
    j.st("sourceFormat", match f.source_format {
        SourceFormat::Ttf => "ttf",
        SourceFormat::Otf => "otf",
        SourceFormat::Woff => "woff",
        SourceFormat::Woff2 => "woff2",
    });
    j.st("outlineType", f.outline_type);
    j.int("upem", f.upem);
    j.open("coords", '{');
    for (k, &v) in &f.coords {
        j.num(k, v);
    }
    j.close('}');
    j.ints("glyphsUsed", &f.glyphs_used);
    j.int("sourceBytes", f.source_bytes);
    j.int("subsetBytes", f.subset_bytes);
    j.int("subsetFlateBytes", f.subset_flate_bytes);
    j.int("numGlyphs", f.num_glyphs);
    j.int("nonEmptyGlyphs", f.non_empty_glyphs);
    j.int("hmtxInstanced", f.hmtx_instanced);
    j.int("staleAdvances", f.stale_advances);
    j.st("subsetTag", &f.subset_tag);
    j.st("cffContainer", match f.cff_container {
        CffContainer::Bare => "bare",
        CffContainer::Opentype => "opentype",
    });
    j.open("cidToText", '{');
    for (k, v) in &f.cid_to_text {
        j.open(&k.to_string(), '[');
        for t in v {
            j.raw("", &json_string(&t.0));
        }
        j.close(']');
    }
    j.close('}');
    j.ints("ambiguousCids", &f.ambiguous_cids);
    j.open("tables", '[');
    for t in &f.tables {
        j.st("", t);
    }
    j.close(']');
    match f.charset_identity {
        Some(b) => j.int("charsetIdentity", b),
        None => j.raw("charsetIdentity", "null"),
    }
    j.int("charsetRetried", f.charset_retried);
    j.st("fsType", &f.fs_type);
    j.st("path", f.path);
    j.open("ms", '{');
    j.real("instance", f.ms.instance);
    j.real("parse", f.ms.parse);
    j.real("subset", f.ms.subset);
    j.real("write", f.ms.write);
    j.close('}');
    j.close('}');
}

fn image_info(j: &mut J, i: &ImageInfo) {
    j.open("info", '{');
    j.st("path", i.path);
    j.st("filter", i.filter);
    j.int("bitsPerComponent", i.bits_per_component);
    j.st("colorSpace", i.color_space);
    j.int("hasSMask", i.has_s_mask);
    if let Some(d) = i.decode {
        j.ints("decode", d);
    }
    j.real("width", i.width);
    j.real("height", i.height);
    j.int("interpolate", i.interpolate);
    j.int("bytesOut", i.bytes_out);
    j.real("decodeMs", i.decode_ms);
    j.real("encodeMs", i.encode_ms);
    j.st("icc", i.icc);
    j.int("gammaOnly", i.gamma_only);
    j.close('}');
}

fn colour(j: &mut J, c: &Record) {
    j.open("colour", '{');
    j.open("elements", '[');
    for e in &c.elements {
        element(j, e);
    }
    j.close(']');
    j.open("colorSpaces", '[');
    for s in &c.color_spaces {
        space(j, s);
    }
    j.close(']');
    j.open("extGStates", '[');
    for s in &c.ext_g_states {
        j.open("", '{');
        j.st("name", &s.name);
        j.st("mode", s.mode);
        j.int("OP", s.OP);
        j.int("op", s.op);
        j.int("OPM", s.OPM);
        j.close('}');
    }
    j.close(']');
    let o = &c.output_intent;
    j.open("outputIntent", '{');
    j.st("subtype", o.subtype);
    j.st("condition", &o.condition);
    j.st("registryName", o.registry_name);
    j.st("info", &o.info);
    j.int("n", o.n);
    j.int("profileBytes", o.profile_bytes);
    j.st("profileSha256", &o.profile_sha256);
    j.st("profileClass", &o.profile_class);
    j.st("profileSpace", &o.profile_space);
    j.st("profileVersion", &o.profile_version);
    j.close('}');
    j.int("defaultRgbBytes", c.default_rgb_bytes);
    j.st("defaultRgbSha256", &c.default_rgb_sha256);
    j.st("blackTextPolicy", c.black_text_policy);
    j.int("blackTextRuns", c.black_text_runs);
    j.open("worklist", '[');
    for w in &c.worklist {
        work(j, w);
    }
    j.close(']');
    j.raw("registrations", "[]");
    j.close('}');
}

fn element(j: &mut J, e: &Element) {
    j.open("", '{');
    if let Some(at) = &e.at {
        j.place("at", at);
    }
    j.opt_st("path", e.path.as_deref());
    j.target(e.target);
    j.opt_st("hint", e.hint.as_deref());
    let r = &e.resolved;
    j.open("resolved", '{');
    j.st("kind", r.kind);
    j.nums("values", &r.values);
    j.opt_st("name", r.name.as_deref());
    if let Some(t) = r.tint {
        j.num("tint", t);
    }
    j.close('}');
    if let Some(o) = e.overprint {
        j.st("overprint", match o {
            Overprint::Fill => "fill",
            Overprint::Stroke => "stroke",
            Overprint::Both => "both",
        });
    }
    j.st("operator", e.operator);
    j.opt_st("resource", e.resource.as_deref());
    if let Some(b) = e.black_text_policy {
        j.int("blackTextPolicy", b);
    }
    if let Some(r) = &e.rect_pt {
        j.nums("rectPt", r);
    }
    if let Some(p) = e.page {
        j.num("page", p);
    }
    j.close('}');
}

fn work(j: &mut J, w: &Work) {
    j.open("", '{');
    if let Some(at) = &w.at {
        j.place("at", at);
    }
    j.opt_st("path", w.path.as_deref());
    j.target(w.target);
    j.nums("rgb", &w.rgb);
    j.opt_st("css", w.css.as_deref());
    j.int("uses", w.uses);
    j.close('}');
}

fn space(j: &mut J, s: &Space) {
    j.open("", '{');
    j.st("name", &s.name);
    j.st("kind", s.kind);
    j.st("alternate", &s.alternate);
    j.opt_st("colorant", s.colorant.as_deref());
    j.opt_st("escaped", s.escaped.as_deref());
    if let Some(c) = &s.c1 {
        j.nums("c1", c);
    }
    j.close('}');
}
