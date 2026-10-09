//! `komga.readlist.sync` planning: resolve an ordered entry list to book ids
//! and diff it against the existing readlist. Pure over fetched Komga
//! records, so every rule is testable without a server.

use std::collections::{BTreeMap, BTreeSet};

use plugin_toolkit::prelude::*;
use plugin_toolkit::serde_json;

use crate::api::{Book, ReadList, Series};
use crate::parse;

/// One readlist slot: a series (name or id) and issue number, or a book id.
#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadlistEntry {
    #[serde(default)]
    pub series: Option<String>,
    #[serde(default)]
    pub number: Option<String>,
    #[serde(default)]
    pub book: Option<String>,
}

impl ReadlistEntry {
    pub fn label(&self) -> String {
        match (&self.book, &self.series, &self.number) {
            (Some(b), _, _) => format!("book {b}"),
            (None, Some(s), Some(n)) => format!("{s} #{n}"),
            _ => "invalid entry".into(),
        }
    }
}

/// CLI form: a JSON object, or `<series>#<number>`.
pub fn parse_entry(s: &str) -> std::result::Result<ReadlistEntry, String> {
    let t = s.trim();
    if t.starts_with('{') {
        return serde_json::from_str(t).map_err(|e| format!("entry {t}: {e}"));
    }
    match t.rsplit_once('#') {
        Some((series, number)) if !series.trim().is_empty() && !number.trim().is_empty() => {
            Ok(ReadlistEntry {
                series: Some(series.trim().into()),
                number: Some(number.trim().into()),
                book: None,
            })
        }
        _ => Err(format!(
            "entry '{t}': expected <series>#<number> or a JSON object"
        )),
    }
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookRef {
    pub book_id: String,
    pub series_id: String,
    pub series_name: String,
    pub number: String,
    pub file: String,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEntry {
    /// 1-based position in the input.
    pub position: usize,
    pub entry: String,
    pub book: BookRef,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedEntry {
    pub position: usize,
    pub entry: String,
    pub error: String,
    /// The matching books when more than one matched; none is picked.
    pub candidates: Vec<BookRef>,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadlistDiff {
    /// `create`, `update`, or `none`.
    pub action: String,
    pub readlist_id: Option<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Books kept in both lists but in a different relative order.
    pub reordered: bool,
    pub summary_changed: bool,
    /// The existing readlist is unordered; the write sets it ordered.
    pub ordered_changed: bool,
    /// The existing readlist's books as seen by this plan; pass them as
    /// `expected_current` to execute.
    pub current: Vec<String>,
}

/// Fetched inputs: series in scope, books per needed series id, and books
/// looked up directly by id (`Err` text when the lookup failed). `scope`
/// names the libraries searched; `foreign` holds series keys that are ids of
/// series outside them.
pub struct Catalog {
    pub scope: String,
    pub foreign: BTreeSet<String>,
    pub series: Vec<Series>,
    pub books: BTreeMap<String, Vec<Book>>,
    pub direct: BTreeMap<String, std::result::Result<Book, String>>,
}

pub enum SeriesMatch<'a> {
    One(&'a Series),
    None,
    Many(Vec<&'a Series>),
}

/// Exact id, else exact folder name or metadata title (case-insensitive).
/// A near match is not a match.
pub fn find_series<'a>(series: &'a [Series], key: &str) -> SeriesMatch<'a> {
    let key = key.trim();
    if let Some(s) = series.iter().find(|s| s.id == key) {
        return SeriesMatch::One(s);
    }
    let named: Vec<&Series> = series
        .iter()
        .filter(|s| {
            s.name.trim().eq_ignore_ascii_case(key)
                || s.metadata.title.trim().eq_ignore_ascii_case(key)
        })
        .collect();
    match named.len() {
        0 => SeriesMatch::None,
        1 => SeriesMatch::One(named[0]),
        _ => SeriesMatch::Many(named),
    }
}

/// Digits with an optional decimal part; `f64` parsing alone would also
/// accept `nan`, `inf`, and `1e2`.
fn numeric(s: &str) -> Option<f64> {
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let ok = match s.split_once('.') {
        Some((int, frac)) => digits(int) && digits(frac),
        None => digits(s),
    };
    ok.then(|| s.parse().ok()).flatten()
}

/// `"001"` and `"1.0"` equal `"1"`; non-numeric issues compare as text.
fn same_number(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    match (numeric(a), numeric(b)) {
        (Some(x), Some(y)) => parse::canonical(x) == parse::canonical(y),
        _ => a.eq_ignore_ascii_case(b),
    }
}

fn book_ref(series_name: &str, b: &Book) -> BookRef {
    BookRef {
        book_id: b.id.clone(),
        series_id: b.series_id.clone(),
        series_name: series_name.into(),
        number: b.metadata.number.clone(),
        file: b.file_name().into(),
    }
}

fn series_name<'a>(series: &'a [Series], id: &str) -> &'a str {
    series
        .iter()
        .find(|s| s.id == id)
        .map_or("", |s| s.name.as_str())
}

pub fn resolve(
    entries: &[ReadlistEntry],
    cat: &Catalog,
) -> (Vec<ResolvedEntry>, Vec<UnresolvedEntry>) {
    let mut resolved: Vec<ResolvedEntry> = Vec::new();
    let mut unresolved = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let position = i + 1;
        let fail = |error: String, candidates: Vec<BookRef>| UnresolvedEntry {
            position,
            entry: entry.label(),
            error,
            candidates,
        };
        let book = match resolve_one(entry, cat) {
            Ok(b) => b,
            Err((error, candidates)) => {
                unresolved.push(fail(error, candidates));
                continue;
            }
        };
        // Komga keeps a book once per readlist; a repeat would silently vanish.
        if let Some(first) = resolved.iter().find(|r| r.book.book_id == book.book_id) {
            unresolved.push(fail(
                format!("same book as entry {}", first.position),
                Vec::new(),
            ));
            continue;
        }
        resolved.push(ResolvedEntry {
            position,
            entry: entry.label(),
            book,
        });
    }
    (resolved, unresolved)
}

fn resolve_one(
    entry: &ReadlistEntry,
    cat: &Catalog,
) -> std::result::Result<BookRef, (String, Vec<BookRef>)> {
    let none = |e: String| (e, Vec::new());
    match (&entry.book, &entry.series, &entry.number) {
        (Some(id), None, None) => match cat.direct.get(id.as_str()) {
            Some(Ok(b)) if b.available() => Ok(book_ref(series_name(&cat.series, &b.series_id), b)),
            Some(Ok(b)) => Err(none(unavailable(b))),
            Some(Err(e)) => Err(none(e.clone())),
            None => Err(none("book was not looked up".into())),
        },
        (None, Some(key), Some(number)) => {
            let s = match find_series(&cat.series, key) {
                SeriesMatch::One(s) => s,
                SeriesMatch::None if cat.foreign.contains(key.trim()) => {
                    return Err(none(format!(
                        "series '{key}' isn't in library '{}'",
                        cat.scope
                    )))
                }
                SeriesMatch::None => {
                    return Err(none(format!(
                        "no series named '{key}' in library '{}'",
                        cat.scope
                    )))
                }
                SeriesMatch::Many(m) => {
                    let ids: Vec<String> = m.iter().map(|s| s.id.clone()).collect();
                    return Err(none(format!(
                        "ambiguous: {} series named or titled '{key}': {}",
                        m.len(),
                        ids.join(", ")
                    )));
                }
            };
            let hits: Vec<&Book> = cat
                .books
                .get(&s.id)
                .into_iter()
                .flatten()
                .filter(|b| !b.deleted && same_number(&b.metadata.number, number))
                .collect();
            match hits.as_slice() {
                [] => Err(none(format!("{} has no #{number}", s.name))),
                [b] if b.available() => Ok(book_ref(&s.name, b)),
                [b] => Err(none(unavailable(b))),
                many => Err((
                    format!("ambiguous: {} books are {} #{number}", many.len(), s.name),
                    many.iter().map(|b| book_ref(&s.name, b)).collect(),
                )),
            }
        }
        _ => Err(none("an entry is either {series, number} or {book}".into())),
    }
}

fn unavailable(b: &Book) -> String {
    if b.deleted {
        "book is deleted".into()
    } else {
        format!("book is unavailable (media {})", b.media_status())
    }
}

pub fn diff(existing: Option<&ReadList>, want: &[String], summary: Option<&str>) -> ReadlistDiff {
    let Some(rl) = existing else {
        return ReadlistDiff {
            action: "create".into(),
            readlist_id: None,
            added: want.to_vec(),
            removed: Vec::new(),
            reordered: false,
            summary_changed: summary.is_some_and(|s| !s.is_empty()),
            ordered_changed: false,
            current: Vec::new(),
        };
    };
    let have: BTreeSet<&str> = rl.book_ids.iter().map(String::as_str).collect();
    let wanted: BTreeSet<&str> = want.iter().map(String::as_str).collect();
    let added: Vec<String> = want
        .iter()
        .filter(|id| !have.contains(id.as_str()))
        .cloned()
        .collect();
    let removed: Vec<String> = rl
        .book_ids
        .iter()
        .filter(|id| !wanted.contains(id.as_str()))
        .cloned()
        .collect();
    let kept_old: Vec<&String> = rl
        .book_ids
        .iter()
        .filter(|id| wanted.contains(id.as_str()))
        .collect();
    let kept_new: Vec<&String> = want
        .iter()
        .filter(|id| have.contains(id.as_str()))
        .collect();
    let reordered = kept_old != kept_new;
    let summary_changed = summary.is_some_and(|s| s != rl.summary);
    let ordered_changed = !rl.ordered;
    let changed =
        !added.is_empty() || !removed.is_empty() || reordered || summary_changed || ordered_changed;
    ReadlistDiff {
        action: if changed { "update" } else { "none" }.into(),
        readlist_id: Some(rl.id.clone()),
        added,
        removed,
        reordered,
        summary_changed,
        ordered_changed,
        current: rl.book_ids.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{BookMetadata, Media};

    fn series(id: &str, name: &str) -> Series {
        Series {
            id: id.into(),
            library_id: "L".into(),
            name: name.into(),
            metadata: Default::default(),
        }
    }

    fn book(id: &str, series_id: &str, number: &str) -> Book {
        Book {
            id: id.into(),
            series_id: series_id.into(),
            url: format!("/comics/{id}.cbz"),
            size_bytes: 1,
            metadata: BookMetadata {
                number: number.into(),
                number_sort: 0.0,
                number_lock: false,
                number_sort_lock: false,
            },
            deleted: false,
            media: Some(Media {
                status: "READY".into(),
            }),
        }
    }

    fn sn(s: &str, n: &str) -> ReadlistEntry {
        ReadlistEntry {
            series: Some(s.into()),
            number: Some(n.into()),
            book: None,
        }
    }

    fn by_id(id: &str) -> ReadlistEntry {
        ReadlistEntry {
            series: None,
            number: None,
            book: Some(id.into()),
        }
    }

    fn catalog() -> Catalog {
        let mut gone = book("gone", "A", "3");
        gone.deleted = true;
        let mut titled = series("E", "Folder E");
        titled.metadata.title = "Titled".into();
        let mut broken = book("b2", "B", "2");
        broken.media = Some(Media {
            status: "ERROR".into(),
        });
        let mut unsupported = book("bu", "B", "9");
        unsupported.media = Some(Media {
            status: "UNSUPPORTED".into(),
        });
        Catalog {
            scope: "comics".into(),
            foreign: BTreeSet::from(["X9".to_string()]),
            series: vec![
                series("A", "Secret Wars (2015)"),
                series("B", "Secret Wars (1984)"),
                series("C", "Dup"),
                series("D", "Dup"),
                titled,
                series("F", "Titled"),
                series("G", "A"),
            ],
            books: BTreeMap::from([
                (
                    "A".into(),
                    vec![
                        book("a1", "A", "1"),
                        book("a2", "A", "2"),
                        book("a2x", "A", "2"),
                        gone,
                    ],
                ),
                (
                    "B".into(),
                    vec![book("b1", "B", "1"), broken, book("ann", "B", "Annual 1")],
                ),
            ]),
            direct: BTreeMap::from([
                ("b1".into(), Ok(book("b1", "B", "1"))),
                ("bu".into(), Ok(unsupported)),
                ("zz".into(), Err("komga books/zz: http 404".into())),
            ]),
        }
    }

    #[test]
    fn resolves_in_order_by_name_id_and_book() {
        let entries = [sn("secret wars (2015)", "001"), sn("B", "1.0")];
        let (ok, bad) = resolve(&entries, &catalog());
        assert!(bad.is_empty(), "{bad:?}");
        let ids: Vec<&str> = ok.iter().map(|r| r.book.book_id.as_str()).collect();
        assert_eq!(ids, ["a1", "b1"]);
        assert_eq!(ok[1].book.series_name, "Secret Wars (1984)");
    }

    #[test]
    fn ambiguous_books_are_listed_never_picked() {
        let (ok, bad) = resolve(&[sn("Secret Wars (2015)", "2")], &catalog());
        assert!(ok.is_empty());
        assert!(bad[0].error.starts_with("ambiguous"), "{}", bad[0].error);
        let c: Vec<&str> = bad[0]
            .candidates
            .iter()
            .map(|c| c.book_id.as_str())
            .collect();
        assert_eq!(c, ["a2", "a2x"]);
    }

    #[test]
    fn misses_are_reported() {
        let entries = [
            sn("Secret Wars", "1"),
            sn("Dup", "1"),
            sn("Secret Wars (2015)", "3"),
            by_id("zz"),
            by_id("b1"),
            sn("B", "1"),
            ReadlistEntry {
                series: Some("A".into()),
                number: None,
                book: None,
            },
        ];
        let (ok, bad) = resolve(&entries, &catalog());
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].position, 5);
        let errs: Vec<(usize, &str)> = bad.iter().map(|u| (u.position, u.error.as_str())).collect();
        assert!(errs[0].1.starts_with("no series named"));
        assert!(errs[1].1.starts_with("ambiguous: 2 series"));
        assert!(errs[2].1.contains("has no #3"), "deleted books never match");
        assert!(errs[3].1.contains("404"));
        assert_eq!(errs[4], (6, "same book as entry 5"));
        assert_eq!(errs[5].0, 7);
    }

    #[test]
    fn series_id_beats_a_series_named_that_id() {
        assert!(matches!(find_series(&catalog().series, "A"), SeriesMatch::One(s) if s.id == "A"));
    }

    #[test]
    fn title_and_folder_name_both_match() {
        let cat = catalog();
        assert!(matches!(find_series(&cat.series, "folder e"), SeriesMatch::One(s) if s.id == "E"));
        let (_, bad) = resolve(&[sn("titled", "1")], &cat);
        assert!(bad[0].error.contains("E, F"), "{}", bad[0].error);
    }

    #[test]
    fn numbers_are_digits_or_exact_text() {
        assert!(same_number("001", "1.0"));
        assert!(same_number("Annual 1", "annual 1"));
        assert!(!same_number("Annual 1", "1"));
        for odd in ["nan", "inf", "1e2", "1.", ".5", "-1"] {
            assert_eq!(numeric(odd), None, "{odd}");
        }
        assert!(!same_number("1e2", "100"));
        let (ok, _) = resolve(&[sn("B", "annual 1")], &catalog());
        assert_eq!(ok[0].book.book_id, "ann");
    }

    #[test]
    fn unavailable_books_are_unresolved() {
        let (ok, bad) = resolve(&[sn("B", "2"), by_id("bu")], &catalog());
        assert!(ok.is_empty());
        assert!(bad[0].error.contains("media ERROR"), "{}", bad[0].error);
        assert!(
            bad[1].error.contains("media UNSUPPORTED"),
            "{}",
            bad[1].error
        );
    }

    #[test]
    fn series_outside_the_library_says_so() {
        let (_, bad) = resolve(&[sn("X9", "1")], &catalog());
        assert_eq!(bad[0].error, "series 'X9' isn't in library 'comics'");
    }

    #[test]
    fn parses_cli_entries() {
        assert_eq!(
            parse_entry("Secret Wars (2015) #3").unwrap(),
            sn("Secret Wars (2015)", "3")
        );
        assert_eq!(parse_entry(r#"{"book":"x1"}"#).unwrap(), by_id("x1"));
        assert!(parse_entry("Secret Wars").is_err());
    }

    fn rl(ids: &[&str], summary: &str) -> ReadList {
        ReadList {
            id: "R".into(),
            name: "n".into(),
            summary: summary.into(),
            ordered: true,
            book_ids: ids.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn diff_create_update_none() {
        let d = diff(None, &strs(&["a", "b"]), None);
        assert_eq!(d.action, "create");
        assert_eq!(d.added, ["a", "b"]);

        let old = rl(&["a", "b", "c"], "s");
        let d = diff(Some(&old), &strs(&["c", "a", "d"]), None);
        assert_eq!(d.action, "update");
        assert_eq!(d.added, ["d"]);
        assert_eq!(d.removed, ["b"]);
        assert!(d.reordered);

        let d = diff(Some(&old), &strs(&["a", "c"]), Some("s"));
        assert!(!d.reordered && !d.summary_changed);
        assert_eq!(d.removed, ["b"]);

        let d = diff(Some(&old), &strs(&["a", "b", "c"]), None);
        assert_eq!(d.action, "none");
        assert!(diff(Some(&old), &strs(&["a", "b", "c"]), Some("t")).summary_changed);
        assert_eq!(d.current, ["a", "b", "c"]);
    }

    #[test]
    fn unordered_readlist_is_a_change() {
        let mut old = rl(&["a"], "");
        old.ordered = false;
        let d = diff(Some(&old), &strs(&["a"]), None);
        assert_eq!(d.action, "update");
        assert!(d.ordered_changed);
    }
}
