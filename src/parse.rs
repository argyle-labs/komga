//! Comic filename parsing: issue/volume numbers, release tags, and series-name
//! similarity. Pure functions; no I/O.
//!
//! Komga does not read issue numbers from filenames — it numbers books by sort
//! order — so the number a file *means* has to be recovered from its name.

/// What a filename's number denotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Issue,
    /// A collected edition (`v07`, `Vol 2`, `Book 1`, `TPB`, `Omnibus`, …),
    /// numbered apart from issues.
    Volume,
}

/// A number that is real but must not be written back as a plain issue
/// number, nor used to pair the file with another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// `605.1`: a point issue.
    Decimal,
    /// `065.Deaths`, `#1.MU`: a number glued to a suffix.
    Suffixed,
    /// `#10 … 001`: a `#N` with another number after it.
    Ambiguous,
    /// A facsimile reprint of an older issue.
    Facsimile,
    /// An annual filed in a series that is not itself the annual.
    Annual,
    /// The file's only number is also in the series name (`Spider-Man 2099`).
    SeriesNumber,
}

impl Edge {
    pub fn as_str(self) -> &'static str {
        match self {
            Edge::Decimal => "decimal",
            Edge::Suffixed => "suffixed",
            Edge::Ambiguous => "ambiguous",
            Edge::Facsimile => "facsimile",
            Edge::Annual => "annual",
            Edge::SeriesNumber => "series_number",
        }
    }

    /// Decimal and suffixed numbers still identify one issue, so two files
    /// carrying the same one are duplicates; the rest are guesses.
    pub fn pairs(self) -> bool {
        matches!(self, Edge::Decimal | Edge::Suffixed)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub kind: Kind,
    pub number: f64,
    /// The number as written back to Komga, or the suffixed label (`65.Deaths`).
    pub label: String,
    pub edge: Option<Edge>,
    /// The name text before the number, for series-mismatch checks.
    pub prefix: String,
    /// Taken as the last standalone number, and it was the only one.
    pub sole_number: bool,
    /// Char offset of the number, to pick the rightmost of two readings.
    pos: usize,
}

/// A filename with its bracketed tags split off.
#[derive(Debug, Clone, PartialEq)]
pub struct FileName {
    pub parsed: Option<Parsed>,
    /// Contents of every `(...)`, `[...]`, `{...}` group, in order.
    pub tags: Vec<String>,
    /// Lowercased extension without the dot.
    pub ext: String,
    /// The name says `Annual`.
    pub annual: bool,
}

/// Words marking a collected edition rather than a single issue.
const COLLECTED_WORDS: [&str; 5] = ["tpb", "hc", "omnibus", "deluxe", "compendium"];

pub fn parse(file: &str) -> FileName {
    let (stem, ext) = split_ext(file);
    let (body, tags) = strip_groups(&stem.replace('_', " "));
    let chars: Vec<char> = body.chars().collect();
    let words: Vec<String> = body
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    let tag_words: Vec<String> = tags
        .iter()
        .flat_map(|t| {
            t.to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    let has = |w: &str| words.iter().chain(&tag_words).any(|x| x == w);

    let mut parsed = explicit_issue(&chars)
        .or_else(|| match (trailing_number(&chars), glued_volume(&chars)) {
            (Some(t), Some(v)) => Some(if v.pos > t.pos { v } else { t }),
            (t, v) => t.or(v),
        })
        .or_else(|| suffixed(&chars));
    if let Some(p) = parsed.as_mut() {
        if COLLECTED_WORDS.iter().any(|w| has(w)) {
            p.kind = Kind::Volume;
        }
        if has("facsimile") && p.edge.is_none() {
            p.edge = Some(Edge::Facsimile);
        }
    }
    FileName {
        parsed,
        tags,
        ext: ext.to_ascii_lowercase(),
        annual: words.iter().any(|w| w == "annual"),
    }
}

/// `os.path.splitext`: the extension starts at the last dot, unless the name
/// before it is empty or all dots.
fn split_ext(file: &str) -> (&str, &str) {
    match file.rfind('.') {
        Some(i) if !file[..i].trim_start_matches('.').is_empty() => (&file[..i], &file[i + 1..]),
        _ => (file, ""),
    }
}

fn strip_groups(s: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(s.len());
    let mut tags = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        let close = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            _ => None,
        };
        if let Some(close) = close {
            if let Some(end) = rest[1..].find(close) {
                tags.push(rest[1..1 + end].trim().to_string());
                out.push(' ');
                rest = &rest[1 + end + 1..];
                continue;
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    (out, tags)
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Digits with an optional `.digits` fraction starting at `i`: `(end, has_fraction)`.
fn read_number(chars: &[char], i: usize) -> Option<(usize, bool)> {
    let mut j = i;
    while j < chars.len() && chars[j].is_ascii_digit() {
        j += 1;
    }
    if j == i {
        return None;
    }
    if j + 1 < chars.len() && chars[j] == '.' && chars[j + 1].is_ascii_digit() {
        let mut k = j + 1;
        while k < chars.len() && chars[k].is_ascii_digit() {
            k += 1;
        }
        return Some((k, true));
    }
    Some((j, false))
}

/// `.Word` right after a number: the end of the word.
fn suffix_word(chars: &[char], end: usize) -> Option<usize> {
    if end + 1 < chars.len() && chars[end] == '.' && chars[end + 1].is_alphabetic() {
        let mut k = end + 1;
        while k < chars.len() && chars[k].is_alphabetic() {
            k += 1;
        }
        return Some(k);
    }
    None
}

fn text(chars: &[char]) -> String {
    chars.iter().collect::<String>().trim().to_string()
}

fn number_at(
    chars: &[char],
    start: usize,
    end: usize,
    fraction: bool,
    kind: Kind,
    prefix_end: usize,
) -> Option<Parsed> {
    let number: f64 = text(&chars[start..end]).parse().ok()?;
    Some(Parsed {
        kind,
        number,
        label: canonical(number),
        edge: fraction.then_some(Edge::Decimal),
        prefix: text(&chars[..prefix_end]),
        sole_number: false,
        pos: start,
    })
}

/// `Issue 008` / `#8`. `Issue N` always wins; `#N` is flagged when another
/// number follows it, since either could be the issue.
fn explicit_issue(chars: &[char]) -> Option<Parsed> {
    for i in 0..chars.len() {
        let hash = chars[i] == '#';
        let after = if hash {
            i + 1
        } else if starts_with_ci(chars, i, "issue") && (i == 0 || !word_char(chars[i - 1])) {
            i + 5
        } else {
            continue;
        };
        let mut j = after;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        let Some((end, fraction)) = read_number(chars, j) else {
            continue;
        };
        let mut p = number_at(chars, j, end, fraction, Kind::Issue, i)?;
        if let (false, Some(k)) = (fraction, suffix_word(chars, end)) {
            let word: String = chars[end + 1..k].iter().collect();
            p.label = format!("{}.{word}", p.label);
            p.edge = Some(Edge::Suffixed);
        } else if hash && standalone(chars).iter().any(|&(s, _, _)| s > end) {
            p.edge = Some(Edge::Ambiguous);
        }
        return Some(p);
    }
    None
}

/// Every number not touching a word character or a dot on either side:
/// `(start, end, has_fraction)`. A leading `-` belongs to the number.
fn standalone(chars: &[char]) -> Vec<(usize, usize, bool)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let digit_at = if chars[i] == '-' { i + 1 } else { i };
        let free_before = i == 0 || !(word_char(chars[i - 1]) || chars[i - 1] == '.');
        if free_before {
            if let Some((end, fraction)) = read_number(chars, digit_at) {
                let free_after =
                    end == chars.len() || !(word_char(chars[end]) || chars[end] == '.');
                if free_after {
                    out.push((i, end, fraction));
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Standalone numbers in a name, ignoring bracketed groups such as `(2015)`.
pub fn name_numbers(name: &str) -> Vec<f64> {
    let (body, _) = strip_groups(name);
    let chars: Vec<char> = body.chars().collect();
    standalone(&chars)
        .into_iter()
        .filter_map(|(s, e, _)| text(&chars[s..e]).parse().ok())
        .collect()
}

/// The last standalone number, or the one before `of N`. Preceded by a
/// volume word, it is a volume.
fn trailing_number(chars: &[char]) -> Option<Parsed> {
    let nums = standalone(chars);
    let mut pick = nums.len().checked_sub(1)?;
    if pick > 0 && word_before(chars, nums[pick].0).as_deref() == Some("of") {
        pick -= 1;
    }
    let (start, end, fraction) = nums[pick];
    let mut p = match volume_keyword_before(chars, start) {
        Some(kw) => number_at(chars, start, end, fraction, Kind::Volume, kw),
        None => number_at(chars, start, end, fraction, Kind::Issue, start),
    }?;
    p.sole_number = nums.len() == 1;
    Some(p)
}

/// The lowercased word ending right before `at`, skipping spaces and dots.
fn word_before(chars: &[char], at: usize) -> Option<String> {
    let mut end = at;
    while end > 0 && (chars[end - 1].is_whitespace() || chars[end - 1] == '.') {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && chars[start - 1].is_alphabetic() {
        start -= 1;
    }
    (start < end).then(|| chars[start..end].iter().collect::<String>().to_lowercase())
}

const VOLUME_WORDS: [&str; 4] = ["volume", "book", "vol", "v"];

/// Start of a volume word ending right before `at` (spaces and dots between
/// allowed).
fn volume_keyword_before(chars: &[char], at: usize) -> Option<usize> {
    let word = word_before(chars, at)?;
    if !VOLUME_WORDS.contains(&word.as_str()) {
        return None;
    }
    let mut end = at;
    while end > 0 && (chars[end - 1].is_whitespace() || chars[end - 1] == '.') {
        end -= 1;
    }
    Some(end - word.chars().count())
}

/// The last `v07` / `vol.2` / `volume.1` / `book 3`.
fn glued_volume(chars: &[char]) -> Option<Parsed> {
    let mut found = None;
    for i in 0..chars.len() {
        if i > 0 && chars[i - 1].is_alphanumeric() {
            continue;
        }
        for kw in VOLUME_WORDS {
            if !starts_with_ci(chars, i, kw) {
                continue;
            }
            let mut j = i + kw.len();
            while j < chars.len() && (chars[j] == '.' || chars[j] == ' ') {
                j += 1;
            }
            if let Some((end, fraction)) = read_number(chars, j) {
                if end == chars.len() || !chars[end].is_alphanumeric() {
                    found = number_at(chars, j, end, fraction, Kind::Volume, i);
                }
            }
            break;
        }
    }
    found
}

/// `065.Deaths`: digits joined by a dot to a word.
fn suffixed(chars: &[char]) -> Option<Parsed> {
    let mut found = None;
    for i in 0..chars.len() {
        if i > 0 && (word_char(chars[i - 1]) || chars[i - 1] == '.') {
            continue;
        }
        let Some((end, false)) = read_number(chars, i) else {
            continue;
        };
        if let Some(k) = suffix_word(chars, end) {
            let mut p = number_at(chars, i, end, false, Kind::Issue, i)?;
            let word: String = chars[end + 1..k].iter().collect();
            p.label = format!("{}.{word}", p.label);
            p.edge = Some(Edge::Suffixed);
            found = Some(p);
        }
    }
    found
}

fn starts_with_ci(chars: &[char], at: usize, word: &str) -> bool {
    let n = word.chars().count();
    at + n <= chars.len()
        && chars[at..at + n]
            .iter()
            .zip(word.chars())
            .all(|(a, b)| a.to_ascii_lowercase() == b)
}

/// `8.0` → `"8"`, `605.1` → `"605.1"`.
pub fn canonical(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Words that mark a variant, printing, or rip rather than the release group.
const NOT_GROUP_WORDS: [&str; 7] = [
    "variant", "cover", "print", "sized", "c2c", "mobile", "webrip",
];

/// `(Digital)` or a scanner/group tag shaped `<who>-<Group>` (`Shan-Empire`,
/// `Son of Ultron-Empire`, `Digital-Empire`). Years, `(F)`, `(c2c)`,
/// `(hybrid)`, `(Variant)`, `(2 covers)`, `(of 04)` are not.
pub fn is_release_tag(tag: &str) -> bool {
    let t = tag.trim();
    let lower = t.to_lowercase();
    if lower == "digital" {
        return true;
    }
    if NOT_GROUP_WORDS.iter().any(|w| lower.contains(w)) {
        return false;
    }
    match t.rsplit_once('-') {
        Some((who, group)) => {
            !who.trim().is_empty()
                && !group.is_empty()
                && group.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

/// A mobile or webrip source.
pub fn is_mobile(tags: &[String]) -> bool {
    tags.iter().any(|t| {
        let l = t.to_lowercase();
        l.contains("mobile") || l.contains("webrip") || l.contains("web-rip")
    })
}

/// Source quality from tags: 2 = `(Digital)`, 1 = untagged, 0 = mobile/webrip.
pub fn digital_tier(tags: &[String]) -> u8 {
    if is_mobile(tags) {
        0
    } else if tags.iter().any(|t| t.to_lowercase().contains("digital")) {
        2
    } else {
        1
    }
}

/// `<Series> (YYYY) Volume NN Issue NNN.ext` with no release tag: a
/// tagger's rename, not a release Mylar tracks.
pub fn is_generic(file: &str) -> bool {
    let (stem, _) = split_ext(file);
    let (body, tags) = strip_groups(stem);
    if tags.is_empty() || tags.iter().any(|t| is_release_tag(t)) {
        return false;
    }
    let words: Vec<&str> = body.split_whitespace().collect();
    let digits = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit());
    matches!(
        words.as_slice(),
        [_, .., v, n, i, m]
            if v.eq_ignore_ascii_case("volume") && digits(n)
                && i.eq_ignore_ascii_case("issue") && digits(m)
    )
}

/// `"Saga (2012)"` → `("Saga", Some(2012))`.
pub fn split_year(name: &str) -> (String, Option<i32>) {
    let t = name.trim_end();
    if let Some(open) = t.rfind('(') {
        let inner = &t[open + 1..];
        if let Some(y) = inner.strip_suffix(')') {
            if y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()) {
                return (t[..open].trim_end().to_string(), y.parse().ok());
            }
        }
    }
    (t.to_string(), None)
}

/// Lowercase alphanumeric words with `(YYYY)` and `vol N` removed.
pub fn normalize(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    let chars: Vec<char> = name.to_lowercase().chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '('
            && i + 5 < chars.len()
            && chars[i + 1..i + 5].iter().all(char::is_ascii_digit)
            && chars[i + 5] == ')'
        {
            s.push(' ');
            i += 6;
            continue;
        }
        s.push(if chars[i].is_alphanumeric() {
            chars[i]
        } else {
            ' '
        });
        i += 1;
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    let mut k = 0;
    while k < words.len() {
        let w = words[k];
        let glued = ["volume", "vol"].iter().any(|kw| {
            w.strip_prefix(kw)
                .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
        });
        if glued {
            k += 1;
            continue;
        }
        if (w == "vol" || w == "volume")
            && words
                .get(k + 1)
                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
        {
            k += 2;
            continue;
        }
        out.push(w);
        k += 1;
    }
    out.join(" ")
}

/// [`normalize`] without a leading `the`, for comparing two filenames'
/// series parts.
pub fn prefix_key(prefix: &str) -> String {
    let n = normalize(prefix);
    match n.strip_prefix("the ") {
        Some(rest) => rest.to_string(),
        None => n,
    }
}

/// A filename's series part, for telling whether two files are the same comic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComicKey {
    key: String,
    /// The key without a scene-style leading token (`bb-` in
    /// `bb-green.lantern.earth.one.`), when the series part is one glued run.
    scene: Option<String>,
}

pub fn comic_key(prefix: &str) -> ComicKey {
    let raw = prefix.trim().trim_end_matches(['.', '-']);
    let glued = !raw.is_empty()
        && !raw.contains(char::is_whitespace)
        && raw.matches(['.', '-']).count() >= 2;
    let scene = glued
        .then(|| raw.split_once(['.', '-']))
        .flatten()
        .filter(|(head, _)| !head.is_empty())
        .map(|(_, rest)| prefix_key(rest))
        .filter(|k| !k.is_empty());
    ComicKey {
        key: prefix_key(prefix),
        scene,
    }
}

/// Same comic: equal non-empty keys, or equal once one side drops its
/// scene-style leading token. A spaced leading word (`Absolute`, `Uncanny`)
/// names a different comic.
pub fn same_comic(a: &ComicKey, b: &ComicKey) -> bool {
    if a.key.is_empty() || b.key.is_empty() {
        return false;
    }
    a.key == b.key || a.scene.as_ref() == Some(&b.key) || b.scene.as_ref() == Some(&a.key)
}

/// The first bracketed bare year, e.g. `2016` from `(2016)`.
pub fn tag_year(tags: &[String]) -> Option<i32> {
    tags.iter()
        .map(|t| t.trim())
        .find(|t| t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()))
        .and_then(|t| t.parse().ok())
}

/// `difflib.SequenceMatcher(None, a, b).ratio()` without autojunk (names are
/// far below its 200-char threshold).
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let matched = matching_chars(&a, &b);
    2.0 * matched as f64 / (a.len() + b.len()) as f64
}

fn matching_chars(a: &[char], b: &[char]) -> usize {
    let mut queue = vec![(0, a.len(), 0, b.len())];
    let mut total = 0;
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest_match(a, b, alo, ahi, blo, bhi);
        if k == 0 {
            continue;
        }
        total += k;
        if alo < i && blo < j {
            queue.push((alo, i, blo, j));
        }
        if i + k < ahi && j + k < bhi {
            queue.push((i + k, ahi, j + k, bhi));
        }
    }
    total
}

/// Longest common run in `a[alo..ahi]`/`b[blo..bhi]`; ties go to the
/// earliest `i`, then the earliest `j`, as difflib does.
fn longest_match(
    a: &[char],
    b: &[char],
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let (mut bi, mut bj, mut bk) = (alo, blo, 0);
    let mut prev = vec![0usize; bhi - blo + 1];
    for (i, ca) in a.iter().enumerate().take(ahi).skip(alo) {
        let mut cur = vec![0usize; bhi - blo + 1];
        for (j, cb) in b.iter().enumerate().take(bhi).skip(blo) {
            if ca == cb {
                let k = prev[j - blo] + 1;
                cur[j - blo + 1] = k;
                if k > bk {
                    (bi, bj, bk) = (i + 1 - k, j + 1 - k, k);
                }
            }
        }
        prev = cur;
    }
    (bi, bj, bk)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(file: &str) -> Parsed {
        parse(file)
            .parsed
            .unwrap_or_else(|| panic!("{file}: no number"))
    }

    #[test]
    fn trailing_number_after_stripping_tags() {
        let x = p("Saga 03 (2012) (Digital) (Empire).cbz");
        assert_eq!(
            (x.kind, x.number, x.label.as_str()),
            (Kind::Issue, 3.0, "3")
        );
        assert_eq!(x.edge, None);
        assert_eq!(x.prefix, "Saga");
    }

    #[test]
    fn underscores_are_spaces() {
        let x = p("Saga_026_(2015).cbr");
        assert_eq!((x.number, x.prefix.as_str()), (26.0, "Saga"));
    }

    #[test]
    fn explicit_issue_beats_volume_and_year() {
        let x = p("The Amazing Spider-Man (2022) Volume 06 Issue 008.cbr");
        assert_eq!((x.kind, x.number), (Kind::Issue, 8.0));
        assert!(x.prefix.ends_with("Volume 06"), "{}", x.prefix);
        assert_eq!(p("Batman #12 (2016).cbz").number, 12.0);
    }

    #[test]
    fn padding_does_not_change_the_number() {
        assert_eq!(p("All Star Superman 07 (2007).cbr").label, "7");
        assert_eq!(p("All Star Superman 007 (2007).cbr").label, "7");
    }

    #[test]
    fn number_glued_to_another_number_is_not_the_issue() {
        let x = p("Amazing Spider-Man 1000-Queen in Black CGD 2026 001 (2026).cbz");
        assert_eq!(x.number, 1.0);
    }

    #[test]
    fn decimal_issue_is_an_edge() {
        let x = p("Amazing Spider-Man 605.1 (2009).cbz");
        assert_eq!((x.number, x.label.as_str()), (605.1, "605.1"));
        assert_eq!(x.edge, Some(Edge::Decimal));
    }

    #[test]
    fn suffixed_number_is_an_edge() {
        let x = p("Amazing Spider-Man 065.Deaths (2025) (Digital) (Shan-Empire).cbz");
        assert_eq!((x.kind, x.number), (Kind::Issue, 65.0));
        assert_eq!(x.label, "65.Deaths");
        assert_eq!(x.edge, Some(Edge::Suffixed));
    }

    #[test]
    fn volumes_are_their_own_kind() {
        let x = p("Lazarus v07 (2023) (Digital) (EJGriffin-Empire).cbz");
        assert_eq!(
            (x.kind, x.number, x.prefix.as_str()),
            (Kind::Volume, 7.0, "Lazarus")
        );
        let x = p("bb-green.lantern.earth.one.volume.1.pdf");
        assert_eq!((x.kind, x.number), (Kind::Volume, 1.0));
        let x = p("Green Lantern - Earth One  v01 (2018) (digital) (Son of Ultron-Empire).cbr");
        assert_eq!((x.kind, x.number), (Kind::Volume, 1.0));
        let x = p("Saga Vol. 2 (2013).cbz");
        assert_eq!((x.kind, x.number), (Kind::Volume, 2.0));
        let x = p("Blackest Night 10th Anniversary Omnibus v01 (2019) (hybrid).cbz");
        assert_eq!((x.kind, x.number), (Kind::Volume, 1.0));
    }

    #[test]
    fn issue_after_a_volume_word_is_still_the_issue() {
        let x = p("Batman Vol 3 012 (2017).cbz");
        assert_eq!((x.kind, x.number), (Kind::Issue, 12.0));
    }

    #[test]
    fn single_volume_without_a_number_is_unparsed() {
        assert!(parse("Brit (2003).cbr").parsed.is_none());
        assert!(parse("Brit - Red White Black And Blue.cbr")
            .parsed
            .is_none());
        assert!(parse("Spider-Verse (2015) (Digital) (F) (Zone-Empire).cbr")
            .parsed
            .is_none());
    }

    #[test]
    fn tags_and_extension_are_kept() {
        let f = parse("Amazing Spider-Man 008 (2025) (Digital) (Shan-Empire).CBZ");
        assert_eq!(f.tags, vec!["2025", "Digital", "Shan-Empire"]);
        assert_eq!(f.ext, "cbz");
    }

    #[test]
    fn release_tags_exclude_bare_years() {
        assert!(!is_release_tag("2025"));
        assert!(is_release_tag("Digital"));
        assert!(is_release_tag("Shan-Empire"));
    }

    #[test]
    fn digital_tier_ranks_sources() {
        let t = |v: &[&str]| digital_tier(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(t(&["2010", "digital-Empire"]), 2);
        assert_eq!(t(&["2007", "Empire-Megan"]), 1);
        assert_eq!(t(&["2020", "digital-mobile", "Empire"]), 0);
        assert_eq!(t(&["webrip"]), 0);
    }

    #[test]
    fn normalize_drops_years_and_volume_words() {
        assert_eq!(
            normalize("The Amazing Spider-Man (2022)"),
            "the amazing spider man"
        );
        assert_eq!(normalize("Saga Vol 2"), "saga");
        assert_eq!(normalize("Saga vol07"), "saga");
    }

    #[test]
    fn similarity_matches_difflib() {
        assert_eq!(similarity("abcd", "bcde"), 0.75);
        assert_eq!(similarity("", ""), 1.0);
        assert_eq!(similarity("abc", "xyz"), 0.0);
        // difflib.SequenceMatcher(None, "amazing spider man", "the amazing spider man").ratio()
        assert!((similarity("amazing spider man", "the amazing spider man") - 0.9).abs() < 1e-9);
    }

    #[test]
    fn generic_names_are_volume_issue_renames_without_tags() {
        assert!(is_generic(
            "The Amazing Spider-Man (2025) Volume 01 Issue 008.cbz"
        ));
        assert!(is_generic(
            "Absolute Wonder Woman (2024) Volume 01 Issue 001.cbz"
        ));
        assert!(!is_generic(
            "Amazing Spider-Man 008 (2025) (Digital) (Shan-Empire).cbz"
        ));
        assert!(!is_generic("Invincible 075 (2010).cbr"));
        assert!(!is_generic("Volume 01 Issue 008 (2025).cbz"));
        assert!(!is_generic("X (2025) (Digital) Volume 01 Issue 008.cbz"));
    }

    #[test]
    fn split_year_peels_a_trailing_year() {
        assert_eq!(split_year("Saga (2012)"), ("Saga".to_string(), Some(2012)));
        assert_eq!(split_year("Brit"), ("Brit".to_string(), None));
    }

    #[test]
    fn hash_with_a_later_number_is_ambiguous_and_facsimile_is_an_edge() {
        let x = p("Marvel Super Heroes Secret Wars #10 Facsimile Edition 001 (1985) (c2c) (digital) (Marika-Empire).cbz");
        assert_eq!(x.number, 10.0);
        assert_eq!(x.edge, Some(Edge::Ambiguous));
        let x = p("Marvel Super Heroes Secret Wars #10 Facsimile Edition.cbz");
        assert_eq!(x.edge, Some(Edge::Facsimile));
        let x = p("Civil War 001 Facsimile Edition (2006 2025) (digital) (Marika-Empire).cbz");
        assert_eq!((x.number, x.edge), (1.0, Some(Edge::Facsimile)));
        let x = p("Batman #12 (2016).cbz");
        assert_eq!(x.edge, None);
    }

    #[test]
    fn hash_with_a_suffix_is_suffixed() {
        let x = p("Spider-Gwen #1.MU (2015).cbz");
        assert_eq!((x.number, x.label.as_str()), (1.0, "1.MU"));
        assert_eq!(x.edge, Some(Edge::Suffixed));
    }

    #[test]
    fn of_n_takes_the_number_before_it() {
        assert_eq!(
            p("Deadpools Secret Secret Wars 03 (of 04) (2015).cbr").number,
            3.0
        );
        assert_eq!(p("Master of Kung Fu 02 of 04 (2015).cbr").number, 2.0);
        assert_eq!(
            p("Master of Kung Fu 02 of 04 (2015).cbr").prefix,
            "Master of Kung Fu"
        );
    }

    #[test]
    fn collected_edition_words_make_a_volume() {
        for f in [
            "Saga TPB 2 (2013).cbz",
            "Saga HC 1 (2014).cbz",
            "Invincible Compendium 1 (2011).cbz",
            "Saga Deluxe Edition 1 (2014).cbz",
            "Invincible Omnibus 02 (2012).cbz",
            "Saga Book 3 (2019).cbz",
            "Lazarus Vol 04 TPB (2016) (Digital) (DR & Quinch-Empire).cbr",
            "Saga 1 (2014) (TPB).cbz",
        ] {
            assert_eq!(p(f).kind, Kind::Volume, "{f}");
        }
    }

    #[test]
    fn annual_is_noted() {
        assert!(parse("Invincible Annual 01 (2011).cbz").annual);
        assert!(!parse("Invincible 001 (2003).cbz").annual);
    }

    #[test]
    fn a_later_glued_volume_beats_a_series_name_number() {
        let x = p("Spider-Man 2099 v01 - Out of Time (2014).cbz");
        assert_eq!((x.kind, x.number), (Kind::Volume, 1.0));
        let x = p("Batman v2 012 (2012).cbz");
        assert_eq!((x.kind, x.number), (Kind::Issue, 12.0));
    }

    #[test]
    fn sole_number_is_flagged_for_series_name_checks() {
        assert!(p("Spider-Man 2099 (2014).cbz").sole_number);
        assert!(!p("Secret Wars 2099 001 (2015).cbz").sole_number);
        assert_eq!(p("1602 - Witch Hunter Angela 004 (2015).cbr").number, 4.0);
        assert_eq!(name_numbers("Spider-Man 2099 (2014)"), vec![2099.0]);
        assert_eq!(
            name_numbers("1602 Witch Hunter Angela (2015)"),
            vec![1602.0]
        );
    }

    #[test]
    fn loose_tags_are_not_release_tags() {
        for t in [
            "F",
            "c2c",
            "C2C REMIX",
            "Variant",
            "2nd Printing Variant Cover",
            "hybrid",
            "2006 2025",
            "2 covers",
            "of 04",
            "Image",
            "Zhang Chin Ant-Sized variant",
            "digital-mobile",
            "Digital-Infinite Comic",
        ] {
            assert!(!is_release_tag(t), "{t}");
        }
        for t in [
            "Digital",
            "digital",
            "Shan-Empire",
            "Son of Ultron-Empire",
            "Digital-Empire",
            "Archangel+Zone-Empire",
            "DR & Quinch-Empire",
            "Pyrate-DCP",
        ] {
            assert!(is_release_tag(t), "{t}");
        }
    }

    #[test]
    fn same_comic_allows_the_and_a_scene_token_only() {
        let k = comic_key;
        assert!(same_comic(
            &k("The Amazing Spider-Man   Volume 06"),
            &k("Amazing Spider-Man")
        ));
        assert!(same_comic(&k("All-Star Superman"), &k("All Star Superman")));
        assert!(same_comic(
            &k("bb-green.lantern.earth.one."),
            &k("Green Lantern - Earth One")
        ));
        for (a, b) in [
            ("Secret Wars Journal", "Secret Wars"),
            ("Absolute Batman", "Batman"),
            ("Ultimate Spider-Man", "Spider-Man"),
            ("Superior Spider-Man", "Spider-Man"),
            ("Uncanny X-Men", "X-Men"),
            ("Absolute Wonder Woman   Volume 01", "Wonder Woman"),
            ("Invincible", ""),
            ("", ""),
        ] {
            assert!(!same_comic(&k(a), &k(b)), "{a} vs {b}");
        }
    }

    #[test]
    fn tag_year_takes_the_first_bare_year() {
        let t = |v: &[&str]| tag_year(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(t(&["2016", "Digital"]), Some(2016));
        assert_eq!(t(&["2006 2025", "digital"]), None);
        assert_eq!(t(&["Digital"]), None);
    }
}
