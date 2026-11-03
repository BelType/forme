//! The refusals of a list the writer could not write, each a sentence, before a byte is written: Caster's `validateList` (`src/displayList/validate.ts`), ported check for check, in its order, with its sentences.
//!
//! What it holds: the version (read first, by [`crate::list::parse`]); every blob key a resource names; each image's samples in a shape the schema names, as many bytes as it says (version 6); the font instances (unique keys, sorted widths, a width for every glyph a text item shows and every space glyph it names); every image and raster key a page item names; the marked content of each page (no end without a start, none left open); the links (`order` running 0, 1, 2 … over the document, a destination or a URI on each); the structure tree (an element index anywhere in range, one parent each, no cycle, a content child that is a content start naming its element back, every content start some element's child, a link child owned by the link's element, the attribute keys the schema names). What it leaves to the writer is what only writing finds. A number in a sentence is written as JavaScript's `String` writes it, so a sentence is the oracle's to the character.

use std::collections::{BTreeMap, BTreeSet};

use crate::Refusal;
use crate::content::js_string;
use crate::list::{Blobs, DisplayList, Item, LinkItem, Num, StructChild, StructElement, TagStart, json_string};

/// The keys an attribute object of `/A` may carry, which the writer writes in this order.
pub const ATTRIBUTE_KEYS: [&str; 8] = ["O", "ListNumbering", "BBox", "Scope", "Headers", "RowSpan", "ColSpan", "TextPosition"];

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
    check_items(list)?;
    check_links(list)?;
    check_tree(list)
}

/// Each image's samples in a shape the schema names, holding as many bytes as the shape says (version 6).
fn check_samples(list: &DisplayList, blobs: &Blobs) -> Result<(), Refusal> {
    for (key, image) in &list.resources.images {
        let Some(s) = &image.samples else { continue };
        let space = s.color_space.as_str();
        let bits = s.bits_per_component.0;
        let indexed = space == "Indexed";
        let palette_bytes = s.palette.as_ref().map_or(0, |p| blobs[p].len());
        let named = matches!(space, "DeviceRGB" | "DeviceGray" | "Indexed") && (bits == 8.0 || bits == 16.0);
        if !named || indexed != s.palette.is_some() || (indexed && (bits != 8.0 || s.icc.is_some() || palette_bytes % 3 != 0 || !(3..=768).contains(&palette_bytes))) {
            let palette = if s.palette.is_some() { format!(" with a palette of {palette_bytes} bytes") } else { String::new() };
            let profile = if s.icc.is_some() { " with a profile" } else { "" };
            return Err(Refusal::new(format!("caster: image {key}'s samples are {space} at {} bits{palette}{profile}, a shape the schema does not name", js_string(bits))));
        }
        let components = if space == "DeviceRGB" { 3.0 } else { 1.0 };
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
