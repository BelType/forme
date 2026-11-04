//! The committed corpus, read from a Caster checkout (`CASTER`, the sibling `../caster` by default): every list Caster's fixtures hold, its blobs beside it, held to the schema, the validator and the writer.

use std::path::PathBuf;

use forme::list::{self, Conformance, DisplayList, Item, StructChild, TagEndItem, TagStart};
use forme::{Refusal, read_blobs, validate::validate, write};
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
    assert_eq!(corpus.len(), 19, "the nineteen lists");
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

    refused("cast", &|l| l.version = list::Num(4.0), "caster: the writer reads a display list of version 7, and this one is version 4; a list is written again by print(), never upgraded", None);
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
        e.attributes.as_mut().unwrap()[0].0.push(("Placement".into(), forme::read::Value::Str("Block".into())));
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
        let once = write(&list, &blobs).unwrap_or_else(|r| panic!("{name}: {r}")).bytes;
        assert_eq!(write(&list, &blobs).unwrap().bytes, once, "{name}: twice");
        let value: Value = serde_json::from_str(&text_of(&name)).unwrap();
        for descending in [false, true] {
            let other = list::parse(&reordered(&value, descending)).unwrap();
            assert_eq!(write(&other, &blobs).unwrap().bytes, once, "{name}: keys {}", if descending { "descending" } else { "ascending" });
        }
    }
}

#[test]
fn the_writer_refuses_a_blob_that_is_not_its_name_and_a_date_that_is_not_an_instant() {
    let (mut list, mut blobs) = list_of("cast");
    let font = list.resources.fonts[0].blob.clone();
    blobs.get_mut(&font).unwrap()[0] ^= 1;
    assert!(write(&list, &blobs).unwrap_err().message.starts_with(&format!("forme: the blob {font} holds bytes whose SHA-256 is ")));
    blobs.get_mut(&font).unwrap()[0] ^= 1;
    list.info.created = "2026-11-02T10:20:30.456Z".into();
    let text = String::from_utf8_lossy(&write(&list, &blobs).unwrap().bytes).into_owned();
    assert!(text.contains("/CreationDate (D:20261102102030Z)") && text.contains("/ModDate (D:20261102102030Z)"), "both dates, at whole seconds");
    list.info.created = "2026-11-02 10:20".into();
    assert_eq!(write(&list, &blobs).unwrap_err().message, "caster: the display list's info.created is \"2026-11-02 10:20\", which is not an ISO 8601 instant in UTC (YYYY-MM-DDTHH:mm:ssZ)");
}

#[test]
fn the_wtpdf_declarations_are_written_with_the_pdfua2_claim_and_withdrawn_with_it() {
    let (mut list, blobs) = list_of("cast");
    let declared = |list: &DisplayList| {
        let text = String::from_utf8_lossy(&write(list, &blobs).unwrap().bytes).into_owned();
        ["<pdfuaid:part>2</pdfuaid:part>", "<pdfd:conformsTo>http://pdfa.org/declarations/wtpdf#accessibility1.0</pdfd:conformsTo>", "<pdfd:conformsTo>http://pdfa.org/declarations/wtpdf#reuse1.0</pdfd:conformsTo>", "<pdfd:conformsTo>http://pdfa.org/declarations/wtpdf/#accessibility1.0</pdfd:conformsTo>", "<pdfd:conformsTo>http://pdfa.org/declarations/wtpdf/#reuse1.0</pdfd:conformsTo>", "xmlns:pdfd=\"http://pdfa.org/declarations/\""].map(|s| text.contains(s))
    };
    assert_eq!(list.info.conformance, Conformance::Pdfua2);
    assert_eq!(declared(&list), [true; 6], "claimed: the identification and both declarations, each in veraPDF's spelling and the text's");
    list.info.conformance = Conformance::Tagged;
    assert_eq!(declared(&list), [false; 6], "withdrawn: none of them");
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

/// The page loop's records against the list they come from: one pen per text item with one adjustment per segment, the first none; the MCIDs of each page its content starts in item order. The records against the oracle's are Caster's `npm run identity -- compare`.
#[test]
fn the_page_loop_records_every_run_and_content_start() {
    for (name, _) in corpus() {
        let (list, blobs) = list_of(&name);
        let records = write(&list, &blobs).unwrap_or_else(|r| panic!("{name}: {r}")).records;
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
    }
}

/// The tree's page entries on every list, read back by hayro-syntax: `/Tabs /S` and `/StructParents` on every page, and as many link annotations on each as the list has link items; and two elements with one `/ID` refused in the oracle's sentence. The tree element by element against the oracle's is Caster's `npm run identity -- compare`.
#[test]
fn every_page_points_into_the_tree_and_two_elements_with_one_id_are_refused() {
    use hayro_syntax::object::{Array, Name};
    for (name, _) in corpus() {
        let (list, blobs) = list_of(&name);
        let bytes = write(&list, &blobs).unwrap_or_else(|r| panic!("{name}: {r}")).bytes;
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
    assert_eq!(write(&list, &blobs).unwrap_err().message, format!("caster: two elements carry the /ID {}", serde_json::to_string(&id).unwrap()));
}

/// Every image the lists paint, read back by hayro-syntax from the page that names it: a file passed through is the file (a JPEG's bytes, a PNG's `IDAT`), the photo rule's JPEG is the list's, and samples inflate to the list's colour plane, with an `/SMask` that inflates to its alpha exactly where some pixel is not opaque. The XObjects against the oracle's are Caster's `npm run identity -- compare --only text,tags,images`.
#[test]
fn every_painted_image_is_the_lists_bytes_in_the_file() {
    use hayro_syntax::object::{Dict, Stream};
    let mut seen = 0;
    for (name, _) in corpus() {
        let (list, blobs) = list_of(&name);
        let written = write(&list, &blobs).unwrap_or_else(|r| panic!("{name}: {r}"));
        let pdf = hayro_syntax::Pdf::new(written.bytes).unwrap();
        let pages = pdf.pages();
        // A facsimile's Form XObject is the facsimile test's.
        for used in written.records.images.iter().filter(|u| u.info.path != "form") {
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
                    let stride = (match samples.color_space.as_str() { "DeviceRGB" => 3, "DeviceCMYK" => 4, _ => 1 } + usize::from(samples.alpha)) * bytes;
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
    assert_eq!(seen, 24, "the images the corpus paints, but the facsimiles");
}

/// The colour story on roads, read back by hayro-syntax: the output intent on the catalog with its CMYK profile, `/DefaultRGB` and the `/All` separation on every page, the black-text runs counted; and the oracle's refusals of a profile that is not CMYK, of print marks with no colour story, and of one colourant with two alternates. The record and the content against the oracle's are Caster's `npm run identity -- compare`.
#[test]
fn the_colour_story_is_written_and_its_refusals_are_the_oracles() {
    use hayro_syntax::object::{Array, Dict, Name, Stream};
    let (list, blobs) = list_of("roads");
    let written = write(&list, &blobs).unwrap();
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
    assert_eq!(write(&rgb, &blobs).unwrap_err().message, "colorResources: the output profile is \"RGB \", not CMYK");
    let mut bare = list.clone();
    bare.writing.colour = None;
    assert_eq!(write(&bare, &blobs).unwrap_err().message, "caster: print marks are stroked in the /All separation, which needs the colour options");
    let mut twice = list.clone();
    let spot = twice.pages.iter_mut().flat_map(|p| &mut p.items).find_map(|i| match i { Item::Text(t) => match &mut t.fill.ink { Some(list::Ink::Spot(s)) => Some(s), _ => None }, _ => None }).unwrap();
    spot.spot = "All".into();
    assert_eq!(write(&twice, &blobs).unwrap_err().message, "colorResources: the spot \"All\" is declared with two alternates, device-cmyk(0 1 0.65 0.15) and device-cmyk(1 1 1 1)");
}

#[test]
fn the_vectors_are_counted_as_the_oracle_counts_them_and_their_refusals_are_its_own() {
    let (list, blobs) = list_of("vectors");
    let records = write(&list, &blobs).unwrap().records;
    // The blend's stacking context is one more isolated group (Egildo's ruling 15a): the stage and the box blended over it.
    assert_eq!(serde_json::from_str::<Value>(&records.vector_resources).unwrap(), serde_json::json!({ "shadings": 1, "patterns": 0, "extGStates": 2, "forms": 4 }));
    let paint: Vec<Value> = serde_json::from_str(&records.paint).unwrap();
    assert_eq!(paint.len(), 21);
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

/// The blending space of a blend (Egildo's ruling 15a), read back by hayro-syntax: under the colour story, the page whose blend no group isolates has a `/Group` whose `/CS` is the sRGB stream `/DefaultRGB` names, and the other page none; with no colour story, no page has one.
#[test]
fn a_blend_mixes_in_the_documents_srgb() {
    use hayro_syntax::object::{Array, Dict, Name, Stream};
    let (list, blobs) = list_of("vector-roads");
    let pdf = hayro_syntax::Pdf::new(write(&list, &blobs).unwrap().bytes).unwrap();
    let groups: Vec<bool> = pdf.pages().iter().map(|page| {
        let Some(group) = page.raw().get::<Dict>(b"Group") else { return false };
        let space = group.get::<Array>(b"CS").unwrap();
        assert_eq!(space.iter::<Name>().next().as_deref(), Some(b"ICCBased".as_slice()));
        let default = page.raw().get::<Dict>(b"Resources").and_then(|r| r.get::<Dict>(b"ColorSpace")).and_then(|c| c.get::<Array>(b"DefaultRGB")).unwrap();
        assert_eq!({ let mut s = space.flex_iter(); s.next::<Name>(); s.next::<Stream>().unwrap().decoded().unwrap() }, { let mut d = default.flex_iter(); d.next::<Name>(); d.next::<Stream>().unwrap().decoded().unwrap() }, "the page's blending space is the sRGB of /DefaultRGB");
        true
    }).collect();
    assert_eq!(groups, [true, false], "blend-difference mixes with the page; box-blend with its stage, a group of its own");
    let (bare, blobs) = list_of("vectors");
    let pdf = hayro_syntax::Pdf::new(write(&bare, &blobs).unwrap().bytes).unwrap();
    assert!(pdf.pages().iter().all(|page| page.raw().get::<Dict>(b"Group").is_none()));
}

/// The facsimiles, read back by hayro-syntax: each painted facsimile is a Form XObject whose content is the source page's inside pdf-lib's two `q … Q` pairs, with `/FormType 1`, the source's box as `/BBox`, no `/Group`, and its font resolving to a program equal to the source's; two pages of one file share one copy of the font; and the refusals of an encrypted file, a file that is not a PDF and a page past the file's end, in the oracle's sentences. The forms against the oracle's are Caster's `npm run identity -- compare`.
#[test]
fn every_facsimile_is_its_source_page_as_a_form() {
    use hayro_syntax::object::{Array, Dict, Stream};
    for name in ["facsimile", "facsimile-pages"] {
        let (list, blobs) = list_of(name);
        let written = write(&list, &blobs).unwrap_or_else(|r| panic!("{name}: {r}"));
        let pdf = hayro_syntax::Pdf::new(written.bytes).unwrap();
        let mut fonts = Vec::new();
        for used in written.records.images.iter().filter(|u| u.info.path == "form") {
            let source_list = &list.resources.images[&used.source];
            let source = hayro_syntax::Pdf::new(blobs[source_list.blob.as_ref().unwrap()].clone()).unwrap();
            let page = &source.pages()[source_list.facsimile.as_ref().unwrap().page.0 as usize - 1];
            let form: Stream = pdf.pages()[used.page].raw().get::<Dict>(b"Resources").and_then(|r| r.get::<Dict>(b"XObject")).and_then(|x| x.get::<Stream>(used.name.as_bytes())).unwrap();
            let what = format!("{name}: {} on page {}", used.name, used.page);
            assert!(used.name.starts_with("Fc"), "{what}: named as the oracle names a facsimile");
            let mut content = b"q\n\nq\n\n".to_vec();
            content.extend_from_slice(&page.raw().get::<Stream>(b"Contents").or_else(|| page.raw().get::<Array>(b"Contents").and_then(|a| a.iter::<Stream>().next())).unwrap().decoded().unwrap());
            content.extend_from_slice(b"\nQ\n\nQ\n\n");
            assert_eq!(*form.decoded().unwrap(), content, "{what}: the content");
            let dict = form.dict();
            assert_eq!(dict.get::<i32>(b"FormType"), Some(1), "{what}: /FormType");
            assert!(dict.get::<Dict>(b"Group").is_none(), "{what}: no /Group");
            let bbox: Vec<f64> = dict.get::<Array>(b"BBox").unwrap().iter::<f64>().collect();
            assert_eq!((bbox[2] - bbox[0], bbox[3] - bbox[1]), (used.info.width, used.info.height), "{what}: the box");
            let font_dict = dict.get::<Dict>(b"Resources").and_then(|r| r.get::<Dict>(b"Font")).unwrap();
            let (key, font) = font_dict.entries().next().unwrap();
            let source_font = page.resources().fonts.get::<Dict>(key.as_ref()).unwrap();
            assert_eq!(program(&font_dict.get::<Dict>(key.as_ref()).unwrap()), program(&source_font), "{what}: the font program");
            fonts.push(format!("{font:?}"));
        }
        assert_eq!(fonts.len(), list.resources.images.len(), "{name}: every facsimile painted");
        fonts.dedup();
        assert_eq!(fonts.len(), 1, "{name}: one copy of the font the pages share");
    }
    let (mut list, mut blobs) = list_of("facsimile");
    let mut refused = |bytes: Vec<u8>, page: f64| {
        let key = forme_hex(&bytes);
        let source = list.resources.images.get_mut(&0).unwrap();
        source.blob = Some(key.clone());
        source.facsimile.as_mut().unwrap().page = list::Num(page);
        blobs.insert(key, bytes);
        write(&list, &blobs).unwrap_err().message
    };
    let fixtures = lists().join("../facsimile");
    let url = "https://print.invalid/facsimile/fixture.pdf";
    assert_eq!(refused(std::fs::read(fixtures.join("encrypted.pdf")).unwrap(), 1.0), format!("caster: the facsimile at object[0] names {url}, which is encrypted; an encrypted file cannot be carried into a PDF/UA-2 file, and the printer asks for no password. Ship the page from an unencrypted copy"));
    assert_eq!(refused(std::fs::read(fixtures.join("fixture.pdf")).unwrap(), 3.0), format!("caster: the facsimile at object[0] asks for page 3 of {url}, which has 2 pages"));
    assert!(refused(b"not a PDF".to_vec(), 1.0).starts_with(&format!("caster: the facsimile at object[0] names {url}, which cannot be read as a PDF: ")));
}

/// A font's program, decoded, through its descendant where it has one.
fn program(font: &hayro_syntax::object::Dict<'_>) -> Vec<u8> {
    use hayro_syntax::object::{Array, Dict, Stream};
    let font = font.get::<Array>(b"DescendantFonts").and_then(|a| a.iter::<Dict>().next()).unwrap_or_else(|| font.clone());
    font.get::<Dict>(b"FontDescriptor").and_then(|f| f.get::<Stream>(b"FontFile2")).unwrap().decoded().unwrap().to_vec()
}

fn forme_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A PNG passed through with an `iCCP` chunk, which no corpus file has: its profile, inflated by hayro-syntax's inflater, is the `/ICCBased` stream of the image, byte for byte. Made from cast's PNG with the sRGB profile of roads' colour story put in an `iCCP` after its `IHDR`.
#[test]
fn a_passed_through_pngs_iccp_profile_is_inflated_into_its_iccbased_stream() {
    use hayro_syntax::object::{Array, Dict, Stream};
    use std::io::Write;
    let (roads, roads_blobs) = list_of("roads");
    let profile = roads_blobs[&roads.writing.colour.as_ref().unwrap().profiles.srgb].clone();
    let (mut list, mut blobs) = list_of("cast");
    let png = blobs[list.resources.images[&0].blob.as_ref().unwrap()].clone();
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(9));
    z.write_all(&profile).unwrap();
    let mut payload = b"sRGB\0\0".to_vec();
    payload.extend(z.finish().unwrap());
    let mut chunk = (payload.len() as u32).to_be_bytes().to_vec();
    chunk.extend(b"iCCP");
    chunk.extend(&payload);
    chunk.extend([0; 4]);
    let ihdr_end = 8 + 12 + 13;
    let with = [&png[..ihdr_end], &chunk, &png[ihdr_end..]].concat();
    let key = forme_hex(&with);
    list.resources.images.get_mut(&0).unwrap().blob = Some(key.clone());
    blobs.insert(key, with);
    let written = write(&list, &blobs).unwrap();
    assert_eq!(written.records.images[0].info.icc, "embedded");
    let pdf = hayro_syntax::Pdf::new(written.bytes).unwrap();
    let used = &written.records.images[0];
    let image: Stream = pdf.pages()[used.page].raw().get::<Dict>(b"Resources").and_then(|r| r.get::<Dict>(b"XObject")).and_then(|x| x.get::<Stream>(used.name.as_bytes())).unwrap();
    let icc = image.dict().get::<Array>(b"ColorSpace").and_then(|a| {
        let mut items = a.flex_iter();
        items.next::<hayro_syntax::object::Name>()?;
        items.next::<Stream>()
    }).unwrap();
    assert_eq!(*icc.decoded().unwrap(), profile);
}

/// The hand reader against serde_json with `float_roundtrip` on every committed list: the same tree, every string equal and every number the same double, bit for bit.
#[test]
fn the_reader_reads_every_number_of_the_corpus_as_serde_json_with_float_roundtrip_does() {
    use forme::read::{self, Value as Ours};
    fn same(theirs: &Value, ours: &Ours, at: &str, numbers: &mut usize) {
        match (theirs, ours) {
            (Value::Null, Ours::Null) => {}
            (Value::Bool(a), Ours::Bool(b)) => assert_eq!(a, b, "{at}"),
            (Value::Number(a), Ours::Num(b)) => {
                assert_eq!(a.as_f64().unwrap().to_bits(), b.to_bits(), "{at}: {a} read as {b:?}");
                *numbers += 1;
            }
            (Value::String(a), Ours::Str(b)) => assert_eq!(a, b, "{at}"),
            (Value::Array(a), Ours::Array(b)) => {
                assert_eq!(a.len(), b.len(), "{at}");
                for (i, (x, y)) in a.iter().zip(b).enumerate() {
                    same(x, y, &format!("{at}/{i}"), numbers);
                }
            }
            (Value::Object(a), Ours::Object(b)) => {
                assert_eq!(a.len(), b.len(), "{at}");
                for (k, y) in b {
                    same(&a[k], y, &format!("{at}/{k}"), numbers);
                }
            }
            _ => panic!("{at}: {theirs} read as {ours:?}"),
        }
    }
    let mut numbers = 0;
    for (name, _) in corpus() {
        let text = text_of(&name);
        same(&serde_json::from_str(&text).unwrap(), &read::parse(&text).unwrap(), &name, &mut numbers);
    }
    assert!(numbers > 50_000, "every number of the corpus: {numbers}");
}

/// Every committed list cut short, and every struct key of it renamed in turn at its first place, is refused by `parse` with a sentence, and nothing panics.
#[test]
fn every_list_cut_short_or_with_a_field_renamed_is_refused() {
    /// The object keys that name a struct's field, at their first place, as the path of keys and indices that reaches each; the members of the list's maps (`coords`, `forms`, the integer-keyed tables, an attribute object) are data, not fields.
    fn fields(value: &Value, path: &mut Vec<String>, seen: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        let map = matches!(path.last().map(String::as_str), Some("coords" | "forms" | "images" | "rasters" | "emoji")) || (path.len() >= 2 && path[path.len() - 2] == "attributes");
        match value {
            Value::Object(members) => {
                for (k, v) in members {
                    if !map && !seen.contains(k) {
                        seen.push(k.clone());
                        out.push([path.as_slice(), std::slice::from_ref(k)].concat());
                    }
                    path.push(k.clone());
                    fields(v, path, seen, out);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    path.push(i.to_string());
                    fields(v, path, seen, out);
                    path.pop();
                }
            }
            _ => {}
        }
    }
    fn rename(value: &mut Value, path: &[String]) {
        match (value, path) {
            (Value::Object(members), [last]) => {
                let v = members.remove(last).unwrap();
                members.insert(format!("{last}X"), v);
            }
            (Value::Object(members), [first, rest @ ..]) => rename(members.get_mut(first).unwrap(), rest),
            (Value::Array(items), [first, rest @ ..]) => rename(&mut items[first.parse::<usize>().unwrap()], rest),
            _ => unreachable!(),
        }
    }
    let mut refused = 0;
    for (name, _) in corpus() {
        let text = text_of(&name);
        let text = text.trim_end();
        let cuts = (0..256).map(|k| k * text.len() / 256).chain(text.len().saturating_sub(64)..text.len());
        for cut in cuts.filter(|&c| text.is_char_boundary(c)) {
            assert!(list::parse(&text[..cut]).is_err(), "{name}: cut at byte {cut} of {}", text.len());
            refused += 1;
        }
        let value: Value = serde_json::from_str(text).unwrap();
        let mut paths = Vec::new();
        fields(&value, &mut Vec::new(), &mut Vec::new(), &mut paths);
        for path in paths {
            let mut renamed = value.clone();
            rename(&mut renamed, &path);
            let error = list::parse(&serde_json::to_string(&renamed).unwrap()).map(|_| ()).unwrap_err().message;
            assert!(error.starts_with("forme: the display list is not the schema of version 7: at ") || (path == ["version"] && error.contains("this one is version undefined")), "{name}: {} renamed: {error}", path.join("/"));
            refused += 1;
        }
    }
    assert!(refused > 5_000, "{refused} refused");
}

/// Caster's hand-made list of `test/textLine.test.ts`, made the same way: the photo list with an image and a rectangle between two runs of line 0, and a run after the page's last tag end. No committed list reaches either case.
fn interrupted() -> (DisplayList, list::Blobs) {
    let mut value: Value = serde_json::from_str(&text_of("photo")).unwrap();
    let items = value["pages"][0]["items"].as_array_mut().unwrap();
    let (run, mut image, mut last) = (items[1].clone(), items[6].clone(), items[10].clone());
    image["transform"] = serde_json::json!([20, 0, 0, 20, 300, 265]);
    let rect = serde_json::json!({ "kind": "shape", "owner": run["owner"], "rect": [34, 262, 120, 1.5], "fill": { "rgb": [0.8, 0, 0.2] } });
    items.splice(2..2, [image, rect, run]);
    last["lineIndex"] = serde_json::json!(last["lineIndex"].as_f64().unwrap() + 1.0);
    last["baseline"] = serde_json::json!(last["baseline"].as_f64().unwrap() - 16.5);
    items.push(last);
    for element in value["structure"]["elements"].as_array_mut().unwrap() {
        for child in element["children"].as_array_mut().unwrap() {
            if let Some(item) = child.get("item").and_then(Value::as_u64).filter(|&i| i >= 2) {
                child["item"] = serde_json::json!(item + 3);
            }
        }
    }
    let list = list::parse(&value.to_string()).unwrap();
    let blobs = read_blobs(&list, &lists().join("blobs")).unwrap();
    (list, blobs)
}

/// A line's text object holds text and nothing else (ISO 32000-2, 9.4 and figure 9): an image and a rectangle between two runs of one line close it, the next run reopens it, and a page end closes it. The file against the oracle's is Caster's comparator.
#[test]
fn a_non_text_item_and_a_page_end_close_the_open_line() {
    const IN_TEXT: &[&str] = &["w", "J", "j", "M", "d", "ri", "i", "gs", "CS", "cs", "SC", "SCN", "sc", "scn", "G", "g", "RG", "rg", "K", "k", "Tc", "Tw", "Tz", "TL", "Tf", "Tr", "Ts", "Td", "TD", "Tm", "T*", "Tj", "TJ", "'", "\"", "BDC", "BMC", "EMC", "MP", "DP"];
    let (list, blobs) = interrupted();
    let written = write(&list, &blobs).unwrap();
    let pdf = hayro_syntax::Pdf::new(written.bytes).unwrap();
    let mut ops: Vec<String> = Vec::new();
    let mut iter = pdf.pages()[0].operations();
    while let Some(i) = iter.next() {
        ops.push(String::from_utf8_lossy(i.operator).into_owned());
    }
    let mut in_text = false;
    for (at, op) in ops.iter().enumerate() {
        match op.as_str() {
            "BT" => { assert!(!in_text, "BT inside BT at {at}"); in_text = true; }
            "ET" => { assert!(in_text, "ET outside BT at {at}"); in_text = false; }
            other => assert!(!in_text || IN_TEXT.contains(&other), "{other} inside BT … ET at {at}"),
        }
    }
    assert!(!in_text, "the page ends inside BT … ET");
    assert_eq!(ops.iter().filter(|o| *o == "BT").count(), 7);
    let et = ops.iter().position(|o| o == "ET").unwrap();
    let bt = et + ops[et..].iter().position(|o| o == "BT").unwrap();
    assert_eq!(&ops[et..=bt], ["ET", "q", "cm", "Do", "Q", "q", "rg", "re", "f", "Q", "BT"]);
    assert_eq!(written.records.pens.len(), 7);
}

/// Caster's hand-made list of `test/standard.test.ts`, made the same way: the photo list with every text item's space glyph 0, as the builder writes it for a face with no space, and a width for glyph 0, which the builder asks for every space glyph it names.
fn spaceless(conformance: &str) -> (DisplayList, list::Blobs) {
    let mut value: Value = serde_json::from_str(&text_of("photo")).unwrap();
    value["info"]["conformance"] = serde_json::json!(conformance);
    for item in value["pages"].as_array_mut().unwrap().iter_mut().flat_map(|page| page["items"].as_array_mut().unwrap().iter_mut()) {
        if item["kind"] == "text" {
            item["space"] = serde_json::json!(0);
        }
    }
    for font in value["resources"]["fonts"].as_array_mut().unwrap() {
        font["widths"]["gids"].as_array_mut().unwrap().insert(0, serde_json::json!(0));
        font["widths"]["units"].as_array_mut().unwrap().insert(0, serde_json::json!(500));
    }
    let list = list::parse(&value.to_string()).unwrap();
    let blobs = read_blobs(&list, &lists().join("blobs")).unwrap();
    (list, blobs)
}

/// The glyphs every `TJ` of the first page shows.
fn shown(bytes: Vec<u8>) -> usize {
    let pdf = hayro_syntax::Pdf::new(bytes).unwrap();
    let content = String::from_utf8_lossy(pdf.pages()[0].page_stream().unwrap()).into_owned();
    content.split("TJ").filter_map(|part| part.rfind('[').map(|at| &part[at..])).flat_map(|array| array.split('<').skip(1).map(|hex| hex.split('>').next().unwrap().len() / 4)).sum()
}

/// *PDF/UA-2 is a flag, not a given* (Caster's DESIGN.md S103): a word gap in a face with no space glyph is refused by name under a list claiming PDF/UA-2, and written as positioning with no glyph, in an XMP claiming nothing, under a list claiming nothing.
#[test]
fn a_gap_with_no_space_glyph_is_refused_under_the_claim_and_positioning_without_it() {
    let (list, blobs) = spaceless("pdfua2");
    let refusal = write(&list, &blobs).map(|_| ()).unwrap_err();
    assert!(refusal.message.starts_with("caster: the face ") && refusal.message.contains(") has no space glyph, so the word gap in text item ") && refusal.message.ends_with("or print with `standard: 'none'`, which claims no standard and writes the gap as positioning with no glyph"), "{}", refusal.message);
    assert_eq!(refusal.list_item.map(|at| at[0]), Some(0.0));

    let (claimed, blobs) = list_of("photo");
    let texts: Vec<_> = claimed.pages[0].items.iter().filter_map(|item| if let Item::Text(text) = item { Some(text) } else { None }).collect();
    let glyphs: usize = texts.iter().map(|t| t.glyphs.ids.len()).sum();
    let gaps: usize = texts.iter().map(|t| usize::from(t.leading_space) + t.segments.space_before.iter().skip(1).filter(|&&g| g).count()).sum();
    assert!(gaps > 0);
    let bytes = write(&claimed, &blobs).unwrap().bytes;
    assert!(String::from_utf8_lossy(&bytes).contains("pdfd:declarations"), "the XMP claims PDF/UA-2 and declares Well-Tagged PDF");
    assert_eq!(shown(bytes), glyphs + gaps, "with a space glyph, every gap is one glyph");

    let (none, blobs) = spaceless("tagged");
    let bytes = write(&none, &blobs).unwrap().bytes;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    assert!(!text.contains("pdfuaid") && !text.contains("pdfd:declarations"), "the XMP claims nothing");
    assert_eq!(shown(bytes), glyphs, "with no space glyph, no gap shows a glyph");
}
