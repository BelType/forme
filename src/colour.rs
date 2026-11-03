//! The colour story: the document's colour objects and the operator each paint is written with, ported from Caster's `src/colour/colorResources.ts` (`ColorResources`, `ColorEmitter`) and the `marks` branch of `src/emit/structureEmit.ts`.
//!
//! - **The output intent** is `/GTS_PDFX` with the CMYK profile as `/DestOutputProfile`; the profile is read by three header fields at fixed offsets (the device class at 12, the colour space at 16, the version at 8) and no ICC library, and one that is not CMYK, not `prtr` or `mntr`, or an sRGB profile that is not RGB, is refused in the oracle's sentence.
//! - **`/DefaultRGB`** is on every page, an `/ICCBased` array over the one sRGB stream, beside every `/Separation` and every overprint `/ExtGState` the document allocated, since a name is allocated on first use and the last page may use it first.
//! - **A spot ink is a colour space**: `/Separation`, the colourant's UTF-8 bytes escaped as `escapeColorantName` escapes them (pdf-writer's `Name` escapes the same set), `/DeviceCMYK` and a type 2 function from white to the alternate; one per colourant name, `All` and `None` among them, a second alternate for one name refused. `CS0`, `CS1`, … and `GS0`, `GS1`, … in first-use order, the reset state allocated with the first overprint.
//! - **The operator choice** is the oracle's, in its order: a declared ink wins (`k`, or `/CSn cs t scn`); an unhinted black run under `blackText: '100k'` is `0 0 0 1 k`; anything else stays `rg` and goes on the work list. A rectangle takes no black-text policy. Every colour operand is rounded to four decimals.
//! - **Print marks** are stroked in `/All` at full tint, `0.25 w 0 J 0 j [] 0 d`, every number at thousandths.

use pdf_writer::{Filter, Finish, Name, Pdf, Ref, TextStr};
use sha2::{Digest, Sha256};

use crate::Refusal;
use crate::content::{Content, Operand, js_round, js_string, round3};
use crate::list::{BlackTextPolicy, Blobs, ColourWriting, DisplayList, Fill, Ink, MarksItem, Num, Overprint, PaintTarget, RectItem, TextItem};

/// `KAPPA` of Caster's `src/vectors/pathData.ts`, the circle's control-point ratio.
const KAPPA: f64 = 0.5523;

/// The hairline every mark is stroked at, `MARK_STROKE_PT`.
const MARK_STROKE_PT: f64 = 0.25;

/// One colour operand at four decimals, the oracle's `num`.
fn num4(n: f64) -> f64 {
    js_round(n * 10000.0) / 10000.0
}

/// Latin-1 text of `bytes[from..from + 4]`, as far as the profile holds it, as `TextDecoder('latin1')` reads the field.
fn field(bytes: &[u8], from: usize) -> String {
    bytes.iter().skip(from).take(4).map(|&b| char::from(b)).collect()
}

/// `JSON.stringify` of a field read by [`field`].
fn quoted(s: &str) -> String {
    crate::list::json_string(&s.encode_utf16().collect::<Vec<_>>())
}

/// `escapeColorantName`: the colourant's UTF-8 bytes, each outside PDF's regular printable set written `#XX`.
pub fn escape_colorant(name: &str) -> String {
    name.bytes().map(|b| if (0x21..=0x7e).contains(&b) && !b"#%()/<>[]{}".contains(&b) { char::from(b).to_string() } else { format!("#{b:02X}") }).collect()
}

/// The non-stroking colour one paint is set with.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Rgb([f64; 3]),
    Cmyk([f64; 4]),
    /// A separation, by its index (its resource is `CS<index>`), and the tint.
    Spot(usize, f64),
}

/// `RunColorOperators`: the overprint state set before the paint and put back after it, by index (`GS<index>`), and the paint.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintOps {
    pub setup: Option<usize>,
    pub paint: Paint,
    pub reset: Option<usize>,
}

impl PaintOps {
    /// `/GSn gs`, when there is one.
    pub fn write_setup(&self, c: &mut Content) {
        if let Some(i) = self.setup {
            c.op(&[Operand::Name(&format!("GS{i}"))], "gs");
        }
    }

    /// The fill operators: `r g b rg`, `c m y k k`, or `/CSn cs t scn`.
    pub fn write_paint(&self, c: &mut Content) {
        match self.paint {
            Paint::Rgb(v) => c.op(&v.map(|n| Operand::Num(num4(n))), "rg"),
            Paint::Cmyk(v) => c.op(&v.map(|n| Operand::Num(num4(n))), "k"),
            Paint::Spot(i, tint) => c.op(&[Operand::Name(&format!("CS{i}"))], "cs").op(&[Operand::Num(num4(tint))], "scn"),
        };
    }

    /// The `/GS0 gs` that puts the state back, when one was set.
    pub fn write_reset(&self, c: &mut Content) {
        if let Some(i) = self.reset {
            c.op(&[Operand::Name(&format!("GS{i}"))], "gs");
        }
    }
}

/// An item of the list, by its page and its index in that page's items.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub page: usize,
    pub item: usize,
}

/// A resolved colour as the report holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub kind: &'static str,
    pub values: Vec<Num>,
    pub name: Option<String>,
    pub tint: Option<Num>,
}

/// One row of the colour record: a text run's, keyed by `at`, or a rectangle's, whole.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub at: Option<Place>,
    pub path: Option<String>,
    pub target: PaintTarget,
    pub hint: Option<String>,
    pub resolved: Resolved,
    pub overprint: Option<Overprint>,
    pub operator: &'static str,
    pub resource: Option<String>,
    pub black_text_policy: Option<bool>,
    pub rect_pt: Option<[Num; 4]>,
    pub page: Option<Num>,
}

/// One work-list entry: an RGB colour left as `rg`, the first painter's, counted once per use.
#[derive(Clone, Debug, PartialEq)]
pub struct Work {
    pub at: Option<Place>,
    pub path: Option<String>,
    pub target: PaintTarget,
    pub rgb: [Num; 3],
    pub css: Option<String>,
    pub uses: usize,
}

/// One colour space of the record.
#[derive(Clone, Debug, PartialEq)]
pub struct Space {
    pub name: String,
    pub kind: &'static str,
    pub alternate: String,
    pub colorant: Option<String>,
    pub escaped: Option<String>,
    pub c1: Option<[Num; 4]>,
}

/// One overprint graphics state of the record.
#[derive(Clone, Debug, PartialEq)]
#[allow(non_snake_case)]
pub struct State {
    pub name: String,
    pub mode: &'static str,
    pub OP: bool,
    pub op: bool,
    pub OPM: u8,
}

/// The output intent of the record.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputIntent {
    pub subtype: &'static str,
    pub condition: String,
    pub registry_name: &'static str,
    pub info: String,
    pub n: i32,
    pub profile_bytes: usize,
    pub profile_sha256: String,
    pub profile_class: String,
    pub profile_space: String,
    pub profile_version: String,
}

/// Caster's `WrittenColour`: the writer's colour record.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub elements: Vec<Element>,
    pub color_spaces: Vec<Space>,
    pub ext_g_states: Vec<State>,
    pub output_intent: OutputIntent,
    pub default_rgb_bytes: usize,
    pub default_rgb_sha256: String,
    pub black_text_policy: &'static str,
    pub black_text_runs: usize,
    pub worklist: Vec<Work>,
    pub registrations: Vec<()>,
}

/// One allocated `/Separation`: its colourant and its alternate.
struct Separation {
    colorant: String,
    alternate: [Num; 4],
}

/// What the colour objects are on every page: the names and objects of `/ColorSpace` (`/DefaultRGB` first) and of `/ExtGState`.
pub struct PageResources {
    pub srgb: Ref,
    pub spaces: Vec<(String, Ref)>,
    pub states: Vec<(String, Ref)>,
}

/// The document's colour objects and the record of every choice, one per file.
pub struct Colour<'a> {
    story: &'a ColourWriting,
    profile: &'a [u8],
    srgb: &'a [u8],
    separations: Vec<Separation>,
    /// The overprint modes allocated, in order: `fill`, `stroke`, `both` or `reset`.
    states: Vec<&'static str>,
    elements: Vec<Element>,
    worklist: Vec<(String, Work)>,
    black_text_runs: usize,
}

impl<'a> Colour<'a> {
    /// The colour story of `list`, when it has one.
    ///
    /// # Errors
    /// An output profile that is not CMYK, or whose device class is neither `prtr` nor `mntr`; a `/DefaultRGB` profile that is not RGB; each in the oracle's sentence.
    pub fn new(list: &'a DisplayList, blobs: &'a Blobs) -> Result<Option<Self>, Refusal> {
        let Some(story) = &list.writing.colour else { return Ok(None) };
        let profile = blobs[&story.profiles.cmyk].as_slice();
        let srgb = blobs[&story.profiles.srgb].as_slice();
        let space = field(profile, 16);
        if space != "CMYK" {
            return Err(Refusal::new(format!("colorResources: the output profile is {}, not CMYK", quoted(&space))));
        }
        let class = field(profile, 12);
        if class != "prtr" && class != "mntr" {
            return Err(Refusal::new(format!("colorResources: the output profile's device class is {}; PDF/A-4 6.2.3 t1 wants prtr or mntr", quoted(&class))));
        }
        if field(srgb, 16) != "RGB " {
            return Err(Refusal::new("colorResources: the /DefaultRGB profile is not an RGB profile".into()));
        }
        Ok(Some(Self { story, profile, srgb, separations: Vec::new(), states: Vec::new(), elements: Vec::new(), worklist: Vec::new(), black_text_runs: 0 }))
    }

    /// `separationRef`: the index of the `/Separation` for one colourant, allocated on first use.
    fn separation(&mut self, colorant: &str, alternate: &[Num; 4]) -> Result<usize, Refusal> {
        if let Some(i) = self.separations.iter().position(|s| s.colorant == colorant) {
            let had = &self.separations[i].alternate;
            if had != alternate {
                let join = |a: &[Num; 4]| a.iter().map(|n| js_string(n.0)).collect::<Vec<_>>();
                return Err(Refusal::new(format!("colorResources: the spot {} is declared with two alternates, device-cmyk({}) and device-cmyk({})", quoted(colorant), join(had).join(" "), join(alternate).join(" "))));
            }
            return Ok(i);
        }
        self.separations.push(Separation { colorant: colorant.into(), alternate: *alternate });
        Ok(self.separations.len() - 1)
    }

    /// The index of the graphics state for one mode, allocated on first use.
    fn state(&mut self, mode: &'static str) -> usize {
        self.states.iter().position(|&m| m == mode).unwrap_or_else(|| {
            self.states.push(mode);
            self.states.len() - 1
        })
    }

    /// `extGStateRef`: the reset state first, then the mode's.
    fn overprint(&mut self, mode: Option<Overprint>) -> (Option<usize>, Option<usize>) {
        let Some(mode) = mode else { return (None, None) };
        let reset = self.state("reset");
        let set = self.state(overprint_name(mode));
        (Some(set), Some(reset))
    }

    /// A declared ink as its paint, the resolved colour, the operator and the separation's resource.
    fn hinted(&mut self, ink: &Ink) -> Result<(Paint, Resolved, &'static str, Option<String>), Refusal> {
        Ok(match ink {
            Ink::Cmyk(c) => (Paint::Cmyk(c.cmyk.map(|n| n.0)), Resolved { kind: "cmyk", values: c.cmyk.to_vec(), name: None, tint: None }, "k", None),
            Ink::Spot(s) => {
                let i = self.separation(&s.spot, &s.alternate)?;
                let kind = match s.spot.as_str() {
                    "All" => "all",
                    "None" => "none",
                    _ => "spot",
                };
                (Paint::Spot(i, s.tint.0), Resolved { kind, values: s.alternate.to_vec(), name: Some(s.spot.clone()), tint: Some(s.tint) }, "scn", Some(format!("CS{i}")))
            }
        })
    }

    /// `noteEntry`: the work list's entry for one colour, the first painter's, counted once per use.
    fn note(&mut self, target: PaintTarget, rgb: &[Num; 3], first: Work) {
        let key = format!("{}|{}", target_name(target), rgb.iter().map(|n| js_string(n.0)).collect::<Vec<_>>().join(","));
        match self.worklist.iter_mut().find(|(k, _)| *k == key) {
            Some((_, hit)) => hit.uses += 1,
            None => self.worklist.push((key, first)),
        }
    }

    /// `runOperators`: the colour a run is painted with, and the record of why.
    ///
    /// # Errors
    /// A colourant declared with two alternates.
    pub fn run(&mut self, item: &TextItem, page: usize, at: usize) -> Result<PaintOps, Refusal> {
        let fill = &item.fill;
        let (setup, reset) = self.overprint(fill.overprint);
        let row = |resolved, operator, resource, black| Element { at: Some(Place { page, item: at }), path: None, target: PaintTarget::Fill, hint: None, resolved, overprint: fill.overprint, operator, resource, black_text_policy: black, rect_pt: None, page: None };
        let rgb = fill.rgb.map(|n| n.0);
        let paint = if let Some(ink) = &fill.ink {
            let (paint, resolved, operator, resource) = self.hinted(ink)?;
            self.elements.push(row(resolved, operator, resource, None));
            paint
        } else if self.story.black_text == BlackTextPolicy::HundredK && rgb == [0.0; 3] {
            self.black_text_runs += 1;
            self.elements.push(row(Resolved { kind: "cmyk", values: vec![Num(0.0), Num(0.0), Num(0.0), Num(1.0)], name: None, tint: None }, "k", None, Some(true)));
            Paint::Cmyk([0.0, 0.0, 0.0, 1.0])
        } else {
            self.elements.push(row(Resolved { kind: "rgb", values: fill.rgb.to_vec(), name: None, tint: None }, "rg", None, None));
            self.note(PaintTarget::Fill, &fill.rgb, Work { at: Some(Place { page, item: at }), path: None, target: PaintTarget::Fill, rgb: fill.rgb, css: None, uses: 1 });
            Paint::Rgb(rgb)
        };
        Ok(PaintOps { setup, paint, reset })
    }

    /// `paintOperators`: the same for one measured rectangle, with no black-text policy.
    ///
    /// # Errors
    /// A colourant declared with two alternates.
    pub fn rect(&mut self, item: &RectItem) -> Result<Option<PaintOps>, Refusal> {
        let Some(origin) = &item.paint else { return Ok(None) };
        let fill: &Fill = &item.fill;
        let (setup, reset) = self.overprint(fill.overprint);
        let row = |resolved, operator, resource| Element { at: None, path: Some(origin.path.clone()), target: origin.target, hint: origin.hint.clone(), resolved, overprint: fill.overprint, operator, resource, black_text_policy: None, rect_pt: Some(item.rect), page: Some(origin.page) };
        let paint = if let Some(ink) = &fill.ink {
            let (paint, resolved, operator, resource) = self.hinted(ink)?;
            self.elements.push(row(resolved, operator, resource));
            paint
        } else {
            self.elements.push(row(Resolved { kind: "rgb", values: fill.rgb.to_vec(), name: None, tint: None }, "rg", None));
            self.note(origin.target, &fill.rgb, Work { at: None, path: Some(origin.path.clone()), target: origin.target, rgb: fill.rgb, css: Some(origin.color.clone()), uses: 1 });
            Paint::Rgb(fill.rgb.map(|n| n.0))
        };
        Ok(Some(PaintOps { setup, paint, reset }))
    }

    /// The page's print marks: `q`, `/All` at full tint as the stroking colour, the hairline's state, every segment and circle at thousandths, `S Q`.
    ///
    /// # Errors
    /// An author's `All` declared with another alternate than `device-cmyk(1 1 1 1)`.
    pub fn marks(&mut self, c: &mut Content, item: &MarksItem) -> Result<(), Refusal> {
        let all = self.separation("All", &[Num(1.0); 4])?;
        c.op(&[], "q");
        c.op(&[Operand::Name(&format!("CS{all}"))], "CS").op(&[Operand::Num(1.0)], "SCN");
        c.op(&[Operand::Num(MARK_STROKE_PT)], "w").op(&[Operand::Num(0.0)], "J").op(&[Operand::Num(0.0)], "j").op(&[Operand::Raw("[]"), Operand::Num(0.0)], "d");
        let n = |v: f64| Operand::Num(round3(v));
        for shape in &item.shapes {
            for s in &shape.segments {
                c.op(&[n(s[0].0), n(s[1].0)], "m").op(&[n(s[2].0), n(s[3].0)], "l");
            }
            if let Some([cx, cy, r]) = shape.circle {
                let (cx, cy, r) = (cx.0, cy.0, r.0);
                let k = KAPPA * r;
                c.op(&[n(cx + r), n(cy)], "m");
                for curve in [[cx + r, cy + k, cx + k, cy + r, cx, cy + r], [cx - k, cy + r, cx - r, cy + k, cx - r, cy], [cx - r, cy - k, cx - k, cy - r, cx, cy - r], [cx + k, cy - r, cx + r, cy - k, cx + r, cy]] {
                    c.op(&curve.map(n), "c");
                }
            }
        }
        c.op(&[], "S").op(&[], "Q");
        Ok(())
    }

    /// Write the colour objects (the sRGB stream, every `/Separation`, every `/ExtGState`, the output profile and the intent), and hand back what every page's resources name, the output intent for the catalog, and the record.
    ///
    /// `srgb` is the sRGB stream's object when a blend's group named it before the colour objects were written.
    pub fn finish(self, pdf: &mut Pdf, next: &mut Ref, srgb: Option<Ref>) -> (PageResources, Ref, Record) {
        let srgb = match srgb {
            Some(id) => crate::images::icc_stream_at(pdf, id, self.srgb, 3),
            None => crate::images::icc_stream(pdf, next, self.srgb, 3),
        };
        let mut spaces = Vec::new();
        for (i, s) in self.separations.iter().enumerate() {
            let id = next.bump();
            let mut array = pdf.indirect(id).array();
            array.item(Name(b"Separation")).item(Name(s.colorant.as_bytes())).item(Name(b"DeviceCMYK"));
            let mut f = array.push().dict();
            f.pair(Name(b"FunctionType"), 2);
            f.insert(Name(b"Domain")).array().items([0, 1]);
            f.insert(Name(b"C0")).array().items([0, 0, 0, 0]);
            f.insert(Name(b"C1")).array().items(s.alternate.map(|n| n.0 as f32));
            f.pair(Name(b"N"), 1);
            f.finish();
            array.finish();
            spaces.push((format!("CS{i}"), id));
        }
        let mut states = Vec::new();
        for (i, &mode) in self.states.iter().enumerate() {
            let id = next.bump();
            let (stroke, fill) = overprints(mode);
            let mut d = pdf.indirect(id).dict();
            d.pair(Name(b"Type"), Name(b"ExtGState")).pair(Name(b"OP"), stroke).pair(Name(b"op"), fill).pair(Name(b"OPM"), 1);
            d.finish();
            states.push((format!("GS{i}"), id));
        }
        let space = field(self.profile, 16);
        let n = 4;
        let profile = next.bump();
        pdf.stream(profile, &crate::fonts::deflate(self.profile)).filter(Filter::FlateDecode).pair(Name(b"N"), n);
        let intent = next.bump();
        let mut d = pdf.indirect(intent).dict();
        d.pair(Name(b"Type"), Name(b"OutputIntent")).pair(Name(b"S"), Name(b"GTS_PDFX"));
        d.pair(Name(b"OutputConditionIdentifier"), TextStr(&self.story.condition));
        d.pair(Name(b"RegistryName"), TextStr("http://www.color.org"));
        d.pair(Name(b"Info"), TextStr(&self.story.info));
        d.pair(Name(b"DestOutputProfile"), profile);
        d.finish();

        let sha = |b: &[u8]| crate::hex(&Sha256::digest(b));
        let mut color_spaces = vec![Space { name: "DefaultRGB".into(), kind: "ICCBased", alternate: "sRGB ICC, /N 3".into(), colorant: None, escaped: None, c1: None }];
        color_spaces.extend(self.separations.iter().enumerate().map(|(i, s)| Space { name: format!("CS{i}"), kind: "Separation", alternate: "DeviceCMYK".into(), colorant: Some(s.colorant.clone()), escaped: Some(escape_colorant(&s.colorant)), c1: Some(s.alternate) }));
        let record = Record {
            elements: self.elements,
            color_spaces,
            ext_g_states: self.states.iter().enumerate().map(|(i, &mode)| {
                let (stroke, fill) = overprints(mode);
                State { name: format!("GS{i}"), mode, OP: stroke, op: fill, OPM: 1 }
            }).collect(),
            output_intent: OutputIntent {
                subtype: "GTS_PDFX",
                condition: self.story.condition.clone(),
                registry_name: "http://www.color.org",
                info: self.story.info.clone(),
                n,
                profile_bytes: self.profile.len(),
                profile_sha256: sha(self.profile),
                profile_class: field(self.profile, 12),
                profile_space: space,
                profile_version: format!("{}.{}.{}", self.profile[8], self.profile[9] >> 4, self.profile[9] & 0x0f),
            },
            default_rgb_bytes: self.srgb.len(),
            default_rgb_sha256: sha(self.srgb),
            black_text_policy: match self.story.black_text {
                BlackTextPolicy::None => "none",
                BlackTextPolicy::HundredK => "100k",
            },
            black_text_runs: self.black_text_runs,
            worklist: self.worklist.into_iter().map(|(_, w)| w).collect(),
            registrations: Vec::new(),
        };
        (PageResources { srgb, spaces, states }, intent, record)
    }
}

/// An overprint mode's name in the record.
fn overprint_name(mode: Overprint) -> &'static str {
    match mode {
        Overprint::Fill => "fill",
        Overprint::Stroke => "stroke",
        Overprint::Both => "both",
    }
}

/// `/OP` and `/op` of a mode.
fn overprints(mode: &str) -> (bool, bool) {
    (mode == "stroke" || mode == "both", mode == "fill" || mode == "both")
}

/// A paint target's name, as the work list keys it.
fn target_name(t: PaintTarget) -> &'static str {
    match t {
        PaintTarget::Background => "background",
        PaintTarget::Border => "border",
        PaintTarget::Fill => "fill",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colourant_names_escape_as_the_oracle() {
        assert_eq!(escape_colorant("PANTONE 200 C"), "PANTONE#20200#20C");
        assert_eq!(escape_colorant("Grün#1/(x)"), "Gr#C3#BCn#231#2F#28x#29");
        assert_eq!(escape_colorant("All"), "All");
    }
}
