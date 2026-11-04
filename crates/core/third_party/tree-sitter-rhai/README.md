# tree-sitter-rhai

Vendored from <https://github.com/elkowar/tree-sitter-rhai> at commit
`4ac7384d487ffcb54e746ef1569585a749370c5b`.

Only the generated parser (`src/parser.c`), its header, and
`queries/highlights.scm` are included (the query drops the broad
`(ident) @variable` capture so it cannot shadow function names). The grammar is MIT licensed; see
[`LICENSE`](LICENSE). Upstream has no `Cargo.toml`, so `crates/core/build.rs`
compiles `src/parser.c` directly with `cc`.
