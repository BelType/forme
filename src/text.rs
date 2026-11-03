//! The page's content: the marked-content skeleton and the text, ported from Caster's `src/emit/textEmit.ts` (`emitRun`, `lineWrap`) and the page loop of `src/emit/structureEmit.ts`.
//!
//! - **The skeleton** is the list's tags: a content start is `/<Tag> <</MCID n>> BDC`, its MCIDs numbered 0, 1, 2 … in item order on each page; an artifact start is `/Artifact <</Type /T /Subtype /S>> BDC`; an end is `EMC`, closing the line open inside it first.
//! - **A line is one text object**: `BT … ET` per `lineIndex`, wrapped in `q`, the line's `cm` and its clips, and `Q`, when it has either; every item that paints anything but text closes the open line, the next run opening a new one, and a page end closes it too.
//! - **A vector item** is [`crate::vectors`]'s, painted as an artifact when it sits in an artifact's marked content, its names recorded into the page's scope.
//! - **A run is `Tf`, `rg`, `Tc`, `Tm` and one `TJ`**: each segment's glyphs as one hex string of CIDs, a space glyph where DOM whitespace separated two segments, and between them the number that moves the pen to where the browser put the next. The pen is tracked in the renderer's terms, the `/W` entry plus `Tc` per glyph, and every glyph whose shaped advance differs from its `/W` entry by more than [`KERN_EPSILON`] thousandths carries the difference as its own adjustment.
//! - **An image** is `imageOperators`: `q`, its clip at the list's numbers with `W n`, the `cm` that puts the unit square on the painted rectangle, `/Im<n> Do`, `Q`; the page names each XObject it paints, and the use is recorded. A facsimile's form is drawn in its own units, so its `cm` is scaled down to the unit square first (`formSize`).
//! - **`/ActualText`**: an occurrence of a glyph that stands for other text than the ToUnicode map gives it is wrapped in `/Span <</ActualText <FEFF…> >> BDC … EMC`, the array closed before it and opened after.
//! - **The arithmetic is the oracle's**, operation for operation in its order, on doubles, rounded by [`round3`]; Rust neither reorders nor fuses floating-point operations, so the pen is the oracle's to the bit.

use pdf_writer::Ref;

use crate::Refusal;
use crate::colour::{Colour, PaintOps};
use crate::content::{Content, Operand, round3};
use crate::fonts::{Embedded, segments};
use crate::images::{ImageUse, Images};
use crate::list::{ArtifactType, ClipPath, DisplayList, ImageItem, Item, Matrix, Num, PathSegment, RectItem, TagStart, TextItem};
use crate::vectors::{Node, Scope, Vectors, clip_operators};

/// The largest gap, in thousandths of an em, between the shaped advance and the `/W` entry that is rounding rather than a kern, the oracle's `KERN_EPSILON`.
pub const KERN_EPSILON: f64 = 2.0;

/// The pen adjustment written before each segment of one text item, in thousandths of an em, `None` for the first: Caster's `WriterPen`.
#[derive(Clone, Debug, PartialEq)]
pub struct Pen {
    pub page: usize,
    pub item: usize,
    pub adjustments: Vec<Option<Num>>,
}

/// What the page loop counts beside the streams.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    /// Per page, the index of the item each MCID opens, by MCID.
    pub mcids: Vec<Vec<usize>>,
    pub pens: Vec<Pen>,
    pub kern_adjustments: usize,
    pub actual_text_spans: usize,
    /// One entry per image item painted, in painting order.
    pub images: Vec<ImageUse>,
}

/// One page's content stream, uncompressed, and the XObjects it names.
pub struct PageContent {
    pub bytes: Vec<u8>,
    /// Each XObject the page paints, by its resource name, in the order of first use.
    pub xobjects: Vec<(String, Ref)>,
    /// The vector items' resources: shadings, patterns, graphics states and form and raster XObjects.
    pub scope: Scope,
}

/// The page loop over every page: each page's stream, and the records.
///
/// # Errors
/// A colour-cluster item, which forme refuses by name (the plan's rulings); a content start naming no element, in the oracle's sentence.
pub fn pages(list: &DisplayList, fonts: &[Embedded], images: &Images, mut colour: Option<&mut Colour>, vectors: &mut Vectors) -> Result<(Vec<PageContent>, Counts), Refusal> {
    let mut counts = Counts::default();
    let mut out = Vec::new();
    for (p, page) in list.pages.iter().enumerate() {
        let mut c = Content::new();
        let mut mcids = Vec::new();
        let mut xobjects: Vec<(String, Ref)> = Vec::new();
        let mut scope = Scope::default();
        let mut open: Option<&TextItem> = None;
        let mut open_index = f64::NAN;
        // Whether the item being written sits in an artifact's marked content, for the paint record.
        let mut artifact = false;
        for (at, item) in page.items.iter().enumerate() {
            // Only text-object operators may sit inside `BT … ET` (ISO 32000-2, 9.4), so every item that paints anything but text closes the open line, as a line end does, and the next run opens a new one as a fresh line would.
            if !matches!(item, Item::TagStart(_) | Item::TagEnd(_) | Item::Text(_) | Item::Link(_)) {
                close_line(&mut c, &mut open);
            }
            match item {
                Item::TagEnd(_) => {
                    close_line(&mut c, &mut open);
                    c.op(&[], "EMC");
                    artifact = false;
                }
                Item::TagStart(start) => match &start.start {
                    TagStart::Artifact(a) => {
                        let ty = match a.artifact.artifact_type {
                            ArtifactType::Pagination => "Pagination",
                            ArtifactType::Layout => "Layout",
                            ArtifactType::Page => "Page",
                        };
                        let props = match &a.artifact.subtype {
                            Some(sub) => format!("<</Type /{ty} /Subtype /{sub}>>"),
                            None => format!("<</Type /{ty}>>"),
                        };
                        c.op(&[Operand::Name("Artifact"), Operand::Raw(&props)], "BDC");
                        artifact = true;
                    }
                    TagStart::Content(start) => {
                        let elements = &list.structure.elements;
                        let element = start.element.index().and_then(|i| elements.get(i)).ok_or_else(|| Refusal::new(format!("caster: the marked content at item {at} of page {} names structure element {}, and the list's tree has {}", p + 1, crate::content::js_string(start.element.0), elements.len())))?;
                        let props = format!("<</MCID {}>>", mcids.len());
                        mcids.push(at);
                        c.op(&[Operand::Name(&element.element_type), Operand::Raw(&props)], "BDC");
                        artifact = false;
                    }
                },
                Item::Text(text) => {
                    if open.is_none() || text.line_index.0 != open_index {
                        close_line(&mut c, &mut open);
                        line_open(&mut c, text.transform.as_ref(), text.clips.as_deref().unwrap_or(&[]));
                        c.op(&[], "BT");
                        open = Some(text);
                        open_index = text.line_index.0;
                    }
                    let font = list.resources.fonts.iter().position(|f| f.key == text.font).ok_or_else(|| Refusal::at(format!("caster: a text item names the font {}, and the display list holds no instance under that key", text.font), p, at))?;
                    let upem = list.resources.fonts[font].upem.map_or(f64::NAN, |u| u.0);
                    let ops = colour.as_deref_mut().map(|k| k.run(text, p, at)).transpose()?;
                    let adjustments = run(&mut c, text, &fonts[font], &format!("F{font}"), upem, ops.as_ref(), &mut counts);
                    counts.pens.push(Pen { page: p, item: at, adjustments });
                }
                Item::Raster(_) => return Err(Refusal::at(format!("forme: item {at} of page {} is a colour-cluster raster, which forme does not write; the colour-cluster road is dormant in print()", p + 1), p, at)),
                // Not painted: the structure tree writes a link's annotation.
                Item::Link(_) => {}
                Item::Group(_) | Item::Path(_) | Item::Form(_) | Item::RasterFallback(_) => {
                    let node = match item {
                        Item::Group(x) => Node::Group(x),
                        Item::Path(x) => Node::Path(x),
                        Item::Form(x) => Node::Form(x),
                        Item::RasterFallback(x) => Node::Raster(x),
                        _ => unreachable!("matched above"),
                    };
                    vectors.item(&mut c, node, &mut scope, artifact)?;
                }
                Item::Image(image) => match images.of_source.get(&(image.source.0 as u32)) {
                    Some(&at_x) => {
                        let x = &images.xobjects[at_x];
                        image_operators(&mut c, image, &x.name, (x.info.path == "form").then_some((x.info.width, x.info.height)));
                        if !xobjects.iter().any(|(name, _)| name == &x.name) {
                            xobjects.push((x.name.clone(), x.id));
                        }
                        counts.images.push(ImageUse { page: p, item: at, source: image.source.0 as u32, name: x.name.clone(), hash: x.hash.clone(), info: x.info.clone() });
                    }
                    None => return Err(Refusal::at(format!("forme: the image at item {at} of page {} names source {}, which no XObject was made for", p + 1, crate::content::js_string(image.source.0)), p, at)),
                },
                Item::Rect(rect) => {
                    let ops = match colour.as_deref_mut() {
                        Some(k) => k.rect(rect)?,
                        None => None,
                    };
                    rect_operators(&mut c, rect, ops.as_ref());
                }
                Item::Marks(marks) => match colour.as_deref_mut() {
                    Some(k) => k.marks(&mut c, marks)?,
                    None => return Err(Refusal::at("caster: print marks are stroked in the /All separation, which needs the colour options".into(), p, at)),
                },
            }
        }
        // A page end closes an open line too.
        close_line(&mut c, &mut open);
        counts.mcids.push(mcids);
        out.push(PageContent { bytes: c.finish(), xobjects, scope });
    }
    Ok((out, counts))
}

/// The resource names a page gives its fonts: `F0`, `F1`, … in the order of `resources.fonts`, with their objects.
pub fn font_names(fonts: &[Embedded]) -> Vec<(String, Ref)> {
    fonts.iter().enumerate().map(|(i, e)| (format!("F{i}"), e.font)).collect()
}

/// `imageOperators`: `q`, the clip's path in page points as the list holds it with `W n`, the `cm` that puts the unit square on the painted rectangle, `Do`, `Q`. A Form XObject of `form_size` points is drawn in its own units, so the matrix is `mul([1 / w, 0, 0, 1 / h, 0, 0], transform)`, operation for operation.
fn image_operators(c: &mut Content, image: &ImageItem, name: &str, form_size: Option<(f64, f64)>) {
    c.op(&[], "q");
    if let Some(clip) = &image.clip {
        let n = |p: &[Num; 2]| [Operand::Num(p[0].0), Operand::Num(p[1].0)];
        for segment in clip {
            match segment {
                PathSegment::Move { to } => c.op(&n(to), "m"),
                PathSegment::Line { to } => c.op(&n(to), "l"),
                PathSegment::Curve { c1, c2, to } => c.op(&[n(c1), n(c2), n(to)].concat(), "c"),
                PathSegment::Close => c.op(&[], "h"),
            };
        }
        c.op(&[], "W");
        c.op(&[], "n");
    }
    let t = image.transform.map(|n| n.0);
    let cm = match form_size {
        Some((w, h)) => {
            let a = [1.0 / w, 0.0, 0.0, 1.0 / h, 0.0, 0.0];
            [a[0] * t[0] + a[1] * t[2], a[0] * t[1] + a[1] * t[3], a[2] * t[0] + a[3] * t[2], a[2] * t[1] + a[3] * t[3], a[4] * t[0] + a[5] * t[2] + t[4], a[4] * t[1] + a[5] * t[3] + t[5]]
        }
        None => t,
    };
    c.op(&cm.map(Operand::Num), "cm");
    c.op(&[Operand::Name(name)], "Do");
    c.op(&[], "Q");
}

/// `rectOperators`: `q`, the overprint state, the paint (`rg` at thousandths for the ground or with no colour story), `re f`, the reset, `Q`.
fn rect_operators(c: &mut Content, rect: &RectItem, ops: Option<&PaintOps>) {
    c.op(&[], "q");
    match ops {
        Some(ops) => {
            ops.write_setup(c);
            ops.write_paint(c);
        }
        None => {
            c.op(&rect.fill.rgb.map(|v| Operand::Num(round3(v.0))), "rg");
        }
    }
    c.op(&rect.rect.map(|n| Operand::Num(n.0)), "re").op(&[], "f");
    if let Some(ops) = ops {
        ops.write_reset(c);
    }
    c.op(&[], "Q");
}

/// `ET` and the line's `Q`, when a line is open.
fn close_line(c: &mut Content, open: &mut Option<&TextItem>) {
    if let Some(line) = open.take() {
        c.op(&[], "ET");
        if line.transform.is_some() || line.clips.as_ref().is_some_and(|k| !k.is_empty()) {
            c.op(&[], "Q");
        }
    }
}

/// `lineWrap`'s opening half: `q`, the chain's `cm` as the builder made it, and each clip's path at thousandths with `W n` or `W* n`; nothing when the line has neither.
fn line_open(c: &mut Content, chain: Option<&Matrix>, clips: &[ClipPath]) {
    if chain.is_none() && clips.is_empty() {
        return;
    }
    c.op(&[], "q");
    if let Some(m) = chain {
        c.op(&m.map(|n| Operand::Num(n.0)), "cm");
    }
    for clip in clips {
        clip_operators(c, &clip.path, clip.rule);
    }
}

/// One element of a `TJ` array being built.
enum Shown {
    Glyphs(Vec<u8>),
    Adjust(f64),
}

/// `emitRun`: one run's state operators and its `TJ`, appended to the page; the pen adjustment written before each segment, the writer's record of the item.
fn run(c: &mut Content, item: &TextItem, font: &Embedded, name: &str, upem: f64, colour: Option<&PaintOps>, counts: &mut Counts) -> Vec<Option<Num>> {
    let size = item.size.0;
    let tc = item.letter_spacing.0;
    let space = item.space.0 as u16;
    let width = |gid: u16| font.widths.get(&gid).copied().unwrap_or(0.0);
    let cids = |gids: &[u16]| -> Vec<u8> { gids.iter().flat_map(|g| font.cids.get(g).copied().unwrap_or(0).to_be_bytes()).collect() };
    // A face with no space glyph has `space: 0`, and a list that claims nothing writes its gaps as positioning with no glyph (*PDF/UA-2 is a flag, not a given*, Caster's DESIGN.md S103; a list claiming PDF/UA-2 with such a gap was refused by the validator).
    let gap = space != 0;
    let leading = item.leading_space && gap;
    let space_adv = (width(space) / 1000.0) * size;
    let start = round3(item.segments.x[0].0 - if leading { space_adv + tc } else { 0.0 });
    if let Some(ops) = colour {
        ops.write_setup(c);
    }
    c.op(&[Operand::Name(name), Operand::Num(round3(size))], "Tf");
    match colour {
        Some(ops) => ops.write_paint(c),
        None => {
            c.op(&item.fill.rgb.map(|v| Operand::Num(round3(v.0))), "rg");
        }
    }
    c.op(&[Operand::Num(round3(tc))], "Tc");
    c.op(&[Operand::Num(1.0), Operand::Num(0.0), Operand::Num(0.0), Operand::Num(1.0), Operand::Num(start), Operand::Num(item.baseline.0)], "Tm");

    let mut shown: Vec<Shown> = Vec::new();
    let flush = |c: &mut Content, shown: &mut Vec<Shown>| {
        if shown.is_empty() {
            return;
        }
        let array = shown.iter().map(|s| match s {
            Shown::Glyphs(bytes) => Operand::Hex(bytes),
            Shown::Adjust(n) => Operand::Num(*n),
        }).collect();
        c.op(&[Operand::Array(array)], "TJ");
        shown.clear();
    };
    let mut pen = start;
    if leading {
        shown.push(Shown::Glyphs(cids(&[space])));
        pen += space_adv + tc;
    }
    let mut adjustments = Vec::new();
    for (index, segment) in segments(item).iter().enumerate() {
        let mut emitted = None;
        if index > 0 {
            let next = item.segments.x[index].0;
            let spaced = segment.space_before && gap;
            let adj = if spaced { round3((-(next - pen - space_adv - tc) / size) * 1000.0) } else { round3((-(next - pen) / size) * 1000.0) };
            if spaced {
                shown.push(Shown::Glyphs(cids(&[space])));
                pen += space_adv + tc;
            }
            shown.push(Shown::Adjust(adj));
            pen -= (adj / 1000.0) * size;
            emitted = Some(Num(adj));
        }
        let first = item.segments.first_glyph[index].0 as usize;
        let mut pending: Vec<u16> = Vec::new();
        let close = |shown: &mut Vec<Shown>, pending: &mut Vec<u16>| {
            if !pending.is_empty() {
                shown.push(Shown::Glyphs(cids(pending)));
                pending.clear();
            }
        };
        for (j, &gid) in segment.gids.iter().enumerate() {
            // `actualTextFor`: every occurrence whose text is not the one the map chose, a later member of a cluster (whose text is empty) included.
            let text = segment.text_for(j);
            let actual = (font.chosen.get(&gid).map(Vec::as_slice) != Some(text)).then_some(text);
            if let Some(text) = actual {
                close(&mut shown, &mut pending);
                flush(c, &mut shown);
                let hex: String = text.iter().map(|u| format!("{u:04X}")).collect();
                c.op(&[Operand::Name("Span"), Operand::Raw(&format!("<</ActualText <FEFF{hex}> >>"))], "BDC");
                counts.actual_text_spans += 1;
            }
            pending.push(gid);
            let shaped = (item.glyphs.advances[first + j].0 * 1000.0) / upem;
            let written = width(gid);
            let delta = shaped - written;
            pen += (written / 1000.0) * size + tc;
            if delta.abs() > KERN_EPSILON {
                close(&mut shown, &mut pending);
                let kern = round3(-delta);
                shown.push(Shown::Adjust(kern));
                pen -= (kern / 1000.0) * size;
                counts.kern_adjustments += 1;
            }
            if actual.is_some() {
                close(&mut shown, &mut pending);
                flush(c, &mut shown);
                c.op(&[], "EMC");
            }
        }
        close(&mut shown, &mut pending);
        adjustments.push(emitted);
    }
    flush(c, &mut shown);
    if let Some(ops) = colour {
        ops.write_reset(c);
    }
    adjustments
}
