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

/// The nineteen broken lists of Caster's `test/displayList.test.ts`, each changed as that test changes it, and each refused with the sentence and the item `validateList` gives, as Caster's validator printed them on 2026-11-02.
#[test]
fn the_validator_refuses_the_nineteen_broken_lists_with_the_oracles_sentences() {
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

    refused("cast", &|l| l.version = list::Num(4.0), "caster: the writer reads a display list of version 5, and this one is version 4; a list is written again by print(), never upgraded", None);
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
    let (mut list, mut blobs) = list_of("cast");
    assert_eq!(write(&list, &blobs).unwrap_err().message, "forme: item 1 of page 2 is a image item, which this stage of forme does not write yet");
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
