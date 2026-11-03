//! The tab's face: [`Writer`], the one class Caster's `forme` capability reaches through wasm-bindgen's glue (`vendor/forme.js`, `--target web`). `new Writer()`, then `writer.blob(key, bytes)` once per blob, then `writer.write(listJson)`, which returns `{ bytes: Uint8Array, records: string }` or throws an `Error` whose message is the [`Refusal`]'s sentence, with `listItem: [page, item]` where the refusal names an item.
//!
//! The list crosses as the JSON string `JSON.stringify` makes of it with `resources.blobs` removed, the fixture format's list file, and is read by [`crate::list::parse`]; the blobs cross as bytes, never inside JSON. The records are [`crate::json::records`]'s string, which Caster parses as its `WriterRecords`. Nothing here reads a clock or a `HashMap` order, so the module writes the bytes the native binary writes from the same list (*the same list prints the same bytes*, Caster's DESIGN.md S102).

use js_sys::{Array, Error, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

use crate::Refusal;
use crate::list::Blobs;

/// A write in the making: the blobs handed so far, then one list.
#[wasm_bindgen]
#[derive(Default)]
pub struct Writer {
    blobs: Blobs,
}

#[wasm_bindgen]
impl Writer {
    /// A writer holding no blob.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Writer {
        Writer::default()
    }

    /// Hand over one blob, under the SHA-256 the list names it by; the writer refuses one whose bytes are not its name.
    pub fn blob(&mut self, key: String, bytes: Vec<u8>) {
        self.blobs.insert(key, bytes);
    }

    /// Write the list, given as JSON without `resources.blobs`, with the blobs handed so far.
    ///
    /// # Errors
    /// The [`Refusal`] of [`crate::list::parse`] or of [`crate::write`], as an `Error`.
    pub fn write(&self, list_json: &str) -> Result<Object, JsValue> {
        let written = crate::list::parse(list_json).and_then(|list| crate::write(&list, &self.blobs)).map_err(refused)?;
        let out = Object::new();
        Reflect::set(&out, &"bytes".into(), &Uint8Array::from(written.bytes.as_slice()))?;
        Reflect::set(&out, &"records".into(), &crate::json::records(&written.records).into())?;
        Ok(out)
    }
}

/// A refusal as the `Error` Caster reads: its sentence, and `listItem` where it names an item.
fn refused(refusal: Refusal) -> JsValue {
    let error = Error::new(&refusal.message);
    if let Some([page, item]) = refusal.list_item {
        let _ = Reflect::set(&error, &"listItem".into(), &Array::of2(&page.into(), &item.into()));
    }
    error.into()
}
