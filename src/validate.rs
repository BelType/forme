//! The refusals of a list the writer could not write, each a sentence, before a byte is written: Caster's `validateList` (`src/displayList/validate.ts`), ported check for check, in its order, with its sentences.
//!
//! What it holds: the version (read first, by [`crate::list::parse`]); every blob key a resource names; each image's samples in a shape the schema names, as many bytes as it says (version 6; `DeviceCMYK` at four components and with neither palette nor profile, version 7); the font instances (unique keys, sorted widths, a width for every glyph a text item shows and every space glyph it names); under a PDF/UA-2 claim, no word gap of a face with no space glyph; under the archival claim a colour story, no facsimile, and no CMYK colour without a CMYK output profile; every image and raster key a page item names; the marked content of each page (no end without a start, none left open); the links (`order` running 0, 1, 2 … over the document, a destination or a URI on each); the structure tree (an element index anywhere in range, one parent each, no cycle, a content child that is a content start naming its element back, every content start some element's child, a link child owned by the link's element, the attribute keys the schema names, a namespace that is the PDF 1.7 one where one is named, every `refs` index in range and none empty). What it leaves to the writer is what only writing finds. A number in a sentence is written as JavaScript's `String` writes it, so a sentence is the oracle's to the character.

use std::collections::{BTreeMap, BTreeSet};

use crate::Refusal;
use crate::content::js_string;
use crate::list::{BlackTextPolicy, Blobs, Conformance, DisplayList, Fill, Item, LinkItem, Num, Paint, ShadingSpace, StructChild, StructElement, TagStart, VectorItem, json_string};

/// The keys an attribute object of `/A` may carry, which the writer writes in this order.
pub const ATTRIBUTE_KEYS: [&str; 9] = ["O", "ListNumbering", "BBox", "Scope", "Headers", "RowSpan", "ColSpan", "TextPosition", "NoteType"];

/// The PDF 1.7 standard structure namespace, the one other namespace an element may name.
pub const PDF_1_7_NAMESPACE: &str = "http://iso.org/pdf/ssn";

/// Refuse a list the writer could not write.
///
/// # Errors
/// The first thing in the list that is not so, in the sentence `validateList` writes; one that concerns an item carries it as `list_item`.
pub fn validate(list: &DisplayList, blobs: &Blobs) -> Result<(), Refusal> {
    if list.version.0 != crate::list::LIST_VERSION {
        return Err(Refusal::new(format!("caster: the writer reads a display list of version {}, and this one is version {}; a list is written again by print(), never upgraded", js_string(crate::list::LIST_VERSION), js_string(list.version.0))));
    }
    check_blobs(list, blobs)?;
    check_samples(list, blobs)?;
    check_fonts(list)?;
    check_gaps(list)?;
    check_archival(list, blobs)?;
    check_items(list)?;
    check_links(list)?;
    check_tree(list)
}

/// Each image's samples in a shape the schema names, holding as many bytes as the shape says (version 6; `DeviceCMYK` at four components and with neither palette nor profile, version 7).
fn check_samples(list: &DisplayList, blobs: &Blobs) -> Result<(), Refusal> {
    for (key, image) in &list.resources.images {
        let Some(s) = &image.samples else { continue };
        let space = s.color_space.as_str();
        let bits = s.bits_per_component.0;
        let indexed = space == "Indexed";
        let palette_bytes = s.palette.as_ref().map_or(0, |p| blobs[p].len());
        let named = matches!(space, "DeviceRGB" | "DeviceGray" | "DeviceCMYK" | "Indexed") && (bits == 8.0 || bits == 16.0);
        if !named || indexed != s.palette.is_some() || (indexed && (bits != 8.0 || s.icc.is_some() || palette_bytes % 3 != 0 || !(3..=768).contains(&palette_bytes)) || (space == "DeviceCMYK" && s.icc.is_some())) {
            let palette = if s.palette.is_some() { format!(" with a palette of {palette_bytes} bytes") } else { String::new() };
            let profile = if s.icc.is_some() { " with a profile" } else { "" };
            return Err(Refusal::new(format!("caster: image {key}'s samples are {space} at {} bits{palette}{profile}, a shape the schema does not name", js_string(bits))));
        }
        let components = match space {
            "DeviceRGB" => 3.0,
            "DeviceCMYK" => 4.0,
            _ => 1.0,
        };
        let expected = s.width.0 * s.height.0 * (components + if s.alpha { 1.0 } else { 0.0 }) * (bits / 8.0);
        let held = blobs[&s.blob].len();
        if held as f64 != expected {
            let alpha = if s.alpha { " with alpha" } else { "" };
            return Err(Refusal::new(format!("caster: image {key}'s samples hold {held} bytes, and {} by {} {space} at {} bits{alpha} is {}", js_string(s.width.0), js_string(s.height.0), js_string(bits), js_string(expected))));
        }
    }
    Ok(())
}

/// A number as a set key: `-0` and `0` are one, as `SameValueZero` has them.
fn key(n: Num) -> u64 {
    (n.0 + 0.0).to_bits()
}

/// Every blob key a resource names is a key of the blobs.
fn check_blobs(list: &DisplayList, blobs: &Blobs) -> Result<(), Refusal> {
    let named = |key: Option<&str>, what: &dyn Fn() -> String| match key {
        Some(key) if !blobs.contains_key(key) => Err(Refusal::new(format!("caster: {} names the blob {key}, and the display list holds no bytes under that key", what()))),
        _ => Ok(()),
    };
    let r = &list.resources;
    for instance in &r.fonts {
        named(Some(&instance.blob), &|| format!("the font instance {}", instance.key))?;
    }
    for (key, image) in &r.images {
        named(image.blob.as_deref(), &|| format!("image {key} ({})", image.dom_path))?;
        named(image.jpeg.as_deref(), &|| format!("image {key}'s photo JPEG"))?;
        named(image.samples.as_ref().map(|s| s.blob.as_str()), &|| format!("image {key}'s samples"))?;
        named(image.samples.as_ref().and_then(|s| s.palette.as_deref()), &|| format!("image {key}'s palette"))?;
        named(image.samples.as_ref().and_then(|s| s.icc.as_deref()), &|| format!("image {key}'s profile"))?;
    }
    for (key, raster) in &r.rasters {
        named(raster.blob.as_deref(), &|| format!("raster {key} ({})", raster.feature))?;
        named(raster.jpeg.as_deref(), &|| format!("raster {key}'s photo JPEG"))?;
    }
    for (key, cluster) in &r.emoji {
        named(cluster.blob.as_deref(), &|| format!("colour cluster {key}"))?;
    }
    if let Some(colour) = &list.writing.colour {
        named(Some(&colour.profiles.cmyk), &|| "the colour story's CMYK profile".into())?;
        named(Some(&colour.profiles.srgb), &|| "the colour story's sRGB profile".into())?;
    }
    Ok(())
}

/// One key per instance, its widths sorted, and a width for every glyph a text item shows and every space glyph it names.
fn check_fonts(list: &DisplayList) -> Result<(), Refusal> {
    let mut widths: BTreeMap<&str, BTreeSet<u64>> = BTreeMap::new();
    for instance in &list.resources.fonts {
        if widths.contains_key(instance.key.as_str()) {
            return Err(Refusal::new(format!("caster: the display list holds two font instances under the key {}", instance.key)));
        }
        let (gids, units) = (&instance.widths.gids, &instance.widths.units);
        if gids.len() != units.len() || gids.windows(2).any(|pair| pair[1] <= pair[0]) {
            return Err(Refusal::new(format!("caster: the widths of {} are not one unit per glyph id in ascending order ({} ids, {} units)", instance.key, gids.len(), units.len())));
        }
        widths.insert(&instance.key, gids.iter().map(|&gid| key(gid)).collect());
    }
    for (p, page) in list.pages.iter().enumerate() {
        for (at, item) in page.items.iter().enumerate() {
            let Item::Text(text) = item else { continue };
            let Some(have) = widths.get(text.font.as_str()) else {
                return Err(Refusal::at(format!("caster: a text item names the font {}, and the display list holds no instance under that key", text.font), p, at));
            };
            for &gid in text.glyphs.ids.iter().chain([&text.space]) {
                if gid.0 != 0.0 && !have.contains(&key(gid)) {
                    return Err(Refusal::at(format!("caster: the display list holds no width for glyph {} of {}; the builder writes one for every glyph a text item shows and every space glyph it names", js_string(gid.0), text.font), p, at));
                }
            }
        }
    }
    Ok(())
}

/// A list that claims PDF/A-4 is one the file can keep it for, or it is refused by name (*an archival claim is kept or refused*, Caster's DESIGN.md S109): it carries a colour story, so its device colour has an output intent; it places no page of another PDF, which the writer copies as it is and does not read; and no colour in it is CMYK unless the output profile is a CMYK one. CMYK is a declared ink (process or spot, whose alternate is CMYK) on a run or a rectangle, black text under `blackText: '100k'`, a vector paint or gradient in CMYK, print marks (the `/All` separation), and an image whose samples are `DeviceCMYK` or whose file is a four-component JPEG. The facsimiles are read by image key, then the pages' items in order, then the forms by name.
fn check_archival(list: &DisplayList, blobs: &Blobs) -> Result<(), Refusal> {
    if list.info.archival.is_none() {
        return Ok(());
    }
    let Some(story) = &list.writing.colour else {
        return Err(Refusal::new("caster: the display list claims PDF/A-4 and carries no colour story, so no output intent would say what its colours mean (ISO 19005-4, 6.2.4.3)".into()));
    };
    for image in list.resources.images.values() {
        if let Some(f) = &image.facsimile {
            return Err(Refusal::new(format!("caster: the facsimile at {} is page {} of {}, a page of another PDF, which Caster copies as it is and does not read, so it cannot stand behind it under PDF/A-4; print without `a-4` in `standard`, or place the page as an image", image.dom_path, js_string(f.page.0), f.url)));
        }
    }
    if blobs[&story.profiles.cmyk].get(16..20) == Some(b"CMYK") {
        return Ok(());
    }
    let refusal = |what: String| format!("caster: {what}, and no CMYK profile stands behind it: the output intent is an RGB profile, so the file could not say what the ink means, which PDF/A-4 asks of every colour (ISO 19005-4, 6.2.4.3); pass a CMYK profile as the `profile` option, or print without `a-4` in `standard`");
    let inked = |fill: &Fill| fill.ink.is_some();
    for (p, page) in list.pages.iter().enumerate() {
        for (at, item) in page.items.iter().enumerate() {
            let cmyk = match item {
                Item::Text(t) if story.black_text == BlackTextPolicy::HundredK && !inked(&t.fill) && t.fill.rgb.iter().all(|c| c.0 == 0.0) => {
                    return Err(Refusal::at(refusal(format!("text item {at} of page {} is painted in CMYK, by `blackText: '100k'`", p + 1)), p, at));
                }
                Item::Text(t) => inked(&t.fill),
                Item::Rect(r) => inked(&r.fill),
                Item::Marks(_) => true,
                Item::Image(i) => image_cmyk(list, blobs, i.source.0)?,
                Item::Group(g) => group_cmyk(g),
                Item::Path(x) => path_cmyk(x),
                Item::Form(f) => mask_cmyk(&f.soft_mask),
                Item::RasterFallback(r) => mask_cmyk(&r.soft_mask),
                _ => false,
            };
            if cmyk {
                return Err(Refusal::at(refusal(format!("{} item {at} of page {} is painted in CMYK", item.kind(), p + 1)), p, at));
            }
        }
    }
    for (name, form) in &list.resources.forms {
        if form.items.iter().any(vector_cmyk) {
            return Err(Refusal::new(refusal(format!("the form {name} is painted in CMYK"))));
        }
    }
    Ok(())
}

/// Whether an image's samples or file are CMYK: `DeviceCMYK` samples, or a JPEG of four components.
fn image_cmyk(list: &DisplayList, blobs: &Blobs, source: f64) -> Result<bool, Refusal> {
    let Some(image) = list.resources.images.get(&(source as u32)) else { return Ok(false) };
    if image.samples.as_ref().is_some_and(|s| s.color_space == "DeviceCMYK") {
        return Ok(true);
    }
    match (&image.blob, image.mime.as_deref()) {
        (Some(blob), Some("image/jpeg")) => Ok(crate::images::parse_jpeg(&blobs[blob])?.components == 4),
        _ => Ok(false),
    }
}

/// Whether a vector item paints in CMYK, its children and its mask among it; a form item is read where its definition is, by name.
fn vector_cmyk(item: &VectorItem) -> bool {
    match item {
        VectorItem::Path(x) => path_cmyk(x),
        VectorItem::Group(g) => group_cmyk(g),
        VectorItem::Form(f) => mask_cmyk(&f.soft_mask),
        VectorItem::RasterFallback(r) => mask_cmyk(&r.soft_mask),
    }
}

/// A path's fill or stroke in CMYK, a process colour or a CMYK gradient, or its mask's content.
fn path_cmyk(x: &crate::list::PathItem) -> bool {
    let paint = |p: &Paint| match p {
        Paint::Cmyk(_) => true,
        Paint::Gradient(g) => g.gradient.color_space == ShadingSpace::DeviceCmyk,
        Paint::Rgb(_) => false,
    };
    x.fill.as_ref().is_some_and(paint) || x.stroke.as_ref().is_some_and(|s| paint(&s.paint)) || mask_cmyk(&x.soft_mask)
}

/// A group's children or its mask's content in CMYK.
fn group_cmyk(g: &crate::list::GroupItem) -> bool {
    g.items.iter().any(vector_cmyk) || mask_cmyk(&g.soft_mask)
}

/// Whether a soft mask's content paints in CMYK.
fn mask_cmyk(mask: &Option<crate::list::SoftMask>) -> bool {
    mask.as_ref().is_some_and(|m| m.items.iter().any(vector_cmyk))
}

/// A list that claims PDF/UA-2 shows no word gap as glyph 0 (*PDF/UA-2 is a flag, not a given*, Caster's DESIGN.md S103): a text item's `space` is 0 when its face has no space glyph, and a gap it owes (`leadingSpace`, or `spaceBefore` past the first segment) would show `.notdef`, which PDF/UA-2 forbids (ISO 14289-2, 8.4.5.9). Under a list that claims nothing the writer writes such a gap as positioning with no glyph.
fn check_gaps(list: &DisplayList) -> Result<(), Refusal> {
    if list.info.conformance != Conformance::Pdfua2 {
        return Ok(());
    }
    for (p, page) in list.pages.iter().enumerate() {
        for (at, item) in page.items.iter().enumerate() {
            let Item::Text(text) = item else { continue };
            if text.space.0 != 0.0 || !(text.leading_space || text.segments.space_before.iter().skip(1).any(|&gap| gap)) {
                continue;
            }
            let family = list.resources.fonts.iter().find(|instance| instance.key == text.font).map(|instance| instance.family.as_str()).filter(|family| !family.is_empty()).unwrap_or(&text.font);
            return Err(Refusal::at(format!("caster: the face {family} ({}) has no space glyph, so the word gap in text item {at} of page {} would be glyph 0, .notdef, which PDF/UA-2 forbids a file to show (ISO 14289-2, 8.4.5.9); give the text a face with a space, or print with `standard: 'none'`, which claims no standard and writes the gap as positioning with no glyph", text.font, p + 1), p, at));
        }
    }
    Ok(())
}

/// Whether a table holds the key a JavaScript object lookup by this number would find.
fn holds<T>(table: &crate::list::Table<T>, source: Num) -> bool {
    source.index().and_then(|i| u32::try_from(i).ok()).is_some_and(|k| table.contains_key(&k))
}

/// Every resource key a page item names, and each page's marked content opened and closed in turn.
fn check_items(list: &DisplayList) -> Result<(), Refusal> {
    let r = &list.resources;
    for (p, page) in list.pages.iter().enumerate() {
        let mut open: Option<usize> = None;
        for (at, item) in page.items.iter().enumerate() {
            let place = format!("item {at} of page {}", p + 1);
            match item {
                Item::Image(image) if !holds(&r.images, image.source) => {
                    return Err(Refusal::at(format!("caster: the image at {place} names source {}, which resources.images does not hold", js_string(image.source.0)), p, at));
                }
                Item::Raster(raster) if !holds(&r.emoji, raster.source) => {
                    return Err(Refusal::at(format!("caster: the raster at {place} names emoji {}, which the resources do not hold", js_string(raster.source.0)), p, at));
                }
                Item::RasterFallback(raster) if !holds(&r.rasters, raster.source) => {
                    return Err(Refusal::at(format!("caster: the raster at {place} names rasters {}, which the resources do not hold", js_string(raster.source.0)), p, at));
                }
                Item::TagEnd(_) => {
                    if open.is_none() {
                        return Err(Refusal::at(format!("caster: the marked content end at {place} closes nothing"), p, at));
                    }
                    open = None;
                }
                Item::TagStart(_) => {
                    if let Some(opened) = open {
                        return Err(Refusal::at(format!("caster: the marked content at {place} opens inside the one opened at item {opened}, which is not yet closed"), p, at));
                    }
                    open = Some(at);
                }
                _ => {}
            }
        }
        if let Some(opened) = open {
            return Err(Refusal::at(format!("caster: the marked content opened at item {opened} of page {} is never closed", p + 1), p, opened));
        }
    }
    Ok(())
}

/// The list's link items in document order.
fn links_of(list: &DisplayList) -> impl Iterator<Item = &LinkItem> {
    list.pages.iter().flat_map(|page| page.items.iter()).filter_map(|item| match item {
        Item::Link(link) => Some(link),
        _ => None,
    })
}

/// The links' `order` runs 0, 1, 2 … over the document, and each link goes somewhere: a destination in the file or a URI.
fn check_links(list: &DisplayList) -> Result<(), Refusal> {
    let mut orders: Vec<f64> = links_of(list).map(|link| link.order.0).collect();
    orders.sort_by(|a, b| a.total_cmp(b));
    for (at, &order) in orders.iter().enumerate() {
        if order != at as f64 {
            return Err(Refusal::new(format!("caster: the link at place {at} among the document's links has order {}; the orders run 0, 1, 2 …, and the /StructParent numbering follows them", js_string(order))));
        }
    }
    for link in links_of(list) {
        if link.target.dest.is_none() && link.target.uri.is_none() {
            return Err(Refusal::new(format!("caster: the link of order {} has neither a destination in the file nor a URI; the builder resolves one for every link without a destination", js_string(link.order.0))));
        }
    }
    Ok(())
}

/// The element at `index`, refused with what names it.
fn element_at<'a>(elements: &'a [StructElement], index: Num, what: &str) -> Result<&'a StructElement, Refusal> {
    let element = index.index().and_then(|i| elements.get(i));
    element.ok_or_else(|| Refusal::new(format!("caster: {what} names structure element {}, and the list's tree has {}", js_string(index.0), elements.len())))
}

/// Hold the tree to the shape the writer writes from: rooted at element 0, every index anywhere in range, every child listed once, every content child a content start that names the element back, every content start claimed, every link child a link owned by it, no attribute key the schema does not name.
fn check_tree(list: &DisplayList) -> Result<(), Refusal> {
    let elements = &list.structure.elements;
    if elements.is_empty() {
        return Err(Refusal::new("caster: the display list's structure tree has no elements; element 0 is the root".into()));
    }
    let mut starts: Vec<(usize, usize)> = Vec::new();
    for (p, page) in list.pages.iter().enumerate() {
        for (at, item) in page.items.iter().enumerate() {
            if let Item::TagStart(tag) = item
                && let TagStart::Content(start) = &tag.start
            {
                element_at(elements, start.element, &format!("the marked content at item {at} of page {}", p + 1))?;
                starts.push((p, at));
            }
        }
    }
    let mut link_owners: BTreeMap<u64, f64> = BTreeMap::new();
    for link in links_of(list) {
        element_at(elements, link.element, &format!("the link of order {}", js_string(link.order.0)))?;
        if let Some(dest) = &link.target.dest {
            element_at(elements, dest.element, &format!("the destination at {}", dest.struct_path))?;
        }
        link_owners.insert(key(link.order), link.element.0);
    }
    for heading in &list.outline {
        element_at(elements, heading.element, &format!("the bookmark {}", json_string(&heading.title.0)))?;
    }
    // -2 is no parent yet, -1 the root's.
    let mut parent: Vec<i64> = vec![-2; elements.len()];
    parent[0] = -1;
    let mut claimed: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (index, element) in elements.iter().enumerate() {
        let place = format!("structure element {index} ({})", element.path);
        for attribute in element.attributes.iter().flatten() {
            if let Some((stray, _)) = attribute.0.iter().find(|(k, _)| !ATTRIBUTE_KEYS.contains(&k.as_str())) {
                return Err(Refusal::new(format!("caster: {place} carries an attribute object with the key {stray}, which the schema does not name")));
            }
        }
        if let Some(namespace) = element.namespace.as_deref().filter(|n| *n != PDF_1_7_NAMESPACE) {
            let units: Vec<u16> = namespace.encode_utf16().collect();
            return Err(Refusal::new(format!("caster: {place} names the namespace {}; an element is in the PDF 2.0 namespace or names {PDF_1_7_NAMESPACE}", json_string(&units))));
        }
        if let Some(refs) = &element.refs {
            if refs.is_empty() {
                return Err(Refusal::new(format!("caster: {place} carries an empty refs; an element that refers to none carries none")));
            }
            for &r in refs {
                element_at(elements, r, &format!("the Ref of {place}"))?;
            }
        }
        for child in &element.children {
            match child {
                StructChild::Element(n) => {
                    let Some(c) = n.index().filter(|&c| c > 0 && c < elements.len()) else {
                        return Err(Refusal::new(format!("caster: {place} lists child {}, which is not an element below the root", js_string(n.0))));
                    };
                    if parent[c] != -2 {
                        let which = if parent[c] == index as i64 { "it already lists".to_string() } else { format!("element {} lists too", parent[c]) };
                        return Err(Refusal::new(format!("caster: {place} lists element {c}, which {which}; an element has one parent")));
                    }
                    parent[c] = index as i64;
                }
                StructChild::Link(link) => {
                    let owner = link_owners.get(&key(link.link));
                    if owner != Some(&(index as f64)) {
                        let which = owner.map_or("no link item has".to_string(), |o| format!("element {} owns", js_string(*o)));
                        return Err(Refusal::new(format!("caster: {place} lists the link of order {}, which {which}", js_string(link.link.0))));
                    }
                }
                StructChild::Content(content) => {
                    let (page, item) = (content.page, content.item);
                    let at = page.index().and_then(|p| list.pages.get(p)).and_then(|pg| item.index().and_then(|i| pg.items.get(i)));
                    let start = match at {
                        Some(Item::TagStart(tag)) => match &tag.start {
                            TagStart::Content(start) => Some(start),
                            TagStart::Artifact(_) => None,
                        },
                        _ => None,
                    };
                    let list_item = Some([page.0, item.0]);
                    let Some(start) = start else {
                        return Err(Refusal { message: format!("caster: {place} lists item {} of page {}, which is not a content start", js_string(item.0), js_string(page.0)), list_item });
                    };
                    if start.element.0 != index as f64 {
                        return Err(Refusal { message: format!("caster: {place} lists item {} of page {}, which opens content for element {}", js_string(item.0), js_string(page.0 + 1.0), js_string(start.element.0)), list_item });
                    }
                    // Both are indices here: the item was found by them.
                    if !claimed.insert((page.0 as usize, item.0 as usize)) {
                        return Err(Refusal { message: format!("caster: {place} lists item {} of page {} twice", js_string(item.0), js_string(page.0 + 1.0)), list_item });
                    }
                }
            }
        }
    }
    if let Some(orphan) = parent.iter().position(|&p| p == -2) {
        return Err(Refusal::new(format!("caster: structure element {orphan} ({}) is no element's child, so the tree does not reach it", elements[orphan].path)));
    }
    // One parent each and the root nobody's child: an element the root does not reach sits on a cycle.
    for (index, element) in elements.iter().enumerate() {
        let mut seen = BTreeSet::new();
        let mut at = index as i64;
        while at != -1 {
            if !seen.insert(at) {
                return Err(Refusal::new(format!("caster: structure element {index} ({}) is its own ancestor; the tree has a cycle", element.path)));
            }
            at = parent[at as usize];
        }
    }
    for &(page, item) in &starts {
        if !claimed.contains(&(page, item)) {
            return Err(Refusal::at(format!("caster: the content start at item {item} of page {} is in no element's children", page + 1), page, item));
        }
    }
    Ok(())
}
