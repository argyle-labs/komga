//! The curated Komga operations this plugin uses: the single list that selects
//! the generated client subset (`build.rs`) and, later, the tool surface.
//!
//! `build.rs` mounts this file via `#[path]`, so it must stay free of crate imports.
//! Codegen prunes by path, so every method on a listed path is generated.

/// `(HTTP method, spec path)` pairs, matching `specs/komga.openapi.json`.
pub const OPERATIONS: &[(&str, &str)] = &[
    ("get", "/api/v1/libraries"),
    ("get", "/api/v1/libraries/{libraryId}"),
    ("patch", "/api/v1/libraries/{libraryId}"),
    ("post", "/api/v1/libraries/{libraryId}/scan"),
    ("get", "/api/v1/series"),
    ("get", "/api/v1/series/{seriesId}"),
    ("get", "/api/v1/series/{seriesId}/books"),
    ("patch", "/api/v1/series/{seriesId}/metadata"),
    ("get", "/api/v1/books"),
    ("get", "/api/v1/books/{bookId}"),
    ("patch", "/api/v1/books/{bookId}/metadata"),
    ("delete", "/api/v1/books/{bookId}/file"),
    ("get", "/api/v1/readlists"),
    ("post", "/api/v1/readlists"),
    ("patch", "/api/v1/readlists/{id}"),
];

/// The distinct paths of [`OPERATIONS`], in first-seen order.
pub fn paths() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for (_, path) in OPERATIONS {
        if !out.contains(path) {
            out.push(path);
        }
    }
    out
}
