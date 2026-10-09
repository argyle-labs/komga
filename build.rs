//! Generate the typed Komga client from the vendored OpenAPI spec.
//!
//! Komga publishes OpenAPI 3.1; the toolkit pipeline lowers it to 3.0 before
//! progenitor runs, so `specs/komga.openapi.json` stays the upstream document.
//! Only the paths in `src/operations.rs` are generated: the whole spec
//! overflows the stack in progenitor, and the plugin uses a small subset.
//! Refresh the spec with `scripts/refresh-spec.sh`.

#[allow(dead_code)]
#[path = "src/operations.rs"]
mod operations;

fn main() {
    println!("cargo:rerun-if-changed=src/operations.rs");
    println!("cargo:rerun-if-changed=specs/komga.openapi.json");
    let specs_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("specs");
    let paths = operations::paths();
    plugin_toolkit_build::openapi::generate_selected(&specs_dir, "komga", &[("komga", &paths)])
        .expect("komga openapi codegen");
}
