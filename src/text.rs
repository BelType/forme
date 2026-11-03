//! The page's content: the marked-content skeleton and the text, ported from Caster's `src/emit/textEmit.ts` (`emitRun`, `lineWrap`) and the page loop of `src/emit/structureEmit.ts`.
//!
//! - **The skeleton** is the list's tags: a content start is `/<Tag> <</MCID n>> BDC`, its MCIDs numbered 0, 1, 2 … in item order on each page; an artifact start is `/Artifact <</Type /T /Subtype /S>> BDC`; an end is `EMC`, closing the line open inside it first.
//! - **A line is one text object**: `BT … ET` per `lineIndex`, wrapped in `q`, the line's `cm` and its clips, and `Q`, when it has either; a vector item closes the open line before its geometry, as the oracle does, while an image, a rectangle and the print marks do not.
//! - **A run is `Tf`, `rg`, `Tc`, `Tm` and one `TJ`**: each segment's glyphs as one hex string of CIDs, a space glyph where DOM whitespace separated two segments, and between them the number that moves the pen to where the browser put the next. The pen is tracked in the renderer's terms, the `/W` entry plus `Tc` per glyph, and every glyph whose shaped advance differs from its `/W` entry by more than [`KERN_EPSILON`] thousandths carries the difference as its own adjustment.
//! - **`/ActualText`**: an occurrence of a glyph that stands for other text than the ToUnicode map gives it is wrapped in `/Span <</ActualText <FEFF…> >> BDC … EMC`, the array closed before it and opened after.
//! - **The arithmetic is the oracle's**, operation for operation in its order, on doubles, rounded by [`round3`]; Rust neither reorders nor fuses floating-point operations, so the pen is the oracle's to the bit.

use pdf_writer::Ref;
use serde::Serialize;

use crate::Refusal;
use crate::content::{Content, Operand, round3};
use crate::fonts::{Embedded, segments};
use crate::list::{ArtifactType, ClipPath, DisplayList, FillRule, Item, Matrix, Num, PathSegment, TagStart, TextItem};

/// The largest gap, in thousandths of an em, between the shaped advance and the `/W` entry that is rounding rather than a kern, the oracle's `KERN_EPSILON`.
pub const KERN_EPSILON: f64 = 2.0;

/// The pen adjustment written before each segment of one text item, in thousandths of an em, `None` for the first: Caster's `WriterPen`.
#[derive(Clone, Debug, PartialEq, Serialize)]
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
}

/// One page's content stream, uncompressed, and the item kinds it left out.
pub struct PageContent {
    pub bytes: Vec<u8>,
    pub left_out: Vec<&'static str>,
}

/// The page loop over every page: each page's stream, and the records.
///
/// # Errors
/// A colour-cluster item, which forme refuses by name (the plan's rulings); a content start naming no element, in the oracle's sentence; and, unless `partial`, every item kind this stage does not write.
pub fn pages(list: &DisplayList, fonts: &[Embedded], partial: bool) -> Result<(Vec<PageContent>, Counts), Refusal> {
    let mut counts = Counts::default();
    let mut out = Vec::new();
    for (p, page) in list.pages.iter().enumerate() {
        let mut c = Content::new();
        let mut mcids = Vec::new();
        let mut left_out = Vec::new();
        let mut open: Option<&TextItem> = None;
        let mut open_index = f64::NAN;
        for (at, item) in page.items.iter().enumerate() {
            match item {
                Item::TagEnd(_) => {
                    close_line(&mut c, &mut open);
                    c.op(&[], "EMC");
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
                    }
                    TagStart::Content(start) => {
                        let elements = &list.structure.elements;
                        let element = start.element.index().and_then(|i| elements.get(i)).ok_or_else(|| Refusal::new(format!("caster: the marked content at item {at} of page {} names structure element {}, and the list's tree has {}", p + 1, crate::content::js_string(start.element.0), elements.len())))?;
                        let props = format!("<</MCID {}>>", mcids.len());
                        mcids.push(at);
                        c.op(&[Operand::Name(&element.element_type), Operand::Raw(&props)], "BDC");
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
                    let adjustments = run(&mut c, text, &fonts[font], &format!("F{font}"), upem, &mut counts);
                    counts.pens.push(Pen { page: p, item: at, adjustments });
                }
                Item::Raster(_) => return Err(Refusal::at(format!("forme: item {at} of page {} is a colour-cluster raster, which forme does not write; the colour-cluster road is dormant in print()", p + 1), p, at)),
                // Not painted: the structure tree collects a link (stage 4).
                Item::Link(_) => left_out.push("link"),
                // A path may not sit inside a text object, so a vector item closes the open line first, as the oracle's page loop does; an image, a rectangle and the marks do not.
                Item::Group(_) | Item::Path(_) | Item::Form(_) | Item::RasterFallback(_) => {
                    close_line(&mut c, &mut open);
                    left_out.push(item.kind());
                }
                Item::Image(_) | Item::Rect(_) | Item::Marks(_) => left_out.push(item.kind()),
            }
            if !partial && let Some(&kind) = left_out.last() {
                return Err(Refusal::at(format!("forme: item {at} of page {} is a {kind} item, which this stage of forme does not write yet", p + 1), p, at));
            }
        }
        counts.mcids.push(mcids);
        out.push(PageContent { bytes: c.finish(), left_out });
    }
    Ok((out, counts))
}

/// The resource names a page gives its fonts: `F0`, `F1`, … in the order of `resources.fonts`, with their objects.
pub fn font_names(fonts: &[Embedded]) -> Vec<(String, Ref)> {
    fonts.iter().enumerate().map(|(i, e)| (format!("F{i}"), e.font)).collect()
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
        let n = |p: &[Num; 2]| [Operand::Num(round3(p[0].0)), Operand::Num(round3(p[1].0))];
        for segment in &clip.path {
            match segment {
                PathSegment::Move { to } => c.op(&n(to), "m"),
                PathSegment::Line { to } => c.op(&n(to), "l"),
                PathSegment::Curve { c1, c2, to } => c.op(&[n(c1), n(c2), n(to)].concat(), "c"),
                PathSegment::Close => c.op(&[], "h"),
            };
        }
        c.op(&[], if clip.rule == FillRule::Evenodd { "W*" } else { "W" });
        c.op(&[], "n");
    }
}

/// One element of a `TJ` array being built.
enum Shown {
    Glyphs(Vec<u8>),
    Adjust(f64),
}

/// `emitRun`: one run's state operators and its `TJ`, appended to the page; the pen adjustment written before each segment, the writer's record of the item.
fn run(c: &mut Content, item: &TextItem, font: &Embedded, name: &str, upem: f64, counts: &mut Counts) -> Vec<Option<Num>> {
    let size = item.size.0;
    let tc = item.letter_spacing.0;
    let space = item.space.0 as u16;
    let width = |gid: u16| font.widths.get(&gid).copied().unwrap_or(0.0);
    let cids = |gids: &[u16]| -> Vec<u8> { gids.iter().flat_map(|g| font.cids.get(g).copied().unwrap_or(0).to_be_bytes()).collect() };
    let space_adv = (width(space) / 1000.0) * size;
    let start = round3(item.segments.x[0].0 - if item.leading_space { space_adv + tc } else { 0.0 });
    c.op(&[Operand::Name(name), Operand::Num(round3(size))], "Tf");
    c.op(&item.fill.rgb.map(|v| Operand::Num(round3(v.0))), "rg");
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
    if item.leading_space {
        shown.push(Shown::Glyphs(cids(&[space])));
        pen += space_adv + tc;
    }
    let mut adjustments = Vec::new();
    for (index, segment) in segments(item).iter().enumerate() {
        let mut emitted = None;
        if index > 0 {
            let next = item.segments.x[index].0;
            let adj = if segment.space_before { round3((-(next - pen - space_adv - tc) / size) * 1000.0) } else { round3((-(next - pen) / size) * 1000.0) };
            if segment.space_before {
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
    adjustments
}
