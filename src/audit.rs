//! Library analysis and the plans built from it: `komga.library.audit`,
//! `komga.books.renumber`, `komga.books.dedupe`. Pure over fetched Komga
//! records, so every rule is testable without a server.

use std::cmp::Ordering;
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
    /// Mis-numbered books `komga.books.renumber` would fix without `force`.
    pub renumberable_books: usize,
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
        self.renumberable_books += o.renumberable_books;
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
        summary.renumberable_books += v
            .entries
            .iter()
            .filter(|e| renumber_skip(e, false).is_none())
            .count();
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

// ── renumber ───────────────────────────────────────────────────────────────

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct Renumber {
    pub book_id: String,
    pub series_name: String,
    pub file: String,
    pub current_number: String,
    pub current_sort: f64,
    /// Written as `number` with `numberLock`.
    pub number: String,
    /// Written as `numberSort` with `numberSortLock`.
    pub number_sort: f64,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub book_id: String,
    pub series_name: String,
    pub file: String,
    pub reason: String,
}

/// Why a book is not renumbered, or `None` when it is planned. Books Komga
/// already numbers right are neither.
fn renumber_skip(e: &Entry<'_>, force: bool) -> Option<Option<String>> {
    let Some(p) = &e.name.parsed else {
        return Some(Some("no number in the filename".into()));
    };
    if p.kind == Kind::Volume || !e.misnumbered() {
        return Some(None);
    }
    if let Some(edge) = p.edge {
        let why = match edge {
            Edge::Decimal => "decimal issue number",
            Edge::Suffixed => "number carries a suffix",
            Edge::Ambiguous => "#N with another number after it",
            Edge::Facsimile => "facsimile edition",
            Edge::Annual => "annual filed in a non-annual series",
            Edge::SeriesNumber => "the only number is part of the series name",
        };
        return Some(Some(format!("{why} ({})", p.label)));
    }
    if let Some(sim) = e.misfiled {
        return Some(Some(format!(
            "filename looks like another series (similarity {sim:.2})"
        )));
    }
    if e.locked() && !force {
        return Some(Some(format!(
            "number is locked at {}; pass force to overwrite",
            e.book.metadata.number
        )));
    }
    None
}

pub fn renumber_plan(data: &[LibraryData], force: bool) -> (Vec<Renumber>, Vec<Skipped>) {
    let mut planned = Vec::new();
    let mut skipped = Vec::new();
    for lib in data {
        for v in views(lib) {
            for e in &v.entries {
                match renumber_skip(e, force) {
                    None => {
                        let p = e.name.parsed.as_ref().expect("planned books parse");
                        planned.push(Renumber {
                            book_id: e.book.id.clone(),
                            series_name: v.series.name.clone(),
                            file: e.file().to_string(),
                            current_number: e.book.metadata.number.clone(),
                            current_sort: e.book.metadata.number_sort,
                            number: p.label.clone(),
                            number_sort: p.number,
                        });
                    }
                    Some(Some(reason)) => skipped.push(Skipped {
                        book_id: e.book.id.clone(),
                        series_name: v.series.name.clone(),
                        file: e.file().to_string(),
                        reason,
                    }),
                    Some(None) => {}
                }
            }
        }
    }
    (planned, skipped)
}

// ── dedupe ─────────────────────────────────────────────────────────────────

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct DedupeLoser {
    pub book_id: String,
    pub file: String,
    pub size_bytes: i64,
    /// Why the kept file wins over this one.
    pub reason: String,
    /// Deleting it leaves the issue with no file; execute needs `allow_orphan`.
    pub last_copy: bool,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct DedupeGroup {
    pub library_id: String,
    pub series_id: String,
    pub series_name: String,
    /// The shared number; `v<N>` for volumes.
    pub number: String,
    /// `None` when a generic purge empties the group.
    pub keep: Option<FileRef>,
    /// With no `keep`, an available file carrying this number that dedupe
    /// never pairs (a facsimile, a guessed number). Informational only: it may
    /// be another issue, so the deletes stay `last_copy`.
    pub kept_elsewhere: Option<FileRef>,
    pub delete: Vec<DedupeLoser>,
}

/// An issue a generic purge leaves with no file, for Mylar to re-acquire.
#[orca_struct]
#[derive(Debug, Clone, PartialEq)]
pub struct OrphanIssue {
    pub series_id: String,
    /// Series name without its `(YYYY)`.
    pub series: String,
    pub year: Option<i32>,
    pub issue_number: String,
}

/// Ranking inputs, best first in each field.
#[derive(Debug, Clone, PartialEq)]
struct Rank {
    /// Not a mobile/webrip source.
    full: bool,
    /// Carries `(Digital)` or a scanner/group tag; Mylar tracks these names.
    release: bool,
    digital: u8,
    /// cbz/cbr rather than pdf or another container.
    archive: bool,
    size: i64,
}

fn rank(e: &Entry<'_>) -> Rank {
    Rank {
        full: !parse::is_mobile(&e.name.tags),
        release: e.name.tags.iter().any(|t| parse::is_release_tag(t)),
        digital: parse::digital_tier(&e.name.tags),
        archive: matches!(e.name.ext.as_str(), "cbz" | "cbr"),
        size: e.book.size_bytes,
    }
}

/// `Less` when `a` should be kept over `b`.
fn keep_order(a: &Entry<'_>, b: &Entry<'_>) -> Ordering {
    let (ra, rb) = (rank(a), rank(b));
    rb.full
        .cmp(&ra.full)
        .then(rb.release.cmp(&ra.release))
        .then(rb.digital.cmp(&ra.digital))
        .then(rb.archive.cmp(&ra.archive))
        .then(rb.size.cmp(&ra.size))
        .then(a.file().cmp(b.file()))
}

fn loss_reason(keep: &Entry<'_>, lose: &Entry<'_>) -> String {
    let (k, l) = (rank(keep), rank(lose));
    if k.full != l.full {
        "this one is a mobile/webrip source".into()
    } else if k.release != l.release {
        "kept file carries a release tag (Mylar tracks it); this one has none".into()
    } else if k.digital != l.digital {
        let tier = |t: u8| match t {
            2 => "(Digital)",
            1 => "not tagged (Digital)",
            _ => "mobile/webrip",
        };
        format!(
            "kept file is {}; this one is {}",
            tier(k.digital),
            tier(l.digital)
        )
    } else if k.archive != l.archive {
        format!(
            "kept file is {}; this one is {}",
            keep.name.ext, lose.name.ext
        )
    } else if k.size != l.size {
        format!("kept file is larger ({} vs {} bytes)", k.size, l.size)
    } else {
        "identical rank; kept the first by name".into()
    }
}

/// Duplicate numbers collapse to the best-ranked copy. With `purge_generic`,
/// every generic-named issue file is deleted too, twin or not; volumes are
/// never purged.
pub fn dedupe_plan(data: &[LibraryData], purge_generic: bool) -> Vec<DedupeGroup> {
    let mut out = Vec::new();
    for lib in data {
        for v in views(lib) {
            let series_key = parse::comic_key(&v.series.name);
            let first = out.len();
            for (kind, number, es) in v.clusters() {
                let purge = purge_generic && kind == Kind::Issue;
                // A generic rename of another comic would be listed as this
                // series' orphan, so only this series' own renames are purged.
                let (generic, mut rest): (Vec<&Entry<'_>>, Vec<&Entry<'_>>) =
                    es.into_iter().partition(|e| {
                        purge
                            && parse::is_generic(e.file())
                            && parse::same_comic(&e.comic_key(), &series_key)
                    });
                rest.sort_by(|a, b| keep_order(a, b));
                let keep = rest.first().copied();
                let mut delete: Vec<DedupeLoser> = generic
                    .iter()
                    .map(|e| loser(e, "generic name (no release tag); purged".into()))
                    .collect();
                if let Some(k) = keep {
                    delete.extend(rest[1..].iter().map(|e| loser(e, loss_reason(k, e))));
                }
                if delete.is_empty() {
                    continue;
                }
                out.push(DedupeGroup {
                    library_id: lib.library.id.clone(),
                    series_id: v.series.id.clone(),
                    series_name: v.series.name.clone(),
                    number,
                    keep: keep.map(Entry::file_ref),
                    kept_elsewhere: None,
                    delete,
                });
            }
            mark_last_copies(&v, &series_key, &mut out[first..]);
        }
    }
    out
}

/// A group with no `keep` loses its last paired copy. A file clustering left
/// out (a guessed number) is noted as a possible copy but never trusted.
fn mark_last_copies(v: &SeriesView<'_>, series_key: &parse::ComicKey, groups: &mut [DedupeGroup]) {
    let deleted: BTreeSet<String> = groups
        .iter()
        .flat_map(|g| &g.delete)
        .map(|l| l.book_id.clone())
        .collect();
    for g in groups.iter_mut().filter(|g| g.keep.is_none()) {
        let mut note = String::new();
        g.kept_elsewhere = v
            .entries
            .iter()
            .find(|e| {
                !deleted.contains(&e.book.id)
                    && e.misfiled.is_none()
                    && parse::same_comic(&e.comic_key(), series_key)
                    && e.name.parsed.as_ref().is_some_and(|p| {
                        p.kind == Kind::Issue && p.label.to_lowercase() == g.number
                    })
            })
            .map(|e| {
                let edge = e.name.parsed.as_ref().and_then(|p| p.edge);
                note = edge.map_or_else(
                    || format!("; possible copy: {}", e.file()),
                    |k| format!("; possible copy: {} ({})", e.file(), k.as_str()),
                );
                e.file_ref()
            });
        for l in &mut g.delete {
            l.last_copy = true;
            l.reason.push_str("; last copy of this issue");
            l.reason.push_str(&note);
        }
    }
}

fn loser(e: &Entry<'_>, reason: String) -> DedupeLoser {
    DedupeLoser {
        book_id: e.book.id.clone(),
        file: e.file().to_string(),
        size_bytes: e.book.size_bytes,
        reason,
        last_copy: false,
    }
}

/// Issues execute would leave with no file: none without `allow_orphan`,
/// since last copies are then dropped.
pub fn orphans(groups: &[DedupeGroup], allow_orphan: bool) -> Vec<OrphanIssue> {
    groups
        .iter()
        .filter(|g| allow_orphan && g.keep.is_none())
        .map(|g| {
            let (series, year) = parse::split_year(&g.series_name);
            OrphanIssue {
                series_id: g.series_id.clone(),
                series,
                year,
                issue_number: g.number.clone(),
            }
        })
        .collect()
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
        assert_eq!(r.summary.renumberable_books, 4);
        assert_eq!(r.summary.duplicate_numbers, 1);
        let s = &r.libraries[0].series[0];
        assert_eq!(s.duplicates[0].number, "7");
        assert_eq!(
            s.gaps,
            vec![IssueRange { from: 1, to: 2 }, IssueRange { from: 5, to: 6 }]
        );
        assert_eq!(r.summary.missing_issues, 4);
    }

    #[test]
    fn renumber_plans_only_wrong_unlocked_plain_issues() {
        let mut locked = book("l", "S1", "Saga 05 (2012).cbz", 2.0, 1);
        locked.metadata.number_lock = true;
        let data = lib(
            &[("S1", "Saga (2012)")],
            vec![
                book("ok", "S1", "Saga 01 (2012).cbz", 1.0, 1),
                book("wrong", "S1", "Saga 18 (2012).cbz", 3.0, 1),
                locked,
                book("dec", "S1", "Saga 605.1 (2012).cbz", 4.0, 1),
                book("suf", "S1", "Saga 065.Deaths (2012).cbz", 5.0, 1),
                book("vol", "S1", "Saga v07 (2012).cbz", 6.0, 1),
                book("none", "S1", "Saga (2012).cbz", 7.0, 1),
                book("other", "S1", "Batman 004 (2016).cbz", 8.0, 1),
            ],
        );
        let (planned, skipped) = renumber_plan(std::slice::from_ref(&data), false);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].book_id, "wrong");
        assert_eq!(
            (planned[0].number.as_str(), planned[0].number_sort),
            ("18", 18.0)
        );
        let skipped: Vec<&str> = skipped.iter().map(|s| s.book_id.as_str()).collect();
        assert_eq!(skipped, vec!["l", "dec", "suf", "none", "other"]);

        let (planned, _) = renumber_plan(std::slice::from_ref(&data), true);
        let ids: Vec<&str> = planned.iter().map(|p| p.book_id.as_str()).collect();
        assert_eq!(ids, vec!["wrong", "l"]);
    }

    #[test]
    fn renumber_is_idempotent_once_numbers_match() {
        let mut b = book("a", "S1", "Saga 18 (2012).cbz", 18.0, 1);
        b.metadata.number_lock = true;
        b.metadata.number_sort_lock = true;
        let (planned, skipped) = renumber_plan(&[lib(&[("S1", "Saga (2012)")], vec![b])], true);
        assert!(planned.is_empty() && skipped.is_empty());
    }

    /// `dedupe_plan` plus the invariant every fixture must hold: without a
    /// purge, each group keeps a file and never deletes it.
    fn plan(data: &[LibraryData], purge: bool) -> Vec<DedupeGroup> {
        let groups = dedupe_plan(data, purge);
        let mut deleted = BTreeSet::new();
        for g in &groups {
            for l in &g.delete {
                assert_eq!(l.last_copy, g.keep.is_none(), "{g:?}");
                assert!(deleted.insert(l.book_id.clone()), "deleted twice: {l:?}");
                if let Some(k) = &g.keep {
                    assert_ne!(l.book_id, k.book_id, "keep deleted: {g:?}");
                }
            }
        }
        if !purge {
            for g in &groups {
                let keep = g.keep.as_ref().unwrap_or_else(|| panic!("no keep: {g:?}"));
                assert!(
                    g.delete.iter().all(|l| l.book_id != keep.book_id),
                    "keep deleted: {g:?}"
                );
                assert!(g.delete.iter().all(|l| !l.last_copy), "{g:?}");
            }
        }
        groups
    }

    fn dedupe(series_name: &str, books: Vec<Book>) -> Vec<DedupeGroup> {
        plan(&[lib(&[("S1", series_name)], books)], false)
    }

    #[test]
    fn release_named_beats_generic_even_when_smaller() {
        let g = dedupe(
            "The Amazing Spider-Man (2025)",
            vec![
                book(
                    "gen",
                    "S1",
                    "The Amazing Spider-Man (2025) Volume 01 Issue 008.cbz",
                    1.0,
                    900,
                ),
                book(
                    "rel",
                    "S1",
                    "Amazing Spider-Man 008 (2025) (Digital) (Shan-Empire).cbz",
                    2.0,
                    100,
                ),
            ],
        );
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].number, "8");
        assert_eq!(g[0].keep.as_ref().unwrap().book_id, "rel");
        assert_eq!(g[0].delete[0].book_id, "gen");
        assert!(g[0].delete[0].reason.contains("release tag"));
    }

    #[test]
    fn digital_beats_other_release_tags_then_size() {
        let g = dedupe(
            "All Star Superman (2006)",
            vec![
                book(
                    "emp",
                    "S1",
                    "All Star Superman 07 (2007) (Empire-Megan).cbr",
                    1.0,
                    900,
                ),
                book(
                    "dig",
                    "S1",
                    "All Star Superman 007 (2007) (digital) (Minutemen-PhD).cbr",
                    2.0,
                    100,
                ),
            ],
        );
        assert_eq!(g[0].keep.as_ref().unwrap().book_id, "dig");
        assert!(g[0].delete[0].reason.contains("(Digital)"));

        let g = dedupe(
            "Invincible (2003)",
            vec![
                book(
                    "small",
                    "S1",
                    "Invincible 073 (2010) (digital-Empire).cbz",
                    1.0,
                    100,
                ),
                book(
                    "big",
                    "S1",
                    "Invincible 073 (2010) (digital) (Minutemen-InnerDemons).cbr",
                    2.0,
                    200,
                ),
            ],
        );
        assert_eq!(g[0].keep.as_ref().unwrap().book_id, "big");
        assert!(g[0].delete[0].reason.contains("larger"));
    }

    #[test]
    fn digital_beats_mobile() {
        let g = dedupe(
            "Saga (2012)",
            vec![
                book(
                    "mob",
                    "S1",
                    "Saga 001 (2012) (digital-mobile) (Empire).cbz",
                    1.0,
                    900,
                ),
                book(
                    "dig",
                    "S1",
                    "Saga 001 (2012) (Digital) (Empire).cbz",
                    2.0,
                    100,
                ),
            ],
        );
        assert_eq!(g[0].keep.as_ref().unwrap().book_id, "dig");
        assert!(g[0].delete[0].reason.contains("mobile/webrip"));
    }

    #[test]
    fn archive_beats_pdf_for_the_same_volume() {
        let g = dedupe(
            "Green Lantern Earth One (2018)",
            vec![
                book(
                    "pdf1",
                    "S1",
                    "bb-green.lantern.earth.one.volume.1.pdf",
                    1.0,
                    900,
                ),
                book(
                    "pdf2",
                    "S1",
                    "bb-green.lantern.earth.one.volume.2.pdf",
                    2.0,
                    900,
                ),
                book(
                    "cbr1",
                    "S1",
                    "Green Lantern - Earth One  v01 (2018) (digital) (Son of Ultron-Empire).cbr",
                    3.0,
                    100,
                ),
                book(
                    "cbr2",
                    "S1",
                    "Green Lantern - Earth One v02 (2020) (digital) (Son of Ultron-Empire).cbr",
                    4.0,
                    100,
                ),
            ],
        );
        let pairs: Vec<(&str, &str, &str)> = g
            .iter()
            .map(|d| {
                (
                    d.number.as_str(),
                    d.keep.as_ref().unwrap().book_id.as_str(),
                    d.delete[0].book_id.as_str(),
                )
            })
            .collect();
        assert_eq!(pairs, vec![("v1", "cbr1", "pdf1"), ("v2", "cbr2", "pdf2")]);

        let g = dedupe(
            "A (2010)",
            vec![
                book("cbz", "S1", "A 001 (2010) (Digital).cbz", 1.0, 1),
                book("pdf", "S1", "A 001 (2010) (Digital).pdf", 2.0, 9),
            ],
        );
        assert_eq!(g[0].keep.as_ref().unwrap().book_id, "cbz");
        assert!(g[0].delete[0].reason.contains("cbz"));
    }

    #[test]
    fn deleted_or_unready_books_never_keep_or_lose() {
        let mut gone = book(
            "cbr1",
            "S1",
            "Green Lantern - Earth One  v01 (2018) (digital) (Son of Ultron-Empire).cbr",
            1.0,
            200,
        );
        gone.deleted = true;
        let mut broken = book(
            "err",
            "S1",
            "Saga 001 (2012) (Digital) (Zone-Empire).cbr",
            3.0,
            1,
        );
        broken.media = Some(crate::api::Media {
            status: "ERROR".into(),
        });
        let data = lib(
            &[("S1", "Green Lantern Earth One (2018)")],
            vec![
                gone,
                book(
                    "pdf1",
                    "S1",
                    "bb-green.lantern.earth.one.volume.1.pdf",
                    2.0,
                    100,
                ),
                broken,
                book("ok", "S1", "Saga 001 (2012).cbr", 4.0, 1),
            ],
        );
        assert!(plan(std::slice::from_ref(&data), true).is_empty());
        let (planned, _) = renumber_plan(std::slice::from_ref(&data), false);
        assert!(planned
            .iter()
            .all(|r| r.book_id != "cbr1" && r.book_id != "err"));
        let r = audit("k", &[data]);
        assert_eq!(r.summary.unavailable_books, 2);
        let un = &r.libraries[0].series[0].unavailable;
        assert_eq!(
            (un[0].deleted, un[1].media_status.as_str()),
            (true, "ERROR")
        );
    }

    #[test]
    fn misfiled_and_guessed_numbers_never_pair() {
        let g = dedupe(
            "Secret Wars (2015)",
            vec![
                book(
                    "a",
                    "S1",
                    "Secret Wars 001 (2015) (Digital) (Zone-Empire).cbr",
                    1.0,
                    1,
                ),
                book(
                    "b",
                    "S1",
                    "Batman 001 (2016) (Digital) (Zone-Empire).cbr",
                    2.0,
                    1,
                ),
                book(
                    "c",
                    "S1",
                    "Secret Wars Journal 001 (2015) (Digital) (Zone-Empire).cbr",
                    3.0,
                    1,
                ),
            ],
        );
        assert!(g.is_empty(), "{g:?}");

        let g = dedupe(
            "Marvel Super Heroes Secret Wars (1984)",
            vec![
                book("gen", "S1", "Marvel Super Heroes Secret Wars (1984) Volume 01 Issue 010.cbz", 1.0, 1),
                book("fax", "S1", "Marvel Super Heroes Secret Wars #10 Facsimile Edition 001 (1985) (c2c) (digital) (Marika-Empire).cbz", 2.0, 1),
            ],
        );
        assert!(g.is_empty(), "{g:?}");
    }

    #[test]
    fn series_name_numbers_are_not_issue_numbers() {
        let data = lib(
            &[("S1", "Spider-Man 2099 (2014)")],
            vec![
                book(
                    "a",
                    "S1",
                    "Spider-Man 2099 (2014) (Digital) (Zone-Empire).cbr",
                    1.0,
                    1,
                ),
                book(
                    "b",
                    "S1",
                    "Spider-Man 2099 (2014) (Digital) (Minutemen-X).cbr",
                    2.0,
                    1,
                ),
                book("c", "S1", "Spider-Man 2099 003 (2014).cbr", 2.0, 1),
            ],
        );
        assert!(plan(std::slice::from_ref(&data), false).is_empty());
        let (planned, skipped) = renumber_plan(std::slice::from_ref(&data), false);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].book_id, "c");
        assert!(skipped
            .iter()
            .any(|s| s.book_id == "a" && s.reason.contains("series name")));
    }

    #[test]
    fn tpb_annual_and_of_n_are_kept_apart() {
        let data = lib(
            &[("S1", "Invincible (2003)")],
            vec![
                book(
                    "i1",
                    "S1",
                    "Invincible 001 (2003) (Digital) (Zone-Empire).cbr",
                    1.0,
                    1,
                ),
                book(
                    "tpb",
                    "S1",
                    "Invincible TPB 1 (2003) (Digital) (Zone-Empire).cbr",
                    2.0,
                    1,
                ),
                book(
                    "ann",
                    "S1",
                    "Invincible Annual 01 (2011) (Digital) (Zone-Empire).cbr",
                    3.0,
                    1,
                ),
                book("i2", "S1", "Invincible 02 (of 04) (2003).cbr", 4.0, 1),
                book(
                    "i2b",
                    "S1",
                    "Invincible 002 (2003) (Digital) (Zone-Empire).cbr",
                    5.0,
                    1,
                ),
            ],
        );
        let g = plan(std::slice::from_ref(&data), false);
        assert_eq!(g.len(), 1, "{g:?}");
        assert_eq!(g[0].number, "2");
        assert_eq!(g[0].keep.as_ref().unwrap().book_id, "i2b");
        let r = audit("k", &[data]);
        let kinds: Vec<&str> = r.libraries[0].series[0]
            .edge_cases
            .iter()
            .map(|e| e.kind.as_str())
            .collect();
        assert_eq!(kinds, vec!["collected_edition", "annual"]);
    }

    #[test]
    fn mobile_release_loses_to_untagged() {
        let g = dedupe(
            "Saga (2012)",
            vec![
                book(
                    "mob",
                    "S1",
                    "Saga 001 (2012) (digital-mobile) (Empire-Megan).cbz",
                    1.0,
                    900,
                ),
                book("plain", "S1", "Saga 001 (2012).cbz", 2.0, 100),
            ],
        );
        assert_eq!(g[0].keep.as_ref().unwrap().book_id, "plain");
        assert!(g[0].delete[0].reason.contains("mobile"));
    }

    #[test]
    fn loose_tags_do_not_make_a_release() {
        let g = dedupe(
            "Saga (2012)",
            vec![
                book(
                    "var",
                    "S1",
                    "Saga 001 (2012) (Variant) (c2c) (F).cbz",
                    1.0,
                    900,
                ),
                book(
                    "rel",
                    "S1",
                    "Saga 001 (2012) (Digital) (Zone-Empire).cbz",
                    2.0,
                    100,
                ),
            ],
        );
        assert_eq!(g[0].keep.as_ref().unwrap().book_id, "rel");
        let g = dedupe(
            "Saga (2012)",
            vec![
                book(
                    "var",
                    "S1",
                    "Saga 002 (2012) (Variant) (hybrid).cbz",
                    1.0,
                    900,
                ),
                book("yr", "S1", "Saga 002 (2006 2025).cbz", 2.0, 100),
            ],
        );
        assert!(g[0].delete[0].reason.contains("larger"), "{g:?}");
    }

    #[test]
    fn number_string_mismatch_is_misnumbered() {
        let mut b = book("a", "S1", "Saga 018 (2012).cbz", 18.0, 1);
        b.metadata.number = "1".into();
        let (planned, _) = renumber_plan(&[lib(&[("S1", "Saga (2012)")], vec![b])], false);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].number, "18");

        let mut b = book("a", "S1", "Saga 018 (2012).cbz", 18.0, 1);
        b.metadata.number = "018".into();
        let (planned, _) = renumber_plan(&[lib(&[("S1", "Saga (2012)")], vec![b])], false);
        assert!(planned.is_empty());
    }

    #[test]
    fn volumes_never_duplicate_issues() {
        let g = dedupe(
            "Lazarus (2013)",
            vec![
                book("i7", "S1", "Lazarus 007 (2014) (Digital).cbz", 1.0, 1),
                book("i8", "S1", "Lazarus 008 (2014) (Digital).cbz", 2.0, 1),
                book(
                    "v7",
                    "S1",
                    "Lazarus v07 (2023) (Digital) (EJGriffin-Empire).cbz",
                    3.0,
                    1,
                ),
                book(
                    "v8",
                    "S1",
                    "Lazarus v08 (2026) (Digital) (Forever-Empire).cbz",
                    4.0,
                    1,
                ),
            ],
        );
        assert!(g.is_empty(), "{g:?}");
        let r = audit(
            "k",
            &[lib(
                &[("S1", "Lazarus (2013)")],
                vec![
                    book("i7", "S1", "Lazarus 007 (2014).cbz", 7.0, 1),
                    book("v7", "S1", "Lazarus v07 (2023).cbz", 2.0, 1),
                ],
            )],
        );
        assert_eq!(r.summary.collected_editions, 1);
        assert_eq!(r.summary.misnumbered_books, 0);
    }

    #[test]
    fn purge_generic_deletes_every_generic_file_and_lists_orphans() {
        let data = lib(
            &[("S1", "The Amazing Spider-Man (2025)")],
            vec![
                book(
                    "g8",
                    "S1",
                    "The Amazing Spider-Man (2025) Volume 01 Issue 008.cbz",
                    1.0,
                    900,
                ),
                book(
                    "r8",
                    "S1",
                    "Amazing Spider-Man 008 (2025) (Digital) (Shan-Empire).cbz",
                    2.0,
                    100,
                ),
                book(
                    "g15",
                    "S1",
                    "The Amazing Spider-Man (2025) Volume 01 Issue 015.cbz",
                    3.0,
                    900,
                ),
                book(
                    "r9",
                    "S1",
                    "Amazing Spider-Man 009 (2025) (Digital) (Shan-Empire).cbz",
                    4.0,
                    100,
                ),
                book("v1", "S1", "Amazing Spider-Man v01 (2025).cbz", 5.0, 100),
            ],
        );
        let g = plan(std::slice::from_ref(&data), true);
        let rows: Vec<(&str, Option<&str>, Vec<&str>)> = g
            .iter()
            .map(|d| {
                (
                    d.number.as_str(),
                    d.keep.as_ref().map(|k| k.book_id.as_str()),
                    d.delete.iter().map(|l| l.book_id.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![("8", Some("r8"), vec!["g8"]), ("15", None, vec!["g15"])]
        );
        assert_eq!(
            orphans(&g, true),
            vec![OrphanIssue {
                series_id: "S1".into(),
                series: "The Amazing Spider-Man".into(),
                year: Some(2025),
                issue_number: "15".into(),
            }]
        );
        let last: Vec<(&str, bool)> = g
            .iter()
            .flat_map(|d| &d.delete)
            .map(|l| (l.book_id.as_str(), l.last_copy))
            .collect();
        assert_eq!(last, vec![("g8", false), ("g15", true)]);
        assert!(g[1].delete[0].reason.contains("last copy"));
        assert!(orphans(&plan(&[data], false), true).is_empty());
    }

    #[test]
    fn an_unpaired_twin_is_only_a_possible_copy() {
        let data = lib(
            &[("S1", "The Amazing Spider-Man (2025)")],
            vec![
                book(
                    "g15",
                    "S1",
                    "The Amazing Spider-Man (2025) Volume 01 Issue 015.cbz",
                    1.0,
                    900,
                ),
                book(
                    "fax",
                    "S1",
                    "Amazing Spider-Man 015 Facsimile Edition (2025).cbz",
                    2.0,
                    100,
                ),
                book(
                    "abs",
                    "S1",
                    "Absolute Amazing Spider-Man 016 (2025).cbz",
                    3.0,
                    100,
                ),
                book(
                    "g16",
                    "S1",
                    "The Amazing Spider-Man (2025) Volume 01 Issue 016.cbz",
                    4.0,
                    900,
                ),
            ],
        );
        let g = plan(&[data], true);
        assert_eq!(g.len(), 2, "{g:?}");
        assert_eq!(g[0].number, "15");
        assert_eq!(g[0].kept_elsewhere.as_ref().unwrap().book_id, "fax");
        assert!(g[0].delete[0].last_copy);
        assert!(
            g[0].delete[0].reason.contains(
                "possible copy: Amazing Spider-Man 015 Facsimile Edition (2025).cbz (facsimile)"
            ),
            "{g:?}"
        );
        assert_eq!(g[1].number, "16");
        assert!(g[1].kept_elsewhere.is_none(), "another comic is no copy");
        assert!(g[1].delete[0].last_copy);
        let orphaned: Vec<String> = orphans(&g, true)
            .into_iter()
            .map(|o| o.issue_number)
            .collect();
        assert_eq!(orphaned, vec!["15", "16"]);
        assert!(orphans(&g, false).is_empty());
    }

    #[test]
    fn different_comics_sharing_a_word_never_pair() {
        let cases: [(&str, &[&str]); 4] = [
            (
                "Batman (2016)",
                &[
                    "Batman 001 (2016) (Digital) (Zone-Empire).cbr",
                    "Absolute Batman 001 (2024) (Digital) (Shan-Empire).cbz",
                ],
            ),
            (
                "Spider-Man (2016)",
                &[
                    "Spider-Man 001 (2016) (Digital) (Zone-Empire).cbr",
                    "Ultimate Spider-Man 001 (2016) (Digital) (Zone-Empire).cbr",
                    "Superior Spider-Man 001 (2016) (Digital) (Zone-Empire).cbr",
                ],
            ),
            (
                "X-Men (2016)",
                &[
                    "X-Men 001 (2016) (Digital) (Zone-Empire).cbr",
                    "Uncanny X-Men 001 (2016) (Digital) (Zone-Empire).cbr",
                ],
            ),
            (
                "Wonder Woman (2016)",
                &[
                    "Wonder Woman 001 (2016) (Digital) (Zone-Empire).cbr",
                    "Absolute Wonder Woman (2024) Volume 01 Issue 001.cbz",
                ],
            ),
        ];
        for (series, files) in cases {
            let books = files
                .iter()
                .enumerate()
                .map(|(i, f)| book(&format!("b{i}"), "S1", f, 1.0, 1))
                .collect();
            let data = lib(&[("S1", series)], books);
            assert!(
                plan(std::slice::from_ref(&data), false).is_empty(),
                "{series}"
            );
            assert!(plan(&[data], true).is_empty(), "{series} purge");
        }
    }

    #[test]
    fn cover_years_over_a_year_apart_never_pair() {
        let g = dedupe(
            "Wonder Woman (2016)",
            vec![
                book(
                    "a",
                    "S1",
                    "Wonder Woman 001 (2016) (Digital) (Zone-Empire).cbr",
                    1.0,
                    1,
                ),
                book(
                    "b",
                    "S1",
                    "Wonder Woman 001 (2023) (Digital) (Shan-Empire).cbz",
                    2.0,
                    1,
                ),
            ],
        );
        assert!(g.is_empty(), "{g:?}");
        let g = dedupe(
            "The Amazing Spider-Man (2022)",
            vec![
                book(
                    "a",
                    "S1",
                    "Amazing Spider-Man 008 (2022) (Digital) (Zone-Empire).cbr",
                    1.0,
                    1,
                ),
                book(
                    "b",
                    "S1",
                    "Amazing Spider-Man 008 (2025) (Digital) (Shan-Empire).cbz",
                    2.0,
                    1,
                ),
            ],
        );
        assert!(g.is_empty(), "{g:?}");
        let g = dedupe(
            "Saga (2012)",
            vec![
                book(
                    "a",
                    "S1",
                    "Saga 012 (2013) (Digital) (Zone-Empire).cbr",
                    1.0,
                    1,
                ),
                book(
                    "b",
                    "S1",
                    "Saga 012 (2014) (Digital) (Shan-Empire).cbz",
                    2.0,
                    2,
                ),
            ],
        );
        assert_eq!(g.len(), 1);
        let g = dedupe(
            "The Amazing Spider-Man (2022)",
            vec![
                book(
                    "a",
                    "S1",
                    "Amazing Spider-Man 034 (2023) (Digital) (Shan-Empire).cbz",
                    1.0,
                    1,
                ),
                book(
                    "b",
                    "S1",
                    "The Amazing Spider-Man (2022) Volume 06 Issue 034.cbr",
                    2.0,
                    1,
                ),
            ],
        );
        assert_eq!(g[0].keep.as_ref().unwrap().book_id, "a");
    }
}
