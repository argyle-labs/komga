//! Library analysis for `komga.library.audit`. Pure over fetched Komga
//! records, so every rule is testable without a server.

use std::collections::{BTreeMap, BTreeSet};

use plugin_toolkit::prelude::*;

use crate::api::{Book, Library, Series};
use crate::parse::{self, Edge, FileName, Kind};

/// A prefix this unlike the series name, and not containing it, is flagged.
const MISFILED_SIMILARITY: f64 = 0.6;
/// Gap detection skips series this small; one-shots and pairs have no run.
const GAP_MIN_BOOKS: usize = 3;
/// Issue numbers at or above this are treated as anniversary/legacy numbering
/// and left out of gap ranges.
const GAP_MAX_NUMBER: f64 = 1000.0;

/// Everything fetched for one library (or one series of it).
pub struct LibraryData {
    pub library: Library,
    pub series: Vec<Series>,
    pub books: Vec<Book>,
}

/// One book with its filename read.
pub struct Entry<'a> {
    pub book: &'a Book,
    pub name: FileName,
    /// Similarity of the filename prefix to the series name when flagged as
    /// probably belonging to another series.
    pub misfiled: Option<f64>,
}

impl Entry<'_> {
    fn file(&self) -> &str {
        self.book.file_name()
    }

    fn file_ref(&self) -> FileRef {
        FileRef {
            book_id: self.book.id.clone(),
            file: self.file().to_string(),
            size_bytes: self.book.size_bytes,
        }
    }

    fn locked(&self) -> bool {
        self.book.metadata.number_lock || self.book.metadata.number_sort_lock
    }

    /// Parsed issue number that disagrees with Komga's number or sort number.
    /// `045` and `45` agree.
    fn misnumbered(&self) -> bool {
        match &self.name.parsed {
            Some(p) if p.kind == Kind::Issue => {
                let same = |n: f64| (n - p.number).abs() <= 1e-6;
                !same(self.book.metadata.number_sort)
                    || !self.book.metadata.number.trim().parse().is_ok_and(same)
            }
            _ => false,
        }
    }

    /// Safe to pair with another file of the same number.
    fn pairable(&self) -> bool {
        self.misfiled.is_none()
            && self
                .name
                .parsed
                .as_ref()
                .is_some_and(|p| p.edge.is_none_or(Edge::pairs))
    }

    fn comic_key(&self) -> parse::ComicKey {
        parse::comic_key(self.name.parsed.as_ref().map_or("", |p| p.prefix.as_str()))
    }

    /// Two files of one comic and number are the same issue unless their
    /// cover years are over a year apart (a relaunch). Generic renames carry
    /// the series year, not the cover year, so their year says nothing.
    fn same_run(&self, other: &Entry<'_>) -> bool {
        if parse::is_generic(self.file()) || parse::is_generic(other.file()) {
            return true;
        }
        match (
            parse::tag_year(&self.name.tags),
            parse::tag_year(&other.name.tags),
        ) {
            (Some(a), Some(b)) => (a - b).abs() <= 1,
            _ => true,
        }
    }
}

pub struct SeriesView<'a> {
    pub series: &'a Series,
    /// Books Komga has on disk and has read.
    pub entries: Vec<Entry<'a>>,
    /// Soft-deleted or unreadable books; never planned against.
    pub unavailable: Vec<&'a Book>,
}

pub fn views<'a>(data: &'a LibraryData) -> Vec<SeriesView<'a>> {
    let mut by_series: BTreeMap<&str, Vec<&Book>> = BTreeMap::new();
    for b in &data.books {
        by_series.entry(b.series_id.as_str()).or_default().push(b);
    }
    data.series
        .iter()
        .map(|s| {
            let (ok, unavailable): (Vec<&Book>, Vec<&Book>) = by_series
                .remove(s.id.as_str())
                .unwrap_or_default()
                .into_iter()
                .partition(|b| b.available());
            SeriesView {
                series: s,
                entries: ok.into_iter().map(|b| entry(s, b)).collect(),
                unavailable,
            }
        })
        .collect()
}

fn entry<'a>(series: &Series, book: &'a Book) -> Entry<'a> {
    let mut name = parse::parse(book.file_name());
    let series_numbers = parse::name_numbers(&series.name);
    let annual_series = series.name.to_lowercase().contains("annual");
    if let Some(p) = name.parsed.as_mut() {
        if p.edge.is_none() {
            if p.sole_number && series_numbers.contains(&p.number) {
                p.edge = Some(Edge::SeriesNumber);
            } else if name.annual && !annual_series {
                p.edge = Some(Edge::Annual);
            }
        }
    }
    let misfiled = name.parsed.as_ref().and_then(|p| {
        let prefix = parse::normalize(&p.prefix);
        let series = parse::normalize(&series.name);
        let sim = parse::similarity(&prefix, &series);
        (!prefix.is_empty() && sim < MISFILED_SIMILARITY && !prefix.contains(&series))
            .then_some(sim)
    });
    Entry {
        book,
        name,
        misfiled,
    }
}

impl SeriesView<'_> {
    fn has_issues(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(&e.name.parsed, Some(p) if p.kind == Kind::Issue))
    }

    /// Pairable books that are the same comic and number, keyed apart by kind
    /// so a `v07` never collides with issue #7. Within one number, files whose
    /// series parts or cover years disagree form separate clusters. Ordered issues first,
    /// then volumes, by number.
    fn clusters(&self) -> Vec<(Kind, String, Vec<&Entry<'_>>)> {
        let mut groups: BTreeMap<(u8, String), Vec<&Entry<'_>>> = BTreeMap::new();
        for e in self.entries.iter().filter(|e| e.pairable()) {
            if let Some(p) = &e.name.parsed {
                let key = match p.kind {
                    Kind::Issue => (0, p.label.to_lowercase()),
                    Kind::Volume => (1, format!("v{}", p.label)),
                };
                groups.entry(key).or_default().push(e);
            }
        }
        let mut out = Vec::new();
        for ((kind, label), es) in groups {
            let n = es[0].name.parsed.as_ref().map_or(0.0, |p| p.number);
            let mut clusters: Vec<(parse::ComicKey, Vec<&Entry<'_>>)> = Vec::new();
            for e in es {
                let key = e.comic_key();
                match clusters
                    .iter_mut()
                    .find(|(k, c)| parse::same_comic(k, &key) && c.iter().all(|m| m.same_run(e)))
                {
                    Some((_, c)) => c.push(e),
                    None => clusters.push((key, vec![e])),
                }
            }
            let kind = if kind == 0 { Kind::Issue } else { Kind::Volume };
            for (_, c) in clusters {
                out.push((kind, n, label.clone(), c));
            }
        }
        out.sort_by(|a, b| {
            (a.0 == Kind::Volume)
                .cmp(&(b.0 == Kind::Volume))
                .then(a.1.total_cmp(&b.1))
        });
        out.into_iter().map(|(k, _, l, v)| (k, l, v)).collect()
    }

    fn duplicate_groups(&self) -> Vec<(String, Vec<&Entry<'_>>)> {
        self.clusters()
            .into_iter()
            .filter(|(_, _, v)| v.len() > 1)
            .map(|(_, l, v)| (l, v))
            .collect()
    }

    fn gaps(&self) -> Vec<IssueRange> {
        if self.entries.len() < GAP_MIN_BOOKS {
            return Vec::new();
        }
        let nums: BTreeSet<i64> = self
            .entries
            .iter()
            .filter_map(|e| e.name.parsed.as_ref())
            .filter(|p| {
                p.kind == Kind::Issue
                    && p.edge.is_none()
                    && p.number >= 0.0
                    && p.number < GAP_MAX_NUMBER
            })
            .map(|p| p.number as i64)
            .collect();
        let (Some(&first), Some(&last)) = (nums.first(), nums.last()) else {
            return Vec::new();
        };
        let lo = if first <= 5 { 1 } else { first };
        let mut out: Vec<IssueRange> = Vec::new();
        for n in (lo..=last).filter(|n| !nums.contains(n)) {
            match out.last_mut() {
                Some(r) if r.to == n - 1 => r.to = n,
                _ => out.push(IssueRange { from: n, to: n }),
            }
        }
        out
    }
}

// ── audit ──────────────────────────────────────────────────────────────────

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct FileRef {
    pub book_id: String,
    pub file: String,
    pub size_bytes: i64,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct Misnumbered {
    pub book_id: String,
    pub file: String,
    /// Komga's number and sort number now.
    pub current_number: String,
    pub current_sort: f64,
    /// The number the filename carries.
    pub parsed_number: String,
    pub parsed_sort: f64,
    /// Komga's number or sort number is locked.
    pub locked: bool,
    /// Set when the number is left for a human: `decimal`, `suffixed`,
    /// `ambiguous`, `facsimile`, `annual`, `series_number`.
    pub edge: Option<String>,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct DuplicateGroup {
    /// The shared number; `v<N>` for collected-edition volumes.
    pub number: String,
    pub books: Vec<FileRef>,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct Misfiled {
    pub book_id: String,
    pub file: String,
    /// difflib-style ratio of the filename's series part to the series name.
    pub similarity: f64,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeCase {
    pub book_id: String,
    pub file: String,
    /// `volume`, `collected_edition` (a volume in a series of single issues),
    /// or an issue edge: `decimal`, `suffixed`, `ambiguous`, `facsimile`,
    /// `annual`, `series_number`.
    pub kind: String,
    pub parsed_number: String,
}

/// A book Komga lists but cannot serve: soft-deleted (file gone) or not
/// `READY`. Left out of every plan.
#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct Unavailable {
    pub book_id: String,
    pub file: String,
    pub deleted: bool,
    pub media_status: String,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRange {
    pub from: i64,
    pub to: i64,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesAudit {
    pub series_id: String,
    pub series_name: String,
    pub books: usize,
    pub misnumbered: Vec<Misnumbered>,
    pub duplicates: Vec<DuplicateGroup>,
    pub misfiled: Vec<Misfiled>,
    pub unparseable: Vec<FileRef>,
    pub edge_cases: Vec<EdgeCase>,
    pub unavailable: Vec<Unavailable>,
    /// Issue numbers missing between #1 (or the first issue, when it is past
    /// #5) and the highest.
    pub gaps: Vec<IssueRange>,
}

impl SeriesAudit {
    fn has_findings(&self) -> bool {
        !(self.misnumbered.is_empty()
            && self.duplicates.is_empty()
            && self.misfiled.is_empty()
            && self.unparseable.is_empty()
            && self.edge_cases.is_empty()
            && self.unavailable.is_empty()
            && self.gaps.is_empty())
    }
}

#[orca_struct]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditSummary {
    pub libraries: usize,
    pub series: usize,
    pub books: usize,
    pub misnumbered_books: usize,
    pub misnumbered_series: usize,
    pub duplicate_numbers: usize,
    pub duplicate_series: usize,
    pub misfiled_books: usize,
    pub unparseable_books: usize,
    pub edge_case_books: usize,
    pub collected_editions: usize,
    pub unavailable_books: usize,
    pub series_with_gaps: usize,
    pub missing_issues: i64,
}

impl AuditSummary {
    fn add(&mut self, o: &AuditSummary) {
        self.libraries += o.libraries;
        self.series += o.series;
        self.books += o.books;
        self.misnumbered_books += o.misnumbered_books;
        self.misnumbered_series += o.misnumbered_series;
        self.duplicate_numbers += o.duplicate_numbers;
        self.duplicate_series += o.duplicate_series;
        self.misfiled_books += o.misfiled_books;
        self.unparseable_books += o.unparseable_books;
        self.edge_case_books += o.edge_case_books;
        self.collected_editions += o.collected_editions;
        self.unavailable_books += o.unavailable_books;
        self.series_with_gaps += o.series_with_gaps;
        self.missing_issues += o.missing_issues;
    }
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryAudit {
    pub library_id: String,
    pub library_name: String,
    pub summary: AuditSummary,
    /// Series with at least one finding.
    pub series: Vec<SeriesAudit>,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct AuditReport {
    pub name: String,
    pub summary: AuditSummary,
    pub libraries: Vec<LibraryAudit>,
}

pub fn audit(name: &str, data: &[LibraryData]) -> AuditReport {
    let libraries: Vec<LibraryAudit> = data.iter().map(audit_library).collect();
    let mut summary = AuditSummary::default();
    for l in &libraries {
        summary.add(&l.summary);
    }
    AuditReport {
        name: name.to_string(),
        summary,
        libraries,
    }
}

fn audit_library(data: &LibraryData) -> LibraryAudit {
    let mut summary = AuditSummary {
        libraries: 1,
        ..Default::default()
    };
    let mut series = Vec::new();
    for v in views(data) {
        let a = audit_series(&v);
        summary.series += 1;
        summary.books += a.books;
        summary.misnumbered_books += a.misnumbered.len();
        summary.misnumbered_series += usize::from(!a.misnumbered.is_empty());
        summary.duplicate_numbers += a.duplicates.len();
        summary.duplicate_series += usize::from(!a.duplicates.is_empty());
        summary.misfiled_books += a.misfiled.len();
        summary.unparseable_books += a.unparseable.len();
        summary.edge_case_books += a.edge_cases.len();
        summary.collected_editions += a
            .edge_cases
            .iter()
            .filter(|e| e.kind == "collected_edition")
            .count();
        summary.unavailable_books += a.unavailable.len();
        summary.series_with_gaps += usize::from(!a.gaps.is_empty());
        summary.missing_issues += a.gaps.iter().map(|r| r.to - r.from + 1).sum::<i64>();
        if a.has_findings() {
            series.push(a);
        }
    }
    LibraryAudit {
        library_id: data.library.id.clone(),
        library_name: data.library.name.clone(),
        summary,
        series,
    }
}

fn audit_series(v: &SeriesView<'_>) -> SeriesAudit {
    let issue_series = v.has_issues();
    let mut a = SeriesAudit {
        series_id: v.series.id.clone(),
        series_name: v.series.name.clone(),
        books: v.entries.len() + v.unavailable.len(),
        misnumbered: Vec::new(),
        duplicates: v
            .duplicate_groups()
            .into_iter()
            .map(|(number, es)| DuplicateGroup {
                number,
                books: es.iter().map(|e| e.file_ref()).collect(),
            })
            .collect(),
        misfiled: Vec::new(),
        unparseable: Vec::new(),
        edge_cases: Vec::new(),
        unavailable: v
            .unavailable
            .iter()
            .map(|b| Unavailable {
                book_id: b.id.clone(),
                file: b.file_name().to_string(),
                deleted: b.deleted,
                media_status: b.media_status().to_string(),
            })
            .collect(),
        gaps: v.gaps(),
    };
    for e in &v.entries {
        let Some(p) = &e.name.parsed else {
            a.unparseable.push(e.file_ref());
            continue;
        };
        if e.misnumbered() {
            a.misnumbered.push(Misnumbered {
                book_id: e.book.id.clone(),
                file: e.file().to_string(),
                current_number: e.book.metadata.number.clone(),
                current_sort: e.book.metadata.number_sort,
                parsed_number: p.label.clone(),
                parsed_sort: p.number,
                locked: e.locked(),
                edge: p.edge.map(|x| x.as_str().to_string()),
            });
        }
        let kind = match (p.kind, p.edge) {
            (Kind::Volume, _) if issue_series => Some("collected_edition"),
            (Kind::Volume, _) => Some("volume"),
            (Kind::Issue, Some(edge)) => Some(edge.as_str()),
            (Kind::Issue, None) => None,
        };
        if let Some(kind) = kind {
            a.edge_cases.push(EdgeCase {
                book_id: e.book.id.clone(),
                file: e.file().to_string(),
                kind: kind.to_string(),
                parsed_number: p.label.clone(),
            });
        }
        if let Some(sim) = e.misfiled {
            a.misfiled.push(Misfiled {
                book_id: e.book.id.clone(),
                file: e.file().to_string(),
                similarity: (sim * 100.0).round() / 100.0,
            });
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::BookMetadata;

    fn book(id: &str, series: &str, file: &str, sort: f64, size: i64) -> Book {
        Book {
            id: id.into(),
            series_id: series.into(),
            url: format!("/data/media/comics/x/{file}"),
            size_bytes: size,
            metadata: BookMetadata {
                number: crate::parse::canonical(sort),
                number_sort: sort,
                number_lock: false,
                number_sort_lock: false,
            },
            deleted: false,
            media: Some(crate::api::Media {
                status: "READY".into(),
            }),
        }
    }

    fn lib(series: &[(&str, &str)], books: Vec<Book>) -> LibraryData {
        LibraryData {
            library: Library {
                id: "L1".into(),
                name: "comics".into(),
                empty_trash_after_scan: true,
            },
            series: series
                .iter()
                .map(|(id, name)| Series {
                    id: (*id).into(),
                    library_id: "L1".into(),
                    name: (*name).into(),
                })
                .collect(),
            books,
        }
    }

    #[test]
    fn audit_finds_misnumbered_gaps_and_duplicates() {
        let data = lib(
            &[("S1", "Saga (2012)")],
            vec![
                book("a", "S1", "Saga 03 (2012).cbz", 1.0, 1),
                book("b", "S1", "Saga_004_(2012).cbz", 2.0, 1),
                book("c", "S1", "Saga 007 (2012).cbz", 3.0, 1),
                book("d", "S1", "Saga 07 (2012).cbz", 4.0, 1),
            ],
        );
        let r = audit("k", &[data]);
        assert_eq!(r.summary.misnumbered_books, 4);
        assert_eq!(r.summary.duplicate_numbers, 1);
        let s = &r.libraries[0].series[0];
        assert_eq!(s.duplicates[0].number, "7");
        assert_eq!(
            s.gaps,
            vec![IssueRange { from: 1, to: 2 }, IssueRange { from: 5, to: 6 }]
        );
        assert_eq!(r.summary.missing_issues, 4);
    }
}
