//! The structure tree and everything that points into it, ported from `buildStructure` and the page-label block of Caster's `src/emit/structureEmit.ts` and from `src/emit/outline.ts`.
//!
//! - **The tree is the list's**: elements allocated in the array's order before any dictionary is written, each from its fields, `/K` from `children` in order, `/P` from the element that lists it, the PDF 2.0 namespace on every element. An element whose content is all on one page carries `/Pg` and bare MCIDs; content on two pages is named per item by `/MCR`. Attribute objects are written `/O` first and then in the schema's key order, every string a name but each `Headers` entry, a byte string; one object is `/A` itself, more are an array.
//! - **The ParentTree** is one flat number tree: one array per page indexed by MCID, keyed by the page's index (its `/StructParents`), then one entry per link, keyed by the page count plus the link's `order` (its `/StructParent`). The IDTree is one flat name tree, sorted by UTF-16 code unit, and two elements with one `/ID` are refused.
//! - **A link** is an annotation with `/Rect` the union of its `/QuadPoints`, `/F 4`, `/Border [0 0 0]`, `/Contents`, `/StructParent` and `/P`, and either a `/URI` action or a structure destination `[element /XYZ x y null]`, and an `/OBJR` in its element's `/K`.
//! - **The outline** nests by level, every item open, each destination `[element /XYZ null top null]`; `/PageLabels` opens with an empty dictionary when the first run is not on the first page.

use pdf_writer::{Finish, Name, Obj, Pdf, Ref, Str, TextStr};

use crate::Refusal;
use crate::list::{Attribute, DisplayList, Item, LinkItem, StructChild};
use crate::read::Value;

/// The validator's `ATTRIBUTE_KEYS`: the order an attribute object's keys are written in.
const ATTRIBUTE_KEYS: [&str; 8] = ["O", "ListNumbering", "BBox", "Scope", "Headers", "RowSpan", "ColSpan", "TextPosition"];

/// The PDF 2.0 standard structure namespace; every element points at it.
pub const PDF_2_NAMESPACE: &str = "http://iso.org/pdf2/ssn";

/// What the catalog and the pages need of the tree.
pub struct Tree {
    pub root: Ref,
    pub outlines: Option<Ref>,
    pub page_labels: Option<Ref>,
    /// Per page, its link annotations in `order`.
    pub annots: Vec<Vec<Ref>>,
}

/// Write the tree, the ParentTree, the IDTree, the link annotations, the outline and the page labels.
///
/// # Errors
/// Two elements with one `/ID`, in the oracle's sentence. The tree's shape and every index it holds are the validator's, checked before.
pub fn write(pdf: &mut Pdf, next: &mut Ref, list: &DisplayList, pages: &[Ref], mcids: &[Vec<usize>]) -> Result<Tree, Refusal> {
    let elements = &list.structure.elements;
    let page_count = list.pages.len();
    let mcid_of = |page: usize, item: usize| mcids[page].iter().position(|&at| at == item).unwrap_or(0) as i32;
    let mut parent: Vec<Option<usize>> = vec![None; elements.len()];
    for (index, element) in elements.iter().enumerate() {
        for child in &element.children {
            if let StructChild::Element(c) = child {
                parent[c.0 as usize] = Some(index);
            }
        }
    }

    // The links in `order`, which the validator holds to 0, 1, 2 …, with the page each sits on.
    let mut links: Vec<(&LinkItem, usize)> = list.pages.iter().enumerate().flat_map(|(p, page)| page.items.iter().filter_map(move |item| if let Item::Link(link) = item { Some((link, p)) } else { None })).collect();
    links.sort_by(|a, b| a.0.order.0.total_cmp(&b.0.order.0));

    let namespace = next.bump();
    pdf.indirect(namespace).dict().pair(Name(b"Type"), Name(b"Namespace")).pair(Name(b"NS"), TextStr(PDF_2_NAMESPACE));
    let annot_refs: Vec<Ref> = links.iter().map(|_| next.bump()).collect();
    let refs: Vec<Ref> = elements.iter().map(|_| next.bump()).collect();
    let root = next.bump();
    let mut annots = vec![Vec::new(); page_count];
    for ((link, page), &id) in links.iter().zip(&annot_refs) {
        // `/QuadPoints`, eight numbers per line rectangle, upper-left first, and `/Rect`, their union: `linkGeometry`.
        let quads: Vec<f64> = link.rects.iter().flat_map(|r| [r[0].0, r[1].0, r[2].0, r[1].0, r[0].0, r[3].0, r[2].0, r[3].0]).collect();
        let min_max = |odd: usize| quads.iter().skip(odd).step_by(2).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        let ((x0, x1), (y0, y1)) = (min_max(0), min_max(1));
        let mut annot = pdf.indirect(id).dict();
        annot.pair(Name(b"Type"), Name(b"Annot")).pair(Name(b"Subtype"), Name(b"Link"));
        annot.insert(Name(b"Rect")).array().items([x0, y0, x1, y1].map(|v| v as f32));
        annot.insert(Name(b"QuadPoints")).array().items(quads.iter().map(|&v| v as f32));
        if link.target.dest.is_none() {
            let uri = link.target.uri.as_deref().unwrap_or_default();
            annot.insert(Name(b"A")).dict().pair(Name(b"S"), Name(b"URI")).pair(Name(b"URI"), Str(uri.as_bytes()));
        }
        text(annot.insert(Name(b"Contents")), &link.contents.0);
        annot.pair(Name(b"F"), 4);
        annot.insert(Name(b"Border")).array().items([0, 0, 0]);
        annot.pair(Name(b"StructParent"), (page_count + link.order.0 as usize) as i32);
        annot.pair(Name(b"P"), pages[*page]);
        if let Some(dest) = &link.target.dest {
            let mut array = annot.insert(Name(b"Dest")).array();
            array.item(refs[dest.element.0 as usize]).item(Name(b"XYZ")).item(dest.x.0 as f32).item(dest.y.0 as f32);
            array.push().primitive(pdf_writer::Null);
        }
        annots[*page].push(id);
    }

    let mut by_page: Vec<Vec<Option<Ref>>> = vec![Vec::new(); page_count];
    let mut by_annot: Vec<Option<Ref>> = Vec::new();
    let mut ids: Vec<(Vec<u16>, &str, Ref)> = Vec::new();
    for (index, element) in elements.iter().enumerate() {
        let id = refs[index];
        let contents: Vec<(usize, usize)> = element.children.iter().filter_map(|c| if let StructChild::Content(c) = c { Some((c.page.0 as usize, c.item.0 as usize)) } else { None }).collect();
        for &(page, item) in &contents {
            let mcid = mcid_of(page, item) as usize;
            let slots = &mut by_page[page];
            if slots.len() <= mcid {
                slots.resize(mcid + 1, None);
            }
            slots[mcid] = Some(id);
        }
        let one_page = !contents.is_empty() && contents.iter().all(|c| c.0 == contents[0].0);
        let mut dict = pdf.indirect(id).dict();
        dict.pair(Name(b"Type"), Name(b"StructElem"));
        dict.pair(Name(b"S"), Name(element.element_type.as_bytes()));
        dict.pair(Name(b"NS"), namespace);
        dict.pair(Name(b"P"), parent[index].map_or(root, |p| refs[p]));
        if one_page {
            dict.pair(Name(b"Pg"), pages[contents[0].0]);
        }
        if let Some(lang) = element.lang.as_deref().filter(|l| !l.is_empty()) {
            dict.pair(Name(b"Lang"), Str(lang.as_bytes()));
        }
        if let Some(alt) = element.alt.as_ref().filter(|a| !a.0.is_empty()) {
            text(dict.insert(Name(b"Alt")), &alt.0);
        }
        if let Some(expansion) = element.expansion.as_ref().filter(|e| !e.0.is_empty()) {
            text(dict.insert(Name(b"E")), &expansion.0);
        }
        if let Some(element_id) = element.id.as_deref().filter(|i| !i.is_empty()) {
            dict.pair(Name(b"ID"), Str(element_id.as_bytes()));
            ids.push((element_id.encode_utf16().collect(), element_id, id));
        }
        match element.attributes.as_deref().unwrap_or_default() {
            [] => {}
            [one] => attribute(dict.insert(Name(b"A")), one),
            many => {
                let mut array = dict.insert(Name(b"A")).array();
                for a in many {
                    attribute(array.push(), a);
                }
            }
        }
        let mut kids = dict.insert(Name(b"K")).array();
        for child in &element.children {
            match child {
                StructChild::Element(c) => {
                    kids.item(refs[c.0 as usize]);
                }
                StructChild::Link(l) => {
                    let order = l.link.0 as usize;
                    if by_annot.len() <= order {
                        by_annot.resize(order + 1, None);
                    }
                    by_annot[order] = Some(id);
                    kids.push().dict().pair(Name(b"Type"), Name(b"OBJR")).pair(Name(b"Pg"), pages[links[order].1]).pair(Name(b"Obj"), annot_refs[order]);
                }
                StructChild::Content(c) => {
                    let (page, mcid) = (c.page.0 as usize, mcid_of(c.page.0 as usize, c.item.0 as usize));
                    if one_page {
                        kids.item(mcid);
                    } else {
                        kids.push().dict().pair(Name(b"Type"), Name(b"MCR")).pair(Name(b"Pg"), pages[page]).pair(Name(b"MCID"), mcid);
                    }
                }
            }
        }
    }

    // One flat number tree: page keys first, then one key per annotation; a hole JavaScript's `forEach` skips is skipped.
    let parent_tree = next.bump();
    {
        let mut dict = pdf.indirect(parent_tree).dict();
        let mut nums = dict.insert(Name(b"Nums")).array();
        for (page, slots) in by_page.iter().enumerate() {
            nums.item(page as i32);
            let mut array = nums.push().array();
            for slot in slots {
                match slot {
                    Some(r) => array.item(*r),
                    None => array.item(pdf_writer::Null),
                };
            }
        }
        for (order, owner) in by_annot.iter().enumerate() {
            if let Some(owner) = owner {
                nums.item((page_count + order) as i32).item(*owner);
            }
        }
    }

    // A name tree's keys are sorted by byte value (ISO 32000-2 §7.9.6): the ids by code unit, as the oracle sorts them.
    ids.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(pair) = ids.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(Refusal::new(format!("caster: two elements carry the /ID {}", crate::list::json_string(&pair[1].0))));
    }
    let id_tree = (!ids.is_empty()).then(|| next.bump());
    if let Some(id_tree) = id_tree {
        let mut dict = pdf.indirect(id_tree).dict();
        let mut names = dict.insert(Name(b"Names")).array();
        for (_, element_id, id) in &ids {
            names.item(Str(element_id.as_bytes())).item(*id);
        }
    }

    {
        let mut dict = pdf.indirect(root).dict();
        dict.pair(Name(b"Type"), Name(b"StructTreeRoot"));
        dict.insert(Name(b"K")).array().item(refs[0]);
        dict.insert(Name(b"Namespaces")).array().item(namespace);
        dict.pair(Name(b"ParentTree"), parent_tree);
        dict.pair(Name(b"ParentTreeNextKey"), (page_count + by_annot.len()) as i32);
        if let Some(id_tree) = id_tree {
            dict.pair(Name(b"IDTree"), id_tree);
        }
    }

    let outlines = outline(pdf, next, list, &refs);
    let page_labels = page_labels(pdf, next, list);
    Ok(Tree { root, outlines, page_labels, annots })
}

/// A text string: pdf-writer's `TextStr` where the text is Unicode, and UTF-16BE with its byte-order mark where it holds a lone surrogate, which `TextStr` cannot carry.
pub fn text(obj: Obj, units: &[u16]) {
    match String::from_utf16(units) {
        Ok(s) => obj.primitive(TextStr(&s)),
        Err(_) => {
            let bytes: Vec<u8> = [0xFE, 0xFF].into_iter().chain(units.iter().flat_map(|u| u.to_be_bytes())).collect();
            obj.primitive(Str(&bytes));
        }
    }
}

/// A number as pdf-lib writes it in a dictionary, but as pdf-writer's `f32` where it is not integral.
fn number(obj: Obj, n: f64) {
    if n.fract() == 0.0 && n.abs() < 2.0e9 {
        obj.primitive(n as i32);
    } else {
        obj.primitive(n as f32);
    }
}

/// One attribute object: its keys in `ATTRIBUTE_KEYS` order, every string a name but a `Headers` entry, a byte string.
fn attribute(obj: Obj, attribute: &Attribute) {
    let mut dict = obj.dict();
    for key in ATTRIBUTE_KEYS {
        let Some(value) = attribute.get(key) else { continue };
        let entry = dict.insert(Name(key.as_bytes()));
        match value {
            Value::Str(s) => entry.primitive(Name(s.as_bytes())),
            Value::Num(n) => number(entry, *n),
            Value::Array(values) => {
                let mut array = entry.array();
                for v in values {
                    match v {
                        Value::Str(s) if key == "Headers" => array.item(Str(s.as_bytes())),
                        Value::Str(s) => array.item(Name(s.as_bytes())),
                        v => {
                            number(array.push(), if let Value::Num(n) = v { *n } else { 0.0 });
                            &mut array
                        }
                    };
                }
            }
            _ => entry.primitive(pdf_writer::Null),
        }
    }
}

/// One heading placed in the outline: its entry, and its kids.
struct Node {
    entry: usize,
    kids: Vec<Node>,
    id: Ref,
}

/// Every descendant of a node, which is what an open `/Count` counts.
fn descendants(node: &Node) -> i32 {
    node.kids.iter().map(|k| 1 + descendants(k)).sum()
}

/// The outline, nested by level as `treeOf` nests it, every item open, allocated depth first and then the `/Outlines` dictionary.
fn outline(pdf: &mut Pdf, next: &mut Ref, list: &DisplayList, refs: &[Ref]) -> Option<Ref> {
    if list.outline.is_empty() {
        return None;
    }
    // The headings as a tree by a stack of the open ancestors' paths: a heading pops every open one whose level is not below its own.
    let mut roots: Vec<Node> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for (entry, heading) in list.outline.iter().enumerate() {
        while open.last().is_some_and(|&o| heading.level.0 <= list.outline[o].level.0) {
            open.pop();
        }
        let node = Node { entry, kids: Vec::new(), id: Ref::new(1) };
        let mut siblings = &mut roots;
        for _ in 0..open.len() {
            siblings = &mut siblings.last_mut().expect("an open ancestor").kids;
        }
        siblings.push(node);
        open.push(entry);
    }
    fn allocate(node: &mut Node, next: &mut Ref) {
        node.id = next.bump();
        for kid in &mut node.kids {
            allocate(kid, next);
        }
    }
    for root in &mut roots {
        allocate(root, next);
    }
    let outlines = next.bump();
    fn fill(pdf: &mut Pdf, list: &DisplayList, refs: &[Ref], nodes: &[Node], parent: Ref) {
        for (at, node) in nodes.iter().enumerate() {
            let heading = &list.outline[node.entry];
            let count = descendants(node);
            let mut item = pdf.indirect(node.id).dict();
            text(item.insert(Name(b"Title")), &heading.title.0);
            item.pair(Name(b"Parent"), parent);
            if at > 0 {
                item.pair(Name(b"Prev"), nodes[at - 1].id);
            }
            if let Some(n) = nodes.get(at + 1) {
                item.pair(Name(b"Next"), n.id);
            }
            if let (Some(first), Some(last)) = (node.kids.first(), node.kids.last()) {
                item.pair(Name(b"First"), first.id).pair(Name(b"Last"), last.id);
            }
            if count > 0 {
                item.pair(Name(b"Count"), count);
            }
            let mut dest = item.insert(Name(b"Dest")).array();
            dest.item(refs[heading.element.0 as usize]).item(Name(b"XYZ"));
            dest.push().primitive(pdf_writer::Null);
            dest.item(heading.top_pt.0 as f32);
            dest.push().primitive(pdf_writer::Null);
            dest.finish();
            item.finish();
            fill(pdf, list, refs, &node.kids, node.id);
        }
    }
    fill(pdf, list, refs, &roots, outlines);
    let total: i32 = roots.iter().map(|r| 1 + descendants(r)).sum();
    pdf.indirect(outlines).dict().pair(Name(b"Type"), Name(b"Outlines")).pair(Name(b"First"), roots[0].id).pair(Name(b"Last"), roots[roots.len() - 1].id).pair(Name(b"Count"), total);
    Some(outlines)
}

/// `/PageLabels`: ISO 32000-2 §12.4.2's number tree, whose first key is 0, so an empty dictionary says what the pages before the first run are called.
fn page_labels(pdf: &mut Pdf, next: &mut Ref, list: &DisplayList) -> Option<Ref> {
    let first = list.page_labels.first()?;
    let id = next.bump();
    let mut dict = pdf.indirect(id).dict();
    let mut nums = dict.insert(Name(b"Nums")).array();
    if first.page.0 != 0.0 {
        nums.item(0);
        nums.push().dict();
    }
    for run in &list.page_labels {
        number(nums.push(), run.page.0);
        let mut label = nums.push().dict();
        if !run.label.style.is_empty() {
            label.pair(Name(b"S"), Name(run.label.style.as_bytes()));
        }
        if !run.label.prefix.is_empty() {
            let units: Vec<u16> = run.label.prefix.encode_utf16().collect();
            text(label.insert(Name(b"P")), &units);
        }
        if run.label.start.0 != 1.0 {
            number(label.insert(Name(b"St")), run.label.start.0);
        }
    }
    Some(id)
}
