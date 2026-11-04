// Empty translation unit: it gives the forge_ffiFFI C target (the
// generated header and modulemap) something to compile. The actual
// symbols live in the Rust static library, which the app links; the
// SwiftPM tests never call them, so they stay unresolved.
