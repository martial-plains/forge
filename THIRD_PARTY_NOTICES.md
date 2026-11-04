# Third-party notices

Forge itself is licensed under either of MIT or Apache-2.0 at your option
(`LICENSE-MIT`, `LICENSE-APACHE`). It includes or links the following.

## Included in the source tree

| Material | Where | License |
| --- | --- | --- |
| Mozc OSS dictionary (readings and written forms, trimmed; includes entries from the NAIST/ICOT lexicons) | `crates/editor/data/ja-dict.txt.gz`, built by `scripts/build-kana-kanji-dict.py` | BSD-3-Clause (Google Inc.) plus the NAIST and ICOT notices. Full text: `third_party/mozc/LICENSE`. |
| tree-sitter-rhai grammar | `crates/core/third_party/tree-sitter-rhai` | MIT (ElKowar). Its `LICENSE` is alongside. |
| Lua 5.x interpreter | `sdks/lua/lua` | MIT (PUC-Rio). |
| WASI preview1 reactor adapter | `sdks/lua/wasi_snapshot_preview1.reactor.wasm` | Apache-2.0 (the Wasmtime project). |

Unicode data is used through Rust crates (`unicode-normalization`,
`unicode_names2`, `unicode-width`, `emojis`) and Python's `unicodedata`,
under their own permissive licenses and the Unicode License.

## Plugins

The plugins other than `plugins/hello` are separate repositories. The ones that port GPL-3.0-or-later packages keep that license; they talk to Forge only through the plugin interface (`wit/`), and Forge does not link them.

## Cargo dependencies

Rust crates used by Forge keep their own licenses (MIT, Apache-2.0, BSD,
Unicode and similar); `cargo license` or `cargo deny` lists them.
