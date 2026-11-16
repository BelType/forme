//! The tab's face: [`Writer`], the one class Caster's `forme` capability reaches through wasm-bindgen's glue (`vendor/forme.js`, `--target web`). `new Writer()`, then `writer.blob(key, bytes)` once per blob, then `writer.write(listJson)`, which returns `{ bytes: Uint8Array, records: string }` or throws an `Error` whose message is the [`Refusal`]'s sentence, with `listItem: [page, item]` where the refusal names an item.
//!
//! A writer can also be kept between writes, as Caster's shared worker keeps one when its host asks it to keep blobs: `writer.lacks(keys)` names the keys it holds no bytes for, `writer.writeKept(listJson, keys)` writes with the blobs of those keys alone, and `writer.forget(key)` drops one. A blob is hashed on the first write that uses it after it arrived, where [`crate::write`] hashes it, and refused there as it would be; once a write has succeeded with it, it is not hashed again, and a blob handed again under the same key is hashed again. A blob refused for its bytes is not kept.
//!
//! The list crosses as the JSON string `JSON.stringify` makes of it with `resources.blobs` removed, the fixture format's list file, and is read by [`crate::list::parse`]; the blobs cross as bytes, never inside JSON. The records are [`crate::json::records`]'s string, which Caster parses as its `WriterRecords`. Nothing here reads a clock or a `HashMap` order, so the module writes the bytes the native binary writes from the same list (*the same list prints the same bytes*, Caster's DESIGN.md S102).

use js_sys::{Array, Error, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

use std::collections::BTreeSet;

use crate::Refusal;
use crate::list::Blobs;

/// A write in the making: the blobs handed so far, then one list; or, kept, the blobs of many writes, with the keys whose bytes a write has already found to be their name.
#[wasm_bindgen]
#[derive(Default)]
pub struct Writer {
    blobs: Blobs,
    verified: BTreeSet<String>,
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
        self.verified.remove(&key);
        self.blobs.insert(key, bytes);
    }

    /// The keys of `keys` this writer holds no bytes for.
    pub fn lacks(&self, keys: Vec<String>) -> Vec<String> {
        keys.into_iter().filter(|key| !self.blobs.contains_key(key)).collect()
    }

    /// Drop one blob.
    pub fn forget(&mut self, key: &str) {
        self.verified.remove(key);
        self.blobs.remove(key);
    }

    /// Write the list with the blobs of `keys` alone, the keys the list names, hashing only those no earlier write has found to be their name; the writer keeps every blob this write did not refuse.
    ///
    /// # Errors
    /// The [`Refusal`] of [`crate::list::parse`] or of [`crate::write_trusting`], as an `Error`.
    #[wasm_bindgen(js_name = writeKept)]
    pub fn write_kept(&mut self, list_json: &str, keys: Vec<String>) -> Result<Object, JsValue> {
        let named: Blobs = keys.iter().filter_map(|key| self.blobs.remove_entry(key)).collect();
        let verified = &self.verified;
        let written = crate::list::parse(list_json).and_then(|list| crate::write_trusting(&list, &named, &|key| verified.contains(key)));
        // Kept: every blob, when the write succeeded, now hashed; when it was refused, only those hashed before, since this write's hash may be the refusal.
        for (key, bytes) in named {
            if written.is_ok() {
                self.verified.insert(key.clone());
            }
            if self.verified.contains(&key) {
                self.blobs.insert(key, bytes);
            }
        }
        answer(written.map_err(refused)?)
    }

    /// Write the list, given as JSON without `resources.blobs`, with the blobs handed so far.
    ///
    /// # Errors
    /// The [`Refusal`] of [`crate::list::parse`] or of [`crate::write`], as an `Error`.
    pub fn write(&self, list_json: &str) -> Result<Object, JsValue> {
        answer(crate::list::parse(list_json).and_then(|list| crate::write(&list, &self.blobs)).map_err(refused)?)
    }
}

/// The file and the records as Caster reads them: `{ bytes, records }`.
fn answer(written: crate::Written) -> Result<Object, JsValue> {
    let out = Object::new();
    Reflect::set(&out, &"bytes".into(), &Uint8Array::from(written.bytes.as_slice()))?;
    Reflect::set(&out, &"records".into(), &crate::json::records(&written.records).into())?;
    Ok(out)
}

/// A refusal as the `Error` Caster reads: its sentence, and `listItem` where it names an item.
fn refused(refusal: Refusal) -> JsValue {
    let error = Error::new(&refusal.message);
    if let Some([page, item]) = refusal.list_item {
        let _ = Reflect::set(&error, &"listItem".into(), &Array::of2(&page.into(), &item.into()));
    }
    error.into()
}
