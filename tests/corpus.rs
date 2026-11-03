//! The committed corpus, read from a Caster checkout (`CASTER`, the sibling `../caster` by default): every list Caster's fixtures hold, its blobs beside it, held to the schema, the validator and the writer.

use std::path::PathBuf;

use forme::list::{self, DisplayList, Item, StructChild, TagEndItem, TagStart};
use forme::{Refusal, read_blobs, validate::validate, write, write_partial};
use serde_json::Value;

fn lists() -> PathBuf {
    let caster = std::env::var_os("CASTER").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../caster"));
    caster.join("fixtures/lists")
}

/// Every committed list by name, as `corpus.json` names them, with whether the oracle refused it.
fn corpus() -> Vec<(String, bool)> {
    let corpus: Value = serde_json::from_str(&std::fs::read_to_string(lists().join("corpus.json")).expect("corpus.json")).unwrap();
    corpus["documents"].as_array().unwrap().iter().map(|d| (d["name"].as_str().unwrap().to_string(), d.get("refused").is_some())).collect()
}

fn text_of(name: &str) -> String {
    std::fs::read_to_string(lists().join(format!("{name}.json"))).unwrap()
}

fn list_of(name: &str) -> (DisplayList, list::Blobs) {
    let list = list::parse(&text_of(name)).unwrap_or_else(|r| panic!("{name}: {r}"));
    let blobs = read_blobs(&list, &lists().join("blobs")).unwrap();
    (list, blobs)
}

/// A JSON value written again with every object's keys in ascending or descending order.
fn reordered(value: &Value, descending: bool) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            if descending {
                keys.reverse();
            }
            let entries: Vec<String> = keys.iter().map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), reordered(&map[*k], descending))).collect();
            format!("{{{}}}", entries.join(","))
        }
        Value::Array(items) => format!("[{}]", items.iter().map(|v| reordered(v, descending)).collect::<Vec<_>>().join(",")),
        other => serde_json::to_string(other).unwrap(),
    }
}

#[test]
fn every_committed_list_reads_and_writes_back_to_the_same_json_value() {
    let corpus = corpus();
    assert_eq!(corpus.len(), 14, "the fourteen lists");
    for (name, _) in &corpus {
        let text = text_of(name);
        let original: Value = serde_json::from_str(&text).unwrap();
        let back = serde_json::to_value(list::parse(&text).unwrap_or_else(|r| panic!("{name}: {r}"))).unwrap();
        assert!(back == original, "{name}: the list written back is another JSON value");
    }
}

#[test]
fn the_validator_passes_every_committed_list_the_cff2_list_among_them() {
    for (name, refused) in corpus() {
        let (list, blobs) = list_of(&name);
        assert_eq!(validate(&list, &blobs), Ok(()), "{name} (refused by the oracle's writer: {refused})");
    }
}

/// The twenty-one broken lists of Caster's `test/displayList.test.ts`, each changed as that test changes it, and each refused with the sentence and the item `validateList` gives, as Caster's validator printed them on 2026-11-02 (the last two on 2026-11-03, version 6).
#[test]
fn the_validator_refuses_the_twenty_one_broken_lists_with_the_oracles_sentences() {
    let refused = |name: &str, change: &dyn Fn(&mut DisplayList), message: &str, item: Option<[f64; 2]>| {
        let (mut list, blobs) = list_of(name);
        change(&mut list);
        assert_eq!(validate(&list, &blobs), Err(Refusal { message: message.to_string(), list_item: item }));
    };
    let first_text = |list: &mut DisplayList| -> usize { list.pages[0].items.iter().position(|i| matches!(i, Item::Text(_))).unwrap() };
    let tags = |list: &DisplayList| -> Vec<usize> { list.pages[0].items.iter().enumerate().filter(|(_, i)| i.kind() == "tag").map(|(at, _)| at).collect() };
    let ends = |list: &DisplayList| -> Vec<usize> { tags(list).into_iter().filter(|&at| matches!(list.pages[0].items[at], Item::TagEnd(_))).collect() };
    let links = |list: &mut DisplayList| -> Vec<(usize, usize)> {
        list.pages.iter().enumerate().flat_map(|(p, page)| page.items.iter().enumerate().filter(|(_, i)| matches!(i, Item::Link(_))).map(move |(at, _)| (p, at))).collect()
    };
    macro_rules! text_at {
        ($list:expr) => {{
            let at = first_text($list);
            match &mut $list.pages[0].items[at] {
                Item::Text(text) => text,
                _ => unreachable!(),
            }
        }};
    }
    macro_rules! link_at {
        ($list:expr, $place:expr) => {{
            let (p, at) = $place;
            match &mut $list.pages[p].items[at] {
                Item::Link(link) => link,
                _ => unreachable!(),
            }
        }};
    }

    refused("cast", &|l| l.version = list::Num(4.0), "caster: the writer reads a display list of version 6, and this one is version 4; a list is written again by print(), never upgraded", None);
    refused("cast", &|l| l.resources.fonts[0].blob = "0".repeat(64), "caster: the font instance Source Sans 3#1|wght=700 names the blob 0000000000000000000000000000000000000000000000000000000000000000, and the display list holds no bytes under that key", None);
    refused("cast", &|l| l.resources.images.values_mut().next().unwrap().jpeg = Some("f".repeat(64)), "caster: image 0's photo JPEG names the blob ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, and the display list holds no bytes under that key", None);
    refused("cast", &|l| { let copy = l.resources.fonts[0].clone(); l.resources.fonts.push(copy); }, "caster: the display list holds two font instances under the key Source Sans 3#1|wght=700", None);
    refused("cast", &|l| text_at!(l).font = "nowhere".into(), "caster: a text item names the font nowhere, and the display list holds no instance under that key", Some([0.0, 1.0]));
    refused("cast", &|l| text_at!(l).glyphs.ids[0] = list::Num(99999.0), "caster: the display list holds no width for glyph 99999 of Source Sans 3#1|wght=700; the builder writes one for every glyph a text item shows and every space glyph it names", Some([0.0, 1.0]));
    refused("cast", &|l| l.resources.fonts.iter_mut().find(|f| f.widths.gids.len() > 1).unwrap().widths.gids.reverse(), "caster: the widths of Source Sans 3#1|wght=700 are not one unit per glyph id in ascending order (14 ids, 14 units)", None);
    refused("cast", &|l| {
        let image = l.pages.iter_mut().flat_map(|p| p.items.iter_mut()).find_map(|i| match i { Item::Image(image) => Some(image), _ => None }).unwrap();
        image.source = list::Num(77.0);
    }, "caster: the image at item 1 of page 2 names source 77, which resources.images does not hold", Some([1.0, 1.0]));
    refused("cast", &|l| { let first = tags(l)[0]; l.pages[0].items[first] = Item::TagEnd(TagEndItem { kind: list::KindTag, end: true }); }, "caster: the marked content end at item 0 of page 1 closes nothing", Some([0.0, 0.0]));
    refused("cast", &|l| { let first = ends(l)[0]; l.pages[0].items.remove(first); }, "caster: the marked content at item 2 of page 1 opens inside the one opened at item 0, which is not yet closed", Some([0.0, 2.0]));
    refused("cast", &|l| { let last = *ends(l).last().unwrap(); l.pages[0].items.remove(last); }, "caster: the marked content opened at item 28 of page 1 is never closed", Some([0.0, 28.0]));
    refused("cast", &|l| {
        let at = tags(l).into_iter().find(|&at| matches!(&l.pages[0].items[at], Item::TagStart(t) if matches!(t.start, TagStart::Content(_)))).unwrap();
        if let Item::TagStart(tag) = &mut l.pages[0].items[at] && let TagStart::Content(start) = &mut tag.start { start.element = list::Num(999.0); }
    }, "caster: the marked content at item 0 of page 1 names structure element 999, and the list's tree has 32", None);
    refused("paper", &|l| l.outline[0].element = list::Num(-3.0), "caster: the bookmark \"Leaf litter decomposition along an altitudinal gradient\" names structure element -3, and the list's tree has 102", None);
    refused("cast", &|l| { let n = l.structure.elements.len(); l.structure.elements[0].children.push(StructChild::Element(list::Num(n as f64))); }, "caster: structure element 0 (Document) lists child 32, which is not an element below the root", None);
    refused("cast", &|l| {
        let item = first_text(l) as f64;
        let e = l.structure.elements.iter_mut().find(|x| x.children.iter().any(|c| !matches!(c, StructChild::Element(_)))).unwrap();
        let at = e.children.iter().position(|c| !matches!(c, StructChild::Element(_))).unwrap();
        e.children[at] = StructChild::Content(list::ContentChild { page: list::Num(0.0), item: list::Num(item) });
    }, "caster: structure element 1 (Document/H1[0]) lists item 1 of page 0, which is not a content start", Some([0.0, 1.0]));
    refused("cast", &|l| {
        let e = l.structure.elements.iter_mut().find(|x| x.children.iter().any(|c| matches!(c, StructChild::Content(_)))).unwrap();
        e.children.retain(|c| !matches!(c, StructChild::Content(_)));
    }, "caster: the content start at item 0 of page 1 is in no element's children", Some([0.0, 0.0]));
    refused("paper", &|l| {
        let e = l.structure.elements.iter_mut().find(|x| x.attributes.is_some()).unwrap();
        e.attributes.as_mut().unwrap()[0].0.push(("Placement".into(), Value::String("Block".into())));
    }, "caster: structure element 22 (Document/P[4]/Span[4]) carries an attribute object with the key Placement, which the schema does not name", None);
    refused("paper", &|l| { let last = *links(l).last().unwrap(); let link = link_at!(l, last); link.order = list::Num(link.order.0 + 1.0); }, "caster: the link at place 4 among the document's links has order 5; the orders run 0, 1, 2 …, and the /StructParent numbering follows them", None);
    refused("roads", &|l| {
        let place = links(l).into_iter().find(|&(p, at)| matches!(&l.pages[p].items[at], Item::Link(link) if link.target.dest.is_none())).unwrap();
        link_at!(l, place).target.uri = None;
    }, "caster: the link of order 0 has neither a destination in the file nor a URI; the builder resolves one for every link without a destination", None);
    // Version 6's two: a palette's samples at 16 bits, and samples holding twice the bytes their shape says.
    fn indexed(l: &mut DisplayList) -> &mut list::ImageSamples {
        l.resources.images.values_mut().find_map(|i| i.samples.as_mut().filter(|s| s.color_space == "Indexed")).unwrap()
    }
    refused("images", &|l| indexed(l).bits_per_component = list::Num(16.0), "caster: image 4's samples are Indexed at 16 bits with a palette of 6 bytes, a shape the schema does not name", None);
    refused("images", &|l| indexed(l).alpha = false, "caster: image 4's samples hold 3072 bytes, and 48 by 32 Indexed at 8 bits is 1536", None);
}

#[test]
fn the_same_list_writes_the_same_bytes_twice_and_whatever_its_keys_order() {
    for (name, _) in corpus() {
        let (list, blobs) = list_of(&name);
        let once = write_partial(&list, &blobs).unwrap_or_else(|r| panic!("{name}: {r}")).0.bytes;
        assert_eq!(write_partial(&list, &blobs).unwrap().0.bytes, once, "{name}: twice");
        let value: Value = serde_json::from_str(&text_of(&name)).unwrap();
        for descending in [false, true] {
            let other = list::parse(&reordered(&value, descending)).unwrap();
            assert_eq!(write_partial(&other, &blobs).unwrap().0.bytes, once, "{name}: keys {}", if descending { "descending" } else { "ascending" });
        }
    }
}

#[test]
fn the_writer_refuses_by_name_what_this_stage_does_not_write_a_blob_that_is_not_its_name_and_a_date_that_is_not_an_instant() {
    for (name, _) in corpus().into_iter().filter(|(n, _)| !n.starts_with("facsimile")) {
        let (list, blobs) = list_of(&name);
        assert_eq!(write(&list, &blobs).unwrap_or_else(|r| panic!("{name}: {r}")).bytes, write_partial(&list, &blobs).unwrap().0.bytes, "{name}: written whole");
    }
    let (mut list, mut blobs) = list_of("cast");
    let (facsimile, facsimile_blobs) = list_of("facsimile");
    let refused = write(&facsimile, &facsimile_blobs).unwrap_err().message;
    assert!(refused.starts_with("forme: image 0 (") && refused.ends_with(") is a facsimile, which this stage of forme does not write yet"), "{refused}");
    let font = list.resources.fonts[0].blob.clone();
    blobs.get_mut(&font).unwrap()[0] ^= 1;
    assert!(write_partial(&list, &blobs).unwrap_err().message.starts_with(&format!("forme: the blob {font} holds bytes whose SHA-256 is ")));
    blobs.get_mut(&font).unwrap()[0] ^= 1;
    list.info.created = "2026-11-02T10:20:30.456Z".into();
    let text = String::from_utf8_lossy(&write_partial(&list, &blobs).unwrap().0.bytes).into_owned();
    assert!(text.contains("/CreationDate (D:20261102102030Z)") && text.contains("/ModDate (D:20261102102030Z)"), "both dates, at whole seconds");
    list.info.created = "2026-11-02 10:20".into();
    assert_eq!(write_partial(&list, &blobs).unwrap_err().message, "caster: the display list's info.created is \"2026-11-02 10:20\", which is not an ISO 8601 instant in UTC (YYYY-MM-DDTHH:mm:ssZ)");
}

#[test]
fn a_list_with_a_lone_surrogate_in_a_run_reads_and_writes() {
    let text = text_of("cast").replacen("\"text\":\"A Note", "\"text\":\"\\ud800 Note", 1);
    assert_ne!(text, text_of("cast"), "the run was found");
    let list = list::parse(&text).unwrap();
    let units = list.pages.iter().flat_map(|p| p.items.iter()).find_map(|i| match i { Item::Text(t) => Some(t.text.0.clone()), _ => None }).unwrap();
    assert_eq!(units[0], 0xD800);
    assert!(serde_json::to_string(&list).unwrap().contains("\"text\":\"\\ud800 Note"), "written back as JSON.stringify writes it");
}

/// The four broken trees of Caster's `the writer refuses a tree that is not one` that the nineteen do not already hold: two parents, an orphan, a cycle, and content claimed by the wrong element.
#[test]
fn the_validator_refuses_the_four_other_broken_trees_with_the_oracles_sentences() {
    let refused = |change: &dyn Fn(&mut DisplayList), message: &str, item: Option<[f64; 2]>| {
        let (mut list, blobs) = list_of("cast");
        change(&mut list);
        assert_eq!(validate(&list, &blobs), Err(Refusal { message: message.to_string(), list_item: item }));
    };
    let one = StructChild::Element(list::Num(1.0));
    let unparent_one = |l: &mut DisplayList| {
        let p = l.structure.elements.iter().position(|e| e.children.contains(&one)).unwrap();
        l.structure.elements[p].children.retain(|c| *c != one);
    };
    refused(&|l| l.structure.elements[0].children.push(one.clone()), "caster: structure element 0 (Document) lists element 1, which it already lists; an element has one parent", None);
    refused(&|l| unparent_one(l), "caster: structure element 1 (Document/H1[0]) is no element's child, so the tree does not reach it", None);
    refused(&|l| {
        // Element 1 moves under its own last descendant: every element keeps one parent, and the tree has a cycle.
        unparent_one(l);
        let mut leaf = 1;
        while let Some(StructChild::Element(n)) = l.structure.elements[leaf].children.iter().rev().find(|c| matches!(c, StructChild::Element(_))) {
            leaf = n.index().unwrap();
        }
        l.structure.elements[leaf].children.push(one.clone());
    }, "caster: structure element 1 (Document/H1[0]) is its own ancestor; the tree has a cycle", None);
    refused(&|l| {
        let owners: Vec<usize> = l.structure.elements.iter().enumerate().filter(|(_, e)| e.children.iter().any(|c| matches!(c, StructChild::Content(_)))).map(|(i, _)| i).collect();
        let (a, b) = (owners[0], owners[1]);
        let at = l.structure.elements[b].children.iter().position(|c| !matches!(c, StructChild::Element(_))).unwrap();
        let moved = l.structure.elements[b].children.remove(at);
        l.structure.elements[a].children.push(moved);
    }, "caster: structure element 1 (Document/H1[0]) lists item 3 of page 1, which opens content for element 2", Some([0.0, 3.0]));
}

/// The page loop's records against the list they come from: one pen per text item with one adjustment per segment, the first none; the MCIDs of each page its content starts in item order; every list with nothing left out written by `write` as by `write_partial`. The records against the oracle's are Caster's `npm run identity -- compare`.
#[test]
fn the_page_loop_records_every_run_and_content_start() {
    for (name, _) in corpus() {
        let (list, blobs) = list_of(&name);
        let records = write_partial(&list, &blobs).unwrap_or_else(|r| panic!("{name}: {r}")).0.records;
        let texts: Vec<(usize, usize, usize)> = list.pages.iter().enumerate().flat_map(|(p, page)| page.items.iter().enumerate().filter_map(move |(at, i)| match i { Item::Text(t) => Some((p, at, t.segments.x.len())), _ => None })).collect();
        assert_eq!(records.pens.len(), texts.len(), "{name}: one pen per text item");
        for (pen, &(p, at, segments)) in records.pens.iter().zip(&texts) {
            assert_eq!((pen.page, pen.item, pen.adjustments.len()), (p, at, segments), "{name}: the pen of item {at} of page {p}");
            assert!(pen.adjustments[0].is_none() && pen.adjustments[1..].iter().all(Option::is_some), "{name}: none before the first segment only");
        }
        for (page, mcids) in list.pages.iter().zip(&records.mcids) {
            let starts: Vec<usize> = page.items.iter().enumerate().filter(|(_, i)| matches!(i, Item::TagStart(t) if matches!(t.start, TagStart::Content(_)))).map(|(at, _)| at).collect();
            assert_eq!(mcids, &starts, "{name}: the MCIDs are the content starts in item order");
        }
        let facsimile = |source: &list::Num| list.resources.images.get(&(source.0 as u32)).is_some_and(|s| s.kind == list::ImageKind::Facsimile);
        let whole = list.pages.iter().flat_map(|p| &p.items).all(|i| match i {
            Item::Image(image) => !facsimile(&image.source),
            i => matches!(i, Item::Text(_) | Item::TagStart(_) | Item::TagEnd(_) | Item::Link(_) | Item::Rect(_) | Item::Marks(_)),
        });
        if whole {
            assert_eq!(write(&list, &blobs).unwrap().bytes, write_partial(&list, &blobs).unwrap().0.bytes, "{name}: nothing left out");
        }
    }
}

/// The tree's page entries on every list, read back by hayro-syntax: `/Tabs /S` and `/StructParents` on every page, and as many link annotations on each as the list has link items; and two elements with one `/ID` refused in the oracle's sentence. The tree element by element against the oracle's is Caster's `npm run identity -- compare`.
#[test]
fn every_page_points_into_the_tree_and_two_elements_with_one_id_are_refused() {
    use hayro_syntax::object::{Array, Name};
    for (name, _) in corpus() {
        let (list, blobs) = list_of(&name);
        let bytes = write_partial(&list, &blobs).unwrap_or_else(|r| panic!("{name}: {r}")).0.bytes;
        let pdf = hayro_syntax::Pdf::new(bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        for (index, (page, listed)) in pdf.pages().iter().zip(&list.pages).enumerate() {
            let raw = page.raw();
            assert_eq!(raw.get::<Name>(b"Tabs").as_deref(), Some(b"S".as_slice()), "{name}: /Tabs /S on page {index}");
            assert_eq!(raw.get::<i64>(b"StructParents"), Some(index as i64), "{name}: /StructParents of page {index}");
            let links = listed.items.iter().filter(|i| matches!(i, Item::Link(_))).count();
            assert_eq!(raw.get::<Array>(b"Annots").map_or(0, |a| a.iter::<hayro_syntax::object::Dict>().count()), links, "{name}: the link annotations of page {index}");
        }
    }
    let (mut list, blobs) = list_of("paper");
    let ids: Vec<usize> = list.structure.elements.iter().enumerate().filter(|(_, e)| e.id.is_some()).map(|(i, _)| i).collect();
    list.structure.elements[ids[1]].id = list.structure.elements[ids[0]].id.clone();
    let id = list.structure.elements[ids[0]].id.clone().unwrap();
    assert_eq!(write_partial(&list, &blobs).unwrap_err().message, format!("caster: two elements carry the /ID {}", serde_json::to_string(&id).unwrap()));
}

/// Every image the lists paint, read back by hayro-syntax from the page that names it: a file passed through is the file (a JPEG's bytes, a PNG's `IDAT`), the photo rule's JPEG is the list's, and samples inflate to the list's colour plane, with an `/SMask` that inflates to its alpha exactly where some pixel is not opaque. The XObjects against the oracle's are Caster's `npm run identity -- compare --only text,tags,images`.
#[test]
fn every_painted_image_is_the_lists_bytes_in_the_file() {
    use hayro_syntax::object::{Dict, Stream};
    let mut seen = 0;
    for (name, _) in corpus() {
        let (list, blobs) = list_of(&name);
        let written = write_partial(&list, &blobs).unwrap_or_else(|r| panic!("{name}: {r}")).0;
        let pdf = hayro_syntax::Pdf::new(written.bytes).unwrap();
        let pages = pdf.pages();
        for used in &written.records.images {
            let xobject: Stream = pages[used.page].raw().get::<Dict>(b"Resources").and_then(|r| r.get::<Dict>(b"XObject")).and_then(|x| x.get::<Stream>(used.name.as_bytes())).unwrap_or_else(|| panic!("{name}: {} on page {}", used.name, used.page));
            let source = &list.resources.images[&used.source];
            let what = format!("{name}: image {} ({})", used.source, used.info.path);
            match (used.info.path, used.info.filter) {
                ("passthrough", "DCTDecode") => assert_eq!(&*xobject.raw_data(), blobs[source.blob.as_ref().unwrap()].as_slice(), "{what}"),
                ("passthrough", _) => assert_eq!(*xobject.raw_data(), forme::images::parse_png(&blobs[source.blob.as_ref().unwrap()]).unwrap().idat, "{what}"),
                ("decode-jpeg", _) => assert_eq!(&*xobject.raw_data(), blobs[source.jpeg.as_ref().unwrap()].as_slice(), "{what}"),
                _ => {
                    let samples = source.samples.as_ref().unwrap();
                    let bytes = (samples.bits_per_component.0 / 8.0) as usize;
                    let stride = (if samples.color_space == "DeviceRGB" { 3 } else { 1 } + usize::from(samples.alpha)) * bytes;
                    let colour_bytes = stride - if samples.alpha { bytes } else { 0 };
                    let data = &blobs[&samples.blob];
                    let colour: Vec<u8> = data.chunks_exact(stride).flat_map(|p| p[..colour_bytes].to_vec()).collect();
                    let alpha: Vec<u8> = if samples.alpha { data.chunks_exact(stride).flat_map(|p| p[colour_bytes..].to_vec()).collect() } else { Vec::new() };
                    assert_eq!(*xobject.decoded().unwrap(), colour, "{what}: the colour plane");
                    let mask = xobject.dict().get::<Stream>(b"SMask");
                    assert_eq!(mask.is_some(), alpha.iter().any(|&a| a != 0xff), "{what}: an /SMask exactly where a pixel is not opaque");
                    if let Some(mask) = mask {
                        assert_eq!(*mask.decoded().unwrap(), alpha, "{what}: the alpha plane");
                    }
                }
            }
            seen += 1;
        }
    }
    assert_eq!(seen, 12, "the images the corpus paints, but the facsimiles");
}

/// The colour story on roads, read back by hayro-syntax: the output intent on the catalog with its CMYK profile, `/DefaultRGB` and the `/All` separation on every page, the black-text runs counted; and the oracle's refusals of a profile that is not CMYK, of print marks with no colour story, and of one colourant with two alternates. The record and the content against the oracle's are Caster's `npm run identity -- compare`.
#[test]
fn the_colour_story_is_written_and_its_refusals_are_the_oracles() {
    use hayro_syntax::object::{Array, Dict, Name, Stream};
    let (list, blobs) = list_of("roads");
    let written = write_partial(&list, &blobs).unwrap().0;
    let record = written.records.colour.as_ref().expect("a colour record");
    assert!(record.black_text_runs > 0 && record.color_spaces.iter().any(|s| s.colorant.as_deref() == Some("All")));
    let pdf = hayro_syntax::Pdf::new(written.bytes).unwrap();
    let catalog = pdf.xref().get::<Dict>(pdf.xref().root_id()).unwrap();
    let intent: Dict = catalog.get::<Array>(b"OutputIntents").unwrap().iter::<Dict>().next().unwrap();
    assert_eq!(intent.get::<Name>(b"S").as_deref(), Some(b"GTS_PDFX".as_slice()));
    assert_eq!(&*intent.get::<Stream>(b"DestOutputProfile").unwrap().decoded().unwrap(), blobs[&list.writing.colour.as_ref().unwrap().profiles.cmyk].as_slice());
    for page in pdf.pages().iter() {
        let spaces = page.raw().get::<Dict>(b"Resources").and_then(|r| r.get::<Dict>(b"ColorSpace")).unwrap();
        assert!(spaces.get::<Array>(b"DefaultRGB").is_some() && record.color_spaces[1..].iter().all(|s| spaces.get::<Array>(s.name.as_bytes()).is_some()));
    }

    let mut rgb = list.clone();
    rgb.writing.colour.as_mut().unwrap().profiles.cmyk = rgb.writing.colour.as_ref().unwrap().profiles.srgb.clone();
    assert_eq!(write_partial(&rgb, &blobs).unwrap_err().message, "colorResources: the output profile is \"RGB \", not CMYK");
    let mut bare = list.clone();
    bare.writing.colour = None;
    assert_eq!(write_partial(&bare, &blobs).unwrap_err().message, "caster: print marks are stroked in the /All separation, which needs the colour options");
    let mut twice = list.clone();
    let spot = twice.pages.iter_mut().flat_map(|p| &mut p.items).find_map(|i| match i { Item::Text(t) => match &mut t.fill.ink { Some(list::Ink::Spot(s)) => Some(s), _ => None }, _ => None }).unwrap();
    spot.spot = "All".into();
    assert_eq!(write_partial(&twice, &blobs).unwrap_err().message, "colorResources: the spot \"All\" is declared with two alternates, device-cmyk(0 1 0.65 0.15) and device-cmyk(1 1 1 1)");
}

#[test]
fn the_vectors_are_counted_as_the_oracle_counts_them_and_their_refusals_are_its_own() {
    let (list, blobs) = list_of("vectors");
    let records = write(&list, &blobs).unwrap().records;
    assert_eq!(serde_json::from_str::<Value>(&records.vector_resources).unwrap(), serde_json::json!({ "shadings": 1, "patterns": 0, "extGStates": 2, "forms": 3 }));
    let paint: Vec<Value> = serde_json::from_str(&records.paint).unwrap();
    assert_eq!(paint.len(), 20);
    assert!(paint.iter().any(|r| r["kind"] == "form" && r["params"]["reused"] == true), "the marker's form is drawn more than once and written once");

    let (roads, blobs) = list_of("roads");
    let mut bare = roads.clone();
    let raster = bare.resources.rasters.values_mut().next().unwrap();
    raster.blob = None;
    let feature = raster.feature.clone();
    let refused = write(&bare, &blobs).unwrap_err().message;
    assert!(refused.starts_with("vectorEmit: the raster for ") && refused.ends_with(&format!(" ({feature}) was never embedded")), "{refused}");
    let mut one = roads.clone();
    let gradient = one.pages.iter_mut().flat_map(|p| &mut p.items).find_map(|i| match i { Item::Path(p) => match &mut p.fill { Some(list::Paint::Gradient(g)) => Some(&mut g.gradient), _ => None }, _ => None }).unwrap();
    gradient.stops.truncate(1);
    assert_eq!(write(&one, &blobs).unwrap_err().message, "shading: a gradient needs at least two stops");
}
