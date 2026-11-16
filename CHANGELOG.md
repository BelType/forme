# Changelog

## Unreleased

- **The module is built for speed** (2026-11-16, Egildo's ruling of that day). The `wasm` profile's `opt-level` is `3`, no longer `"z"`, and Caster's `scripts/bundle-forme.js` runs `wasm-opt -O3` in place of `-Oz`: the module is loaded once and kept warm by a server, so the write is paid on every print and the size once. Measured 2026-11-16 in node, the vendored glue on the calling thread, forty writes after five, load average about 4 (indicative, a busy machine): the paper specimen's write fell from 28.3 to 22.0 ms (22 %) and a bill of 61 rows from 20.7 to 15.5 ms (25 %); the survey of that day measured 19 % and 25 %. Every file is byte for byte the size build's: the nineteen lists of Caster's `fixtures/lists/` through `npm run identity -- replay --writer forme`, and the paper's and the bill's dumped lists. The module grew from 940 796 to 1 519 698 bytes, and the budget, glue included, from 1 061 704 to 1 600 000.
