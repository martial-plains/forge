// SPDX-License-Identifier: MIT OR Apache-2.0
// The bindings are generated from the WIT by a macro that cargo doesn't
// know reads it; without this, an edited interface leaves a stale SDK
// ("cannot find function ... in module ...").
fn main() {
    println!("cargo:rerun-if-changed=../../wit");
    println!("cargo:rerun-if-changed=../../wit/forge-editor.wit");
}
