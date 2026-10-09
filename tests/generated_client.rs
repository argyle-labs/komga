//! The generated Komga client covers the curated operation list, and that list
//! still matches the vendored spec.

use komga::komga_api::{types, Client};
use komga::operations::OPERATIONS;
use plugin_toolkit::serde_json;

#[test]
fn client_constructs() {
    Client::new("https://komga.example/", "key", false).unwrap();
}

#[test]
fn vendored_spec_names_no_real_host() {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/specs/komga.openapi.json"
    ))
    .unwrap();
    let spec: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        spec["servers"],
        serde_json::json!([{"url": "https://komga.example"}])
    );
}

#[test]
fn every_listed_operation_is_in_the_vendored_spec() {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/specs/komga.openapi.json"
    ))
    .unwrap();
    let spec: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for (method, path) in OPERATIONS {
        assert!(
            spec["paths"][path][method].is_object(),
            "{method} {path} missing from specs/komga.openapi.json"
        );
    }
}

/// Each operation in the curated list has a generated method (checked at
/// compile time), and the count ties this list to [`OPERATIONS`].
macro_rules! assert_methods {
    ($($m:ident),* $(,)?) => {{
        type Generated = <Client as std::ops::Deref>::Target;
        $(let _ = Generated::$m;)*
        [$(stringify!($m)),*].len()
    }};
}

#[test]
fn generated_methods_exist() {
    let count = assert_methods!(
        get_libraries,
        get_library_by_id,
        update_library_by_id,
        library_scan,
        get_series_deprecated,
        get_series_by_id,
        get_books_by_series_id,
        get_all_books_deprecated,
        get_book_by_id,
        update_book_metadata,
        delete_book_file,
        get_read_lists,
        create_read_list,
        update_read_list_by_id,
    );
    assert_eq!(count, OPERATIONS.len());
    let _: Option<types::BookMetadataUpdateDto> = None;
    let _: Option<types::ReadListUpdateDto> = None;
}
