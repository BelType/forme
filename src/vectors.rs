//! The vector items, ported from Caster's `src/vectors/vectorEmit.ts`, `shading.ts` and `transparency.ts`: measured geometry in, content-stream operators out.
//!
//! - **A path** is its graphics state, then a flat fill as `f` or `f*`, a flat stroke as `w J j M d` and `S`, both as `B` or `B*`, and `paint-order: stroke` as the path twice, stroked and then filled. A gradient fill is `q`, the path, `W n`, the gradient's `cm` and `sh`, `Q`; its alpha stops are a `/DeviceGray` shading in a transparency group that an `/SMask` reads. A gradient stroke is `/Pattern CS /Pn SCN` over a type 2 pattern whose `/Matrix` is the gradient's.
//! - **A shading** is `/ShadingType 2` or `3` over a `/FunctionType 3` stitching function of type 2 segments, its `/Domain` the stops' own range, a hard stop nudged by a millionth of the domain; colours at [`round4`], the domain and bounds at [`round6`]; one object per distinct gradient, `Sh0`, `Sh1`, … in first-use order, and `P0`, `P1`, … for patterns.
//! - **A graphics state** is one `/ExtGState` per distinct `/ca`, `/CA`, `/BM` and `/SMask`, `GT0`, `GT1`, … so as not to meet the colour story's `GS<n>`, with `/SMask /None` where it sets alpha or a blend but no mask.
//! - **An isolated group and a soft mask** are transparency-group forms, `Fm0`, `Fm1`, … each its own; **a form item** is a plain form written once per key, `Fx0`, `Fx1`, … and drawn with its matrix wherever it is named; **a raster fallback** is the image XObject the images made, placed by its `cm`.
//! - **Every name is recorded into the scope of the stream that uses it**, a page's or a form's own `/Resources`, and every node painted is a row of the `paint` record, in painting order; the counts are `vectorResources`.

use pdf_writer::{Finish, Name, Pdf, Ref};

use crate::Refusal;
use crate::json::{array, n, object, opt_st, st};
use crate::content::{Content, Operand, js_round, js_string, round3};
use crate::images::Images;
use crate::list::{BoxPt, ClipPath, DisplayList, FillRule, FormItem, Gradient, GradientType, GroupItem, Matrix, MaskType, Num, Paint, PathItem, PathSegment, RasterFallbackItem, ShadingSpace, SoftMask, Spread, Stroke, VectorItem, VectorOrigin};

/// `Math.round(n * 10000) / 10000`, the shadings' and graphics states' rounding.
pub fn round4(n: f64) -> f64 {
    js_round(n * 10000.0) / 10000.0
}

/// `Math.round(n * 1e6) / 1e6`, a shading's domain and bounds.
pub fn round6(n: f64) -> f64 {
    js_round(n * 1e6) / 1e6
}

/// The resources one content stream refers to, a page's or a form's, each in the order of first use.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    pub shadings: Vec<(String, Ref)>,
    pub patterns: Vec<(String, Ref)>,
    pub ext_g_states: Vec<(String, Ref)>,
    pub xobjects: Vec<(String, Ref)>,
}

/// Put `name` into one map of a scope, once.
fn put(map: &mut Vec<(String, Ref)>, name: &str, id: Ref) {
    if !map.iter().any(|(n, _)| n == name) {
        map.push((name.to_string(), id));
    }
}

/// One vector node, whichever kind.
#[derive(Clone, Copy)]
pub enum Node<'a> {
    Path(&'a PathItem),
    Group(&'a GroupItem),
    Form(&'a FormItem),
    Raster(&'a RasterFallbackItem),
}

impl<'a> From<&'a VectorItem> for Node<'a> {
    fn from(v: &'a VectorItem) -> Self {
        match v {
            VectorItem::Path(x) => Node::Path(x),
            VectorItem::Group(x) => Node::Group(x),
            VectorItem::Form(x) => Node::Form(x),
            VectorItem::RasterFallback(x) => Node::Raster(x),
        }
    }
}

/// What every vector item carries whatever its kind.
struct Painted<'a> {
    transform: Option<&'a Matrix>,
    clips: &'a [ClipPath],
    opacity: Option<f64>,
    stroke_opacity: Option<f64>,
    blend: Option<&'a str>,
    soft_mask: Option<&'a SoftMask>,
    origin: &'a VectorOrigin,
    owner: Option<&'a str>,
}

macro_rules! painted {
    ($node:expr, $($variant:ident),+) => {
        match $node {
            $(Node::$variant(x) => Painted { transform: x.transform.as_ref(), clips: x.clips.as_deref().unwrap_or(&[]), opacity: x.opacity.map(|n| n.0), stroke_opacity: x.stroke_opacity.map(|n| n.0), blend: x.blend.as_deref(), soft_mask: x.soft_mask.as_ref(), origin: &x.origin, owner: x.owner.as_deref() },)+
        }
    };
}

impl<'a> Node<'a> {
    fn painted(self) -> Painted<'a> {
        painted!(self, Path, Group, Form, Raster)
    }
}

/// What every node under one top-level item is emitted on behalf of: its owner, its page and paint index, and whether it is painted as an artifact.
struct Info<'a> {
    owner: Option<&'a str>,
    page: Option<f64>,
    paint_index: Option<f64>,
    artifact: bool,
}

/// A soft mask for a graphics state: its form, its type and its backdrop.
type Mask<'a> = (Ref, &'static str, Option<&'a [Num]>);

/// One shading in the file: its name, its object and its `/Domain`.
struct Shading {
    key: String,
    name: String,
    id: Ref,
    domain: [f64; 2],
}

/// The document's vector objects, shared across every page of one file, and the records of what was painted.
pub struct Vectors<'a> {
    pdf: &'a mut Pdf,
    next: &'a mut Ref,
    list: &'a DisplayList,
    images: &'a Images,
    shadings: Vec<Shading>,
    patterns: Vec<(String, Ref)>,
    states: Vec<(String, Ref)>,
    forms: usize,
    shared: Vec<(String, String, Ref)>,
    /// One row per node painted, in painting order, each a JSON object: Caster's `ReportPaint`.
    pub paint: Vec<String>,
}

// The paint record is written as JSON text by hand, as `JSON.stringify` writes it, by [`crate::json`]'s small writer. An empty string is an absent member.

fn nums(xs: &[Num], round: fn(f64) -> f64) -> String {
    array(xs.iter().map(|x| n(round(x.0))))
}

fn r3(xs: &[f64]) -> String {
    array(xs.iter().map(|&x| n(round3(x))))
}

fn ops_num(xs: &[Num]) -> Vec<Operand<'static>> {
    xs.iter().map(|x| Operand::Num(round3(x.0))).collect()
}

fn f32s(xs: impl IntoIterator<Item = f64>) -> Vec<f32> {
    xs.into_iter().map(|x| x as f32).collect()
}

/// `m`, `l`, `c`, `h` for one path, at the emitter's three decimals.
pub fn path_operators(c: &mut Content, path: &[PathSegment]) {
    let p = |p: &[Num; 2]| [Operand::Num(round3(p[0].0)), Operand::Num(round3(p[1].0))];
    for segment in path {
        match segment {
            PathSegment::Move { to } => c.op(&p(to), "m"),
            PathSegment::Line { to } => c.op(&p(to), "l"),
            PathSegment::Curve { c1, c2, to } => c.op(&[p(c1), p(c2), p(to)].concat(), "c"),
            PathSegment::Close => c.op(&[], "h"),
        };
    }
}

/// A clip: the path, then `W n` or `W* n`.
pub fn clip_operators(c: &mut Content, path: &[PathSegment], rule: FillRule) {
    path_operators(c, path);
    c.op(&[], if rule == FillRule::Evenodd { "W*" } else { "W" }).op(&[], "n");
}

/// The bounds of a path as `[llx lly urx ury]`, or zeros for a path with no point.
fn bounds_of(path: &[PathSegment]) -> [f64; 4] {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for segment in path {
        let points: &[&[Num; 2]] = match segment {
            PathSegment::Move { to } | PathSegment::Line { to } => &[to],
            PathSegment::Curve { c1, c2, to } => &[c1, c2, to],
            PathSegment::Close => &[],
        };
        for p in points {
            b = [b[0].min(p[0].0), b[1].min(p[1].0), b[2].max(p[0].0), b[3].max(p[1].0)];
        }
    }
    if b[0].is_finite() { b } else { [0.0; 4] }
}

fn space_name(space: ShadingSpace) -> &'static str {
    match space {
        ShadingSpace::DeviceRgb => "DeviceRGB",
        ShadingSpace::DeviceCmyk => "DeviceCMYK",
    }
}

fn shading_type(g: &Gradient) -> i32 {
    match g.gradient_type {
        GradientType::Axial => 2,
        GradientType::Radial => 3,
    }
}

/// The colour components one stop contributes in the shading's space.
fn components(rgb: &[Num; 3], cmyk: Option<&[Num; 4]>, alpha: f64, space: &str) -> Result<Vec<f64>, Refusal> {
    Ok(match space {
        "DeviceGray" => vec![round4(alpha)],
        "DeviceCMYK" => cmyk.ok_or_else(|| Refusal::new("shading: a DeviceCMYK shading has a stop with no device-cmyk() colour".into()))?.iter().map(|c| round4(c.0)).collect(),
        _ => rgb.iter().map(|c| round4(c.0)).collect(),
    })
}

fn join(xs: impl IntoIterator<Item = f64>, sep: &str) -> String {
    xs.into_iter().map(js_string).collect::<Vec<_>>().join(sep)
}

impl<'a> Vectors<'a> {
    pub fn new(pdf: &'a mut Pdf, next: &'a mut Ref, list: &'a DisplayList, images: &'a Images) -> Self {
        Self { pdf, next, list, images, shadings: Vec::new(), patterns: Vec::new(), states: Vec::new(), forms: 0, shared: Vec::new(), paint: Vec::new() }
    }

    /// The `vectorResources` record, as JSON.
    pub fn counts(&self) -> String {
        format!("{{\"shadings\":{},\"patterns\":{},\"extGStates\":{},\"forms\":{}}}", self.shadings.len(), self.patterns.len(), self.states.len(), self.forms)
    }

    /// Every operator one top-level vector item contributes to a page, its names recorded into the page's scope; every row it produced carries the item's chain.
    ///
    /// # Errors
    /// A raster fallback with no pixels, a form item naming no form, a gradient with fewer than two stops or a CMYK gradient with a stop with no CMYK colour.
    pub fn item(&mut self, c: &mut Content, node: Node, scope: &mut Scope, artifact: bool) -> Result<(), Refusal> {
        let p = node.painted();
        let info = Info { owner: p.owner, page: p.origin.page.map(|n| n.0), paint_index: p.origin.paint_index.map(|n| n.0), artifact };
        let from = self.paint.len();
        self.emit(c, node, scope, &info)?;
        if let Some(cm) = p.transform {
            let chain = format!(",\"chainCm\":{}", nums(cm, round3));
            for row in &mut self.paint[from..] {
                row.insert_str(row.len() - 1, &chain);
            }
        }
        Ok(())
    }

    /// One row of the record: the node's path, the item's owner, page and paint index, then `extra`, absent entries left out.
    fn row(&mut self, path: &str, info: &Info, kind: &str, label: String, extra: &[(&str, String)]) {
        let mut members = vec![("path", st(path)), ("owner", opt_st(info.owner)), ("kind", st(kind)), ("label", st(&label)), ("page", info.page.map_or(String::new(), n)), ("paintIndex", info.paint_index.map_or(String::new(), n)), ("artifact", info.artifact.to_string())];
        members.extend_from_slice(extra);
        self.paint.push(object(&members));
    }

    /// `emitItem`: one node and everything under it.
    fn emit(&mut self, c: &mut Content, node: Node, scope: &mut Scope, info: &Info) -> Result<(), Refusal> {
        let p = node.painted();
        let origin = p.origin;
        c.op(&[], "q");
        // The in-page chain goes down before the clips: everything under the item was measured with its HTML ancestors' transforms neutralised.
        if let Some(m) = p.transform {
            c.op(&ops_num(m), "cm");
        }
        for (at, clip) in p.clips.iter().enumerate() {
            clip_operators(c, &clip.path, clip.rule);
            let reason = opt_st(origin.clip_reasons.as_ref().and_then(|r| r.get(at)).map(String::as_str));
            self.row(&origin.dom_path, info, "clip", format!("{}:clip", origin.label), &[("geometry", r3(&bounds_of(&clip.path))), ("reason", reason), ("evenOdd", (clip.rule == FillRule::Evenodd).to_string())]);
        }
        let mask = self.soft_mask_form(&p, info)?;
        let mask_type = |m: &SoftMask| if m.mask_type == MaskType::Alpha { "Alpha" } else { "Luminosity" };
        let with_backdrop = mask.zip(p.soft_mask).map(|(form, m)| (form, mask_type(m), m.backdrop.as_deref()));
        let without = mask.zip(p.soft_mask).map(|(form, m)| (form, mask_type(m), None));

        match node {
            Node::Group(g) => {
                if g.isolated == Some(true) {
                    let mut inner = Scope::default();
                    let mut content = Content::new();
                    for child in &g.items {
                        self.emit(&mut content, child.into(), &mut inner, info)?;
                    }
                    let (name, id) = self.transparency_form(&g.bbox, content, &inner);
                    put(&mut scope.xobjects, &name, id);
                    self.state_for(c, &p, scope, info, with_backdrop);
                    c.op(&[Operand::Name(&name)], "Do");
                    let params = object(&[("opacity", n(p.opacity.unwrap_or(1.0))), ("blend", st(p.blend.unwrap_or("Normal"))), ("children", g.items.len().to_string())]);
                    let reason = opt_st(origin.isolated_reason.as_deref());
                    self.row(&origin.dom_path, info, "group", origin.label.clone(), &[("geometry", nums(&g.bbox, round3)), ("reason", reason), ("form", st(&name)), ("params", params)]);
                } else {
                    self.state_for(c, &p, scope, info, with_backdrop);
                    for child in &g.items {
                        self.emit(c, child.into(), scope, info)?;
                    }
                }
            }
            Node::Form(f) => {
                let list = self.list;
                let definition = list.resources.forms.get(&f.form).ok_or_else(|| Refusal::new(format!("forme: a form item at {} names the form {}, which resources.forms does not hold", origin.dom_path, f.form)))?;
                let (name, id, reused) = match self.shared.iter().find(|(key, _, _)| key == &f.form) {
                    Some((_, name, id)) => (name.clone(), *id, true),
                    None => {
                        // One XObject per key: built for the first item that names it, and named after its build, as `sharedForm` names it.
                        let mut inner = Scope::default();
                        let mut content = Content::new();
                        for child in &definition.items {
                            self.emit(&mut content, child.into(), &mut inner, info)?;
                        }
                        let name = format!("Fx{}", self.shared.len());
                        let id = self.form_stream(&definition.bbox, content, &inner, false);
                        self.shared.push((f.form.clone(), name.clone(), id));
                        (name, id, false)
                    }
                };
                put(&mut scope.xobjects, &name, id);
                self.state_for(c, &p, scope, info, without);
                c.op(&ops_num(&f.matrix), "cm").op(&[Operand::Name(&name)], "Do");
                let reason = opt_st(origin.reason.as_deref());
                self.row(&origin.dom_path, info, "form", origin.label.clone(), &[("geometry", nums(&definition.bbox, round3)), ("ctm", nums(&f.matrix, round3)), ("reason", reason), ("form", st(&name)), ("params", object(&[("key", st(&f.form)), ("reused", reused.to_string())]))]);
            }
            Node::Raster(r) => {
                let list = self.list;
                let key = r.source.index().and_then(|k| u32::try_from(k).ok());
                let raster = key.and_then(|k| list.resources.rasters.get(&k)).ok_or_else(|| Refusal::new(format!("forme: a raster fallback at {} names rasters {}, which the resources do not hold", origin.dom_path, js_string(r.source.0))))?;
                let x = key.and_then(|k| self.images.of_raster.get(&k)).map(|&at| &self.images.xobjects[at]).ok_or_else(|| Refusal::new(format!("vectorEmit: the raster for {} ({}) was never embedded", origin.dom_path, raster.feature)))?;
                let (name, id, has_s_mask) = (x.name.clone(), x.id, x.info.has_s_mask);
                put(&mut scope.xobjects, &name, id);
                self.state_for(c, &p, scope, info, without);
                c.op(&ops_num(&raster.cm), "cm").op(&[Operand::Name(&name)], "Do");
                self.row(&origin.dom_path, info, "raster", origin.label.clone(), &[("geometry", nums(&raster.rect_pt, round3)), ("ctm", nums(&raster.cm, round3)), ("reason", st(&raster.reason)), ("feature", st(&raster.feature)), ("dpi", n(raster.dpi.0)), ("xobject", st(&name)), ("hasSMask", has_s_mask.to_string())]);
            }
            Node::Path(path) => self.path(c, path, &p, scope, info, with_backdrop)?,
        }
        c.op(&[], "Q");
        Ok(())
    }

    /// A path: its state, then its fill and stroke.
    fn path(&mut self, c: &mut Content, path: &PathItem, p: &Painted, scope: &mut Scope, info: &Info, mask: Option<Mask>) -> Result<(), Refusal> {
        let origin = p.origin;
        // The state goes on first, so it covers both the fill and the stroke.
        self.state_for(c, p, scope, info, mask);
        let even = path.rule == FillRule::Evenodd;
        let bounds = r3(&bounds_of(&path.path));
        match &path.fill {
            Some(Paint::Gradient(fill)) => {
                let g = &fill.gradient;
                let shading = self.shading(g, space_name(g.color_space))?;
                put(&mut scope.shadings, &shading.0, shading.1);
                let alpha = if g.has_alpha { Some(self.alpha_mask_form(g, &path.path, even, info, origin)?) } else { None };
                c.op(&[], "q");
                if let Some(form) = alpha {
                    let (name, id, _) = self.state(p.opacity, None, None, Some((form, "Luminosity", Some(&[Num(0.0)]))));
                    put(&mut scope.ext_g_states, &name, id);
                    c.op(&[Operand::Name(&name)], "gs");
                }
                clip_operators(c, &path.path, path.rule);
                c.op(&ops_num(&g.matrix), "cm").op(&[Operand::Name(&shading.0)], "sh").op(&[], "Q");
                self.row(&origin.dom_path, info, "shading", origin.label.clone(), &[("geometry", bounds.clone()), ("ctm", nums(&g.matrix, round3)), ("params", shading_record(g, &shading.0, shading.2, None)), ("reason", st("a gradient fill is `sh` inside a clip, never a shading pattern")), ("evenOdd", even.to_string())]);
            }
            Some(fill) => colour_operators(c, fill, false),
            None => {}
        }
        let flat_fill = path.fill.as_ref().filter(|f| !matches!(f, Paint::Gradient(_)));
        if let Some(stroke) = &path.stroke {
            stroke_state(c, stroke);
            if let Paint::Gradient(paint) = &stroke.paint {
                let g = &paint.gradient;
                let (pattern, id) = self.pattern(g)?;
                put(&mut scope.patterns, &pattern, id);
                c.op(&[Operand::Name("Pattern")], "CS").op(&[Operand::Name(&pattern)], "SCN");
                let shading = self.shading(g, space_name(g.color_space))?;
                self.row(&origin.dom_path, info, "stroke", format!("{}:gradient-stroke", origin.label), &[("geometry", bounds.clone()), ("ctm", nums(&g.matrix, round3)), ("params", shading_record(g, &shading.0, shading.2, Some(&pattern))), ("reason", st("a gradient stroke is a shading pattern whose /Matrix is the page-space CTM"))]);
            } else {
                colour_operators(c, &stroke.paint, true);
                let params = object(&[("widthPt", n(round3(stroke.width.0))), ("cap", n(stroke.cap.0)), ("join", n(stroke.join.0)), ("miterLimit", n(stroke.miter_limit.0)), ("dashPt", nums(&stroke.dash, round3)), ("nonScaling", origin.non_scaling.map_or(String::new(), |b| b.to_string()))]);
                self.row(&origin.dom_path, info, "stroke", origin.label.clone(), &[("geometry", bounds.clone()), ("params", params)]);
            }
        }
        let strokes = path.stroke.is_some();
        if flat_fill.is_some() || strokes {
            path_operators(c, &path.path);
            let fill_op = if even { "f*" } else { "f" };
            if flat_fill.is_some() && strokes && path.stroke_first == Some(true) {
                // `B` fills and then strokes, and no operator does it the other way round, so `paint-order: stroke` writes the path twice.
                c.op(&[], "S");
                path_operators(c, &path.path);
                c.op(&[], fill_op);
            } else if flat_fill.is_some() && strokes {
                c.op(&[], if even { "B*" } else { "B" });
            } else if flat_fill.is_some() {
                c.op(&[], fill_op);
            } else {
                c.op(&[], "S");
            }
        }
        if let Some(fill) = flat_fill {
            let kind = if origin.label.starts_with("decoration:") { "decoration" } else { "fill" };
            let rgb = if let Paint::Rgb(rgb) = fill { nums(&rgb.rgb, round3) } else { String::new() };
            let params = object(&[("rgb", rgb), ("opacity", n(round3(p.opacity.unwrap_or(1.0))))]);
            self.row(&origin.dom_path, info, kind, origin.label.clone(), &[("geometry", bounds), ("evenOdd", even.to_string()), ("params", params)]);
        }
        Ok(())
    }

    /// `stateFor`: the `gs` for the one `/ExtGState` an item's alpha, blend mode and mask ask for, or nothing when they ask for none.
    fn state_for(&mut self, c: &mut Content, p: &Painted, scope: &mut Scope, info: &Info, mask: Option<Mask>) {
        if p.opacity.is_none() && p.stroke_opacity.is_none() && p.blend.is_none() && mask.is_none() {
            return;
        }
        let smask = mask.is_some();
        let (name, id, params) = self.state(p.opacity, p.stroke_opacity, p.blend, mask);
        put(&mut scope.ext_g_states, &name, id);
        self.row(&p.origin.dom_path, info, if smask { "smask" } else { "group" }, format!("{}:state", p.origin.label), &[("extGState", st(&name)), ("params", params)]);
        c.op(&[Operand::Name(&name)], "gs");
    }

    /// One `/ExtGState`, shared by value: its name, its object and the record's params.
    fn state(&mut self, ca: Option<f64>, stroke_ca: Option<f64>, blend: Option<&str>, mask: Option<Mask>) -> (String, Ref, String) {
        let opt = |v: Option<f64>| v.map_or(String::new(), |v| js_string(round4(v)));
        let smask_key = mask.map_or(String::new(), |(form, ty, bc)| format!("{ty}:{} 0 R:{}", form.get(), bc.map_or(String::new(), |b| join(b.iter().map(|n| n.0), ","))));
        let key = format!("{}|{}|{}|{smask_key}", opt(ca), opt(stroke_ca), blend.unwrap_or(""));
        let params = object(&[("ca", ca.map_or(String::new(), |v| n(round4(v)))), ("CA", stroke_ca.map_or(String::new(), |v| n(round4(v)))), ("BM", opt_st(blend)), ("smask", opt_st(mask.map(|m| m.1)))]);
        if let Some(at) = self.states.iter().position(|(k, _)| k == &key) {
            return (format!("GT{at}"), self.states[at].1, params);
        }
        let id = self.next.bump();
        let mut d = self.pdf.indirect(id).dict();
        d.pair(Name(b"Type"), Name(b"ExtGState"));
        if let Some(v) = ca {
            d.pair(Name(b"ca"), round4(v) as f32);
        }
        if let Some(v) = stroke_ca {
            d.pair(Name(b"CA"), round4(v) as f32);
        }
        if let Some(b) = blend {
            d.pair(Name(b"BM"), Name(b.as_bytes()));
        }
        match mask {
            Some((form, ty, bc)) => {
                let mut m = d.insert(Name(b"SMask")).dict();
                m.pair(Name(b"Type"), Name(b"Mask")).pair(Name(b"S"), Name(ty.as_bytes())).pair(Name(b"G"), form);
                if let Some(bc) = bc {
                    m.insert(Name(b"BC")).array().items(f32s(bc.iter().map(|n| n.0)));
                }
            }
            // A state that sets alpha or a blend mode but no mask says so, rather than inherit an enclosing state's mask.
            None => {
                d.pair(Name(b"SMask"), Name(b"None"));
            }
        }
        d.finish();
        self.states.push((key, id));
        (format!("GT{}", self.states.len() - 1), id, params)
    }

    /// `softMaskForm`: a soft mask's own transparency group, or nothing for an item with none.
    fn soft_mask_form(&mut self, p: &Painted, info: &Info) -> Result<Option<Ref>, Refusal> {
        let Some(mask) = p.soft_mask else { return Ok(None) };
        let mut inner = Scope::default();
        let mut content = Content::new();
        for child in &mask.items {
            self.emit(&mut content, child.into(), &mut inner, info)?;
        }
        let (name, id) = self.transparency_form(&mask.bbox, content, &inner);
        let reason = opt_st(p.origin.mask_reason.as_deref());
        self.row(&p.origin.dom_path, info, "smask", format!("{}:mask", p.origin.label), &[("geometry", nums(&mask.bbox, round3)), ("reason", reason), ("form", st(&name))]);
        Ok(Some(id))
    }

    /// `alphaMaskForm`: the same geometry painted as a `/DeviceGray` shading of the stop alphas, into a transparency group an `/SMask` reads.
    fn alpha_mask_form(&mut self, g: &Gradient, path: &[PathSegment], even: bool, info: &Info, origin: &VectorOrigin) -> Result<Ref, Refusal> {
        let mut inner = Scope::default();
        let (alpha, alpha_id, domain) = self.shading(g, "DeviceGray")?;
        put(&mut inner.shadings, &alpha, alpha_id);
        let bbox = bounds_of(path);
        let mut content = Content::new();
        content.op(&[], "q");
        clip_operators(&mut content, path, if even { FillRule::Evenodd } else { FillRule::Nonzero });
        content.op(&ops_num(&g.matrix), "cm").op(&[Operand::Name(&alpha)], "sh").op(&[], "Q");
        let bbox_nums = bbox.map(Num);
        let (name, id) = self.transparency_form(&bbox_nums, content, &inner);
        self.row(&origin.dom_path, info, "smask", format!("{}:alpha-stops", origin.label), &[("geometry", r3(&bbox)), ("reason", st("the gradient has stops with alpha below one")), ("form", st(&name)), ("params", shading_record(g, &alpha, domain, None))]);
        Ok(id)
    }

    /// A transparency-group form, `/I true /K false`, each its own.
    fn transparency_form(&mut self, bbox: &BoxPt, content: Content, scope: &Scope) -> (String, Ref) {
        let name = format!("Fm{}", self.forms);
        self.forms += 1;
        (name, self.form_stream(bbox, content, scope, true))
    }

    /// A form XObject: its content deflated, its `/BBox` at four decimals, a transparency group when `group`, and its own `/Resources`.
    fn form_stream(&mut self, bbox: &BoxPt, content: Content, scope: &Scope, group: bool) -> Ref {
        let id = self.next.bump();
        let bytes = crate::fonts::deflate(&content.finish());
        let mut s = self.pdf.stream(id, &bytes);
        s.filter(pdf_writer::Filter::FlateDecode);
        s.pair(Name(b"Type"), Name(b"XObject")).pair(Name(b"Subtype"), Name(b"Form")).pair(Name(b"FormType"), 1);
        s.insert(Name(b"BBox")).array().items(f32s(bbox.iter().map(|n| round4(n.0))));
        if group {
            s.insert(Name(b"Group")).dict().pair(Name(b"S"), Name(b"Transparency")).pair(Name(b"I"), true).pair(Name(b"K"), false).pair(Name(b"CS"), Name(b"DeviceRGB"));
        }
        let mut resources = s.insert(Name(b"Resources")).dict();
        for (key, map) in [(&b"Shading"[..], &scope.shadings), (b"Pattern", &scope.patterns), (b"ExtGState", &scope.ext_g_states), (b"XObject", &scope.xobjects)] {
            if !map.is_empty() {
                let mut d = resources.insert(Name(key)).dict();
                for (name, id) in map {
                    d.pair(Name(name.as_bytes()), *id);
                }
            }
        }
        id
    }

    /// One shading over its stitching function in `space`, shared by value: its name, its object and its `/Domain`.
    fn shading(&mut self, g: &Gradient, space: &'static str) -> Result<(String, Ref, [f64; 2]), Refusal> {
        let stops_key = g.stops.iter().map(|s| format!("{}:{}:{}:{}", js_string(round4(s.offset.0)), join(s.rgb.iter().map(|c| round4(c.0)), "/"), s.cmyk.map_or(String::new(), |k| join(k.iter().map(|c| round4(c.0)), "/")), js_string(round4(s.alpha.0)))).collect::<Vec<_>>().join(";");
        let key = format!("{space}|{}|{}|{},{}|{stops_key}", shading_type(g), join(g.coords.iter().map(|c| round4(c.0)), ","), g.extend[0], g.extend[1]);
        if let Some(s) = self.shadings.iter().find(|s| s.key == key) {
            return Ok((s.name.clone(), s.id, s.domain));
        }
        // Sorted by offset, stably, as `Array.prototype.sort` sorts: an insertion sort, since a gradient has a handful of stops.
        let mut stops: Vec<_> = g.stops.iter().collect();
        for i in 1..stops.len() {
            let mut j = i;
            while j > 0 && stops[j - 1].offset.0 > stops[j].offset.0 {
                stops.swap(j - 1, j);
                j -= 1;
            }
        }
        if stops.len() < 2 {
            return Err(Refusal::new("shading: a gradient needs at least two stops".into()));
        }
        let t0 = stops[0].offset.0;
        let mut t1 = stops[stops.len() - 1].offset.0;
        if t1 - t0 < 1e-9 {
            t1 = t0 + 1e-6;
        }
        // A hard stop is two stops at one offset; `/Bounds` must increase strictly, so the second is nudged by a millionth of the domain.
        let epsilon = (t1 - t0) * 1e-6;
        let mut offsets: Vec<f64> = stops.iter().map(|s| s.offset.0).collect();
        for i in 1..offsets.len().saturating_sub(1) {
            if offsets[i] <= offsets[i - 1] {
                offsets[i] = offsets[i - 1] + epsilon;
            }
        }
        // A fully transparent stop takes its neighbour's colour toward the side being interpolated, as premultiplied interpolation does.
        let mut segments = Vec::new();
        let mut bounds = Vec::new();
        for i in 0..stops.len() - 1 {
            let (a, b) = (stops[i], stops[i + 1]);
            let from = if a.alpha.0 == 0.0 { components(&b.rgb, b.cmyk.as_ref(), 0.0, space)? } else { components(&a.rgb, a.cmyk.as_ref(), a.alpha.0, space)? };
            let to = if b.alpha.0 == 0.0 { components(&a.rgb, a.cmyk.as_ref(), 0.0, space)? } else { components(&b.rgb, b.cmyk.as_ref(), b.alpha.0, space)? };
            segments.push((from, to));
            if i > 0 {
                bounds.push(round6(offsets[i]));
            }
        }
        let domain = [round6(t0), round6(t1)];
        let function = self.next.bump();
        let mut f = self.pdf.indirect(function).dict();
        f.pair(Name(b"FunctionType"), 3);
        f.insert(Name(b"Domain")).array().items(f32s(domain));
        let mut functions = f.insert(Name(b"Functions")).array();
        for (from, to) in &segments {
            let mut s = functions.push().dict();
            s.pair(Name(b"FunctionType"), 2);
            s.insert(Name(b"Domain")).array().items([0, 1]);
            s.insert(Name(b"C0")).array().items(f32s(from.iter().copied()));
            s.insert(Name(b"C1")).array().items(f32s(to.iter().copied()));
            s.pair(Name(b"N"), 1);
        }
        functions.finish();
        f.insert(Name(b"Bounds")).array().items(f32s(bounds));
        f.insert(Name(b"Encode")).array().items(segments.iter().flat_map(|_| [0, 1]));
        f.finish();
        let id = self.next.bump();
        let mut d = self.pdf.indirect(id).dict();
        d.pair(Name(b"ShadingType"), shading_type(g)).pair(Name(b"ColorSpace"), Name(space.as_bytes()));
        d.insert(Name(b"Coords")).array().items(f32s(g.coords.iter().map(|c| round4(c.0))));
        d.insert(Name(b"Domain")).array().items(f32s(domain));
        d.insert(Name(b"Extend")).array().items(g.extend);
        d.pair(Name(b"Function"), function);
        d.finish();
        let name = format!("Sh{}", self.shadings.len());
        self.shadings.push(Shading { key, name: name.clone(), id, domain });
        Ok((name, id, domain))
    }

    /// A type 2 pattern for a gradient stroke, its `/Matrix` the gradient's, shared by shading and matrix.
    fn pattern(&mut self, g: &Gradient) -> Result<(String, Ref), Refusal> {
        let (shading, shading_id, _) = self.shading(g, space_name(g.color_space))?;
        let matrix: Vec<f64> = g.matrix.iter().map(|m| round4(m.0)).collect();
        let key = format!("{shading}|{}", join(matrix.iter().copied(), ","));
        if let Some(at) = self.patterns.iter().position(|(k, _)| k == &key) {
            return Ok((format!("P{at}"), self.patterns[at].1));
        }
        let id = self.next.bump();
        let mut d = self.pdf.indirect(id).dict();
        d.pair(Name(b"Type"), Name(b"Pattern")).pair(Name(b"PatternType"), 2).pair(Name(b"Shading"), shading_id);
        d.insert(Name(b"Matrix")).array().items(f32s(matrix));
        d.finish();
        self.patterns.push((key, id));
        Ok((format!("P{}", self.patterns.len() - 1), id))
    }
}

/// `w J j M d` for one stroke.
fn stroke_state(c: &mut Content, s: &Stroke) {
    c.op(&[Operand::Num(round3(s.width.0))], "w");
    c.op(&[Operand::Num(s.cap.0)], "J");
    c.op(&[Operand::Num(s.join.0)], "j");
    c.op(&[Operand::Num(round3(s.miter_limit.0))], "M");
    let dash = format!("[{}]", join(s.dash.iter().map(|d| round3(d.0)), " "));
    c.op(&[Operand::Raw(&dash), Operand::Num(round3(s.dash_phase.0))], "d");
}

/// A flat colour as the non-stroking or stroking colour, at three decimals; a gradient sets none here.
fn colour_operators(c: &mut Content, paint: &Paint, stroking: bool) {
    match paint {
        Paint::Rgb(p) => {
            c.op(&ops_num(&p.rgb), if stroking { "RG" } else { "rg" });
        }
        Paint::Cmyk(p) => {
            c.op(&ops_num(&p.cmyk), if stroking { "K" } else { "k" });
        }
        Paint::Gradient(_) => {}
    }
}

/// `shadingRecord`: the shading's params as the report records them, with its resource names.
fn shading_record(g: &Gradient, shading: &str, domain: [f64; 2], pattern: Option<&str>) -> String {
    let stops = array(g.stops.iter().map(|s| object(&[("offset", n(round4(s.offset.0))), ("rgb", nums(&s.rgb, round4)), ("cmyk", s.cmyk.as_ref().map_or(String::new(), |k| nums(k, round4))), ("alpha", n(round4(s.alpha.0)))])));
    let spread = match g.spread {
        Spread::Pad => "pad",
        Spread::Reflect => "reflect",
        Spread::Repeat => "repeat",
    };
    object(&[
        ("type", shading_type(g).to_string()), ("coords", nums(&g.coords, round3)), ("coordsPt", nums(&g.coords_pt, round3)), ("extend", format!("[{},{}]", g.extend[0], g.extend[1])), ("domain", format!("[{},{}]", n(domain[0]), n(domain[1]))),
        ("stops", stops), ("colorSpace", st(space_name(g.color_space))), ("matrix", nums(&g.matrix, round3)), ("spread", st(spread)), ("periods", n(g.periods.0)), ("shading", st(shading)), ("pattern", opt_st(pattern)), ("source", st(&g.source)),
    ])
}
