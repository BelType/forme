# forme

forme is a PDF 2.0 writer for Caster, BelType's printer, written in Rust. In letterpress the forme is the type locked into its frame, ready for the press, which is what a display list becomes when it is written. Caster measures the page in the browser and hands its writer one plain value, the display list; forme takes that list and writes the file. It is built to replace Caster's TypeScript writer, the oracle, which writes through pdf-lib and HarfBuzz's subsetter, and it runs in two places from one crate: in the tab as a WebAssembly module Caster vendors, and on the machine as the `forme` binary. It writes the document's scaffolding, every font instance the list names, subset and instanced by subsetter with the `hmtx` held to the list's widths, each page's marked content and text, its pen tracked as the oracle tracks it, the structure tree, and every image: a JPEG or a PNG passed through under the filter that reads it, the photo rule's JPEG, and samples deflated with an `/SMask` where a pixel is not opaque. It decodes no image, since the page decodes before the list what no filter reads. It refuses by name every item it does not write yet, the facsimile among them, and a list with a colour story.

## What it reads

forme reads the display list of version 6, as Caster's `src/displayList.ts` defines it, and nothing else: no page, no measurement, no clock. The list arrives as the JSON `JSON.stringify` makes of it without `resources.blobs`, and the blobs arrive beside it, each named by its SHA-256. On the machine that is Caster's fixture format, a `<name>.json` beside a `blobs/` folder. `src/list.rs` is the schema on the Rust side and names the sharp edges JSON hides: offsets in UTF-16 code units, numbers parsed exactly, JavaScript's rounding, the four arrays whose order is data. `src/validate.rs` refuses what Caster's `validateList` refuses, with the same sentences in the same order.

## The gate it is held to

forme is held to the readers, not to the oracle's bytes (*the same list prints the same bytes*, Caster's DESIGN.md S102). On every list committed in Caster's `fixtures/lists/`, the readers ask the same questions of the oracle's file and of forme's: whether every parser opens it, whether the validator agrees, whether the same list gives the same file, and later the records, the content streams, the tree, the words, the fonts, the images and the pixels. Each stage of the round adds the readers its writing needs, and the plan names them stage by stage (`.claude/plans/the-rust-writer/plan-rust-writer.md`, in the BelType monorepo). forme writes four things the oracle does not, each named: the trailer `/ID`, a CFF2 face, font programs without hinting tables, and later the Well-Tagged PDF declarations.

## How to build and test

Run `cargo build` for the library and the binary, then `cargo test` with a Caster checkout as the sibling `../caster`, or name it in `CASTER`; the tests read its committed lists. Run `forme validate <list.json>` to hold one list to the validator, and `forme write <list.json> --blobs <dir> -o <out.pdf> [--records <out.json>]` to write its file and, with `--records`, the writer's records as Caster's `WriterRecords` JSON, which Caster's `npm run identity -- compare <dir>` reads beside the file. For the stage gates only, `--partial` leaves out the items this stage does not write and lists them on stderr. Build the module with `cargo rustc --lib --profile wasm --target wasm32-unknown-unknown --crate-type cdylib`, then shrink it with `wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int`. Without the two flags `wasm-opt` refuses the module, because rustc 1.99 emits both features. The crate type is named because cargo performs no LTO on a library whose crate types include `rlib`, and this one has both, for the tests and the binary; `cargo build --lib` builds the module without LTO. The module's size is held to the bytes it lets Caster delete, 1 061 704. `NOTICE` carries the notices of the code subsetter adapts, which travel with every binary.

## The crates it links

| Crate | Pin | Why |
| :--- | :--- | :--- |
| pdf-writer | 0.15.0 | the PDF objects |
| serde, serde_json | 1.0.229, 1.0.154 | the list, read with `float_roundtrip` and `raw_value` |
| sha2 | 0.11.1 | the blobs' names |
| subsetter | 0.2.6 | the subset and the instancing |
| skrifa (read-fonts 0.39.2) | 0.42.1, forme's copy under `vendor/skrifa` | the font reads; the copy leaves skrifa's hinting code out, as `NOTICE` says |
| write-fonts | 0.48.1, forme's copy under `vendor/write-fonts` | subsetter's instanced `glyf`; the copy leaves the table repacker out, as `NOTICE` says |
| flate2 (miniz_oxide) | 1.1.10 (0.9.1) | every stream at level 6; flate2 1.1.10 requires miniz_oxide 0.9 |
