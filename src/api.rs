//! Komga REST client (`/api/v1`) over the generated [`komga_api`] client,
//! authenticated with an `X-API-Key` header. Requests ride orca's
//! `http.request` capability (`delegated-http`).

use std::collections::HashSet;

use plugin_toolkit::prelude::*;

use crate::komga_api::{types, ByteStream, Client, Error, ResponseValue};

pub struct Komga {
    api: Client,
}

#[derive(Debug, Clone)]
pub struct Library {
    pub id: String,
    pub name: String,
    pub empty_trash_after_scan: bool,
}

#[derive(Debug, Clone)]
pub struct Series {
    pub id: String,
    pub library_id: String,
    /// Folder name; the title can differ when ComicInfo sets one.
    pub name: String,
    pub metadata: SeriesMetadata,
}

#[derive(Debug, Clone, Default)]
pub struct SeriesMetadata {
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct BookMetadata {
    pub number: String,
    pub number_sort: f64,
    pub number_lock: bool,
    pub number_sort_lock: bool,
}

#[derive(Debug, Clone)]
pub struct Book {
    pub id: String,
    pub series_id: String,
    /// Absolute file path inside Komga's container.
    pub url: String,
    pub size_bytes: i64,
    pub metadata: BookMetadata,
    /// Komga keeps a book whose file vanished, flagged, until the trash is
    /// emptied.
    pub deleted: bool,
    pub media: Option<Media>,
}

#[derive(Debug, Clone)]
pub struct Media {
    pub status: String,
}

impl Book {
    pub fn media_status(&self) -> &str {
        self.media.as_ref().map_or("UNKNOWN", |m| m.status.as_str())
    }

    /// On disk and read by Komga.
    pub fn available(&self) -> bool {
        !self.deleted && self.media_status() == "READY"
    }

    pub fn file_name(&self) -> &str {
        self.url.rsplit('/').next().unwrap_or(&self.url)
    }
}

#[derive(Debug, Clone)]
pub struct ReadList {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub ordered: bool,
    pub book_ids: Vec<String>,
}

impl From<types::LibraryDto> for Library {
    fn from(l: types::LibraryDto) -> Self {
        Self {
            id: l.id,
            name: l.name,
            empty_trash_after_scan: l.empty_trash_after_scan,
        }
    }
}

impl From<types::SeriesDto> for Series {
    fn from(s: types::SeriesDto) -> Self {
        Self {
            id: s.id,
            library_id: s.library_id,
            name: s.name,
            metadata: SeriesMetadata {
                title: s.metadata.title,
            },
        }
    }
}

impl From<types::BookDto> for Book {
    fn from(b: types::BookDto) -> Self {
        Self {
            id: b.id,
            series_id: b.series_id,
            url: b.url,
            size_bytes: b.size_bytes,
            metadata: BookMetadata {
                number: b.metadata.number,
                number_sort: widen(b.metadata.number_sort),
                number_lock: b.metadata.number_lock,
                number_sort_lock: b.metadata.number_sort_lock,
            },
            deleted: b.deleted,
            media: Some(Media {
                status: b.media.status,
            }),
        }
    }
}

impl From<types::ReadListDto> for ReadList {
    fn from(r: types::ReadListDto) -> Self {
        Self {
            id: r.id,
            name: r.name,
            summary: r.summary,
            ordered: r.ordered,
            book_ids: r.book_ids,
        }
    }
}

/// The spec types `numberSort` as f32; widening through its shortest decimal
/// keeps `1.1` as `1.1` rather than `1.100000023841858`.
fn widen(n: f32) -> f64 {
    n.to_string().parse().unwrap_or(n as f64)
}

struct Page<T> {
    content: Vec<T>,
    last: bool,
    total_elements: usize,
}

impl<T> Page<T> {
    fn new<D: Into<T>>(
        what: &str,
        content: Vec<D>,
        last: Option<bool>,
        total_elements: Option<i64>,
    ) -> Result<Self> {
        let (Some(last), Some(total)) = (last, total_elements) else {
            bail!("decode komga {what}: page lacks last/totalElements");
        };
        Ok(Self {
            content: content.into_iter().map(Into::into).collect(),
            last,
            total_elements: usize::try_from(total)
                .map_err(|_| anyhow!("decode komga {what}: totalElements {total}"))?,
        })
    }
}

pub trait HasId {
    fn id(&self) -> &str;
}

impl HasId for Series {
    fn id(&self) -> &str {
        &self.id
    }
}

impl HasId for Book {
    fn id(&self) -> &str {
        &self.id
    }
}

impl HasId for ReadList {
    fn id(&self) -> &str {
        &self.id
    }
}

/// Collects pages into one list. Ids are deduplicated and the result must
/// match `totalElements` exactly, or the read fails rather than plan on a
/// partial or doubled list.
struct Pager<T> {
    items: Vec<T>,
    seen: HashSet<String>,
    total: Option<usize>,
}

impl<T: HasId> Pager<T> {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            seen: HashSet::new(),
            total: None,
        }
    }

    /// Adds a page; `true` once there are no more.
    fn push(&mut self, page: Page<T>) -> Result<bool> {
        match self.total {
            None => self.total = Some(page.total_elements),
            Some(t) if t != page.total_elements => bail!(
                "komga listing changed while paging ({t} then {} rows); retry",
                page.total_elements
            ),
            Some(_) => {}
        }
        if page.content.is_empty() {
            return Ok(true);
        }
        for item in page.content {
            if self.seen.insert(item.id().to_string()) {
                self.items.push(item);
            }
        }
        Ok(page.last)
    }

    fn finish(self) -> Result<Vec<T>> {
        let total = self.total.unwrap_or(0);
        if self.items.len() != total {
            bail!(
                "komga listing returned {} distinct rows of {total}; retry",
                self.items.len()
            );
        }
        Ok(self.items)
    }
}

impl HasId for types::SeriesDto {
    fn id(&self) -> &str {
        &self.id
    }
}

impl HasId for types::BookDto {
    fn id(&self) -> &str {
        &self.id
    }
}

/// The server-side `library_id` filter must hold for every row; a stray row
/// means the filter was not applied, so fail rather than filter here.
fn in_library<D, T: From<D>>(
    what: &str,
    all: Vec<D>,
    library_id: &str,
    lib: impl Fn(&D) -> &str,
) -> Result<Vec<T>> {
    if let Some(d) = all.iter().find(|d| lib(d) != library_id) {
        bail!(
            "komga {what}: listing returned a row from library '{}'",
            lib(d)
        );
    }
    Ok(all.into_iter().map(Into::into).collect())
}

/// Every row of an unpaged read: Komga ignores `sort=id`, so paged reads have
/// no stable order a rescan could not reshuffle between pages.
fn unpaged<T: HasId>(what: &str, page: Page<T>) -> Result<Vec<T>> {
    let mut pager = Pager::new();
    if !pager.push(page)? {
        bail!("komga {what}: unpaged listing reported more pages");
    }
    pager.finish()
}

/// Error bodies the generated client keeps (`ByteStream`) or drops (`()`).
trait ErrorBody {
    fn into_bytes(self) -> Vec<u8>;
}

impl ErrorBody for ByteStream {
    fn into_bytes(self) -> Vec<u8> {
        self.into_inner()
    }
}

impl ErrorBody for () {
    fn into_bytes(self) -> Vec<u8> {
        Vec::new()
    }
}

fn fail<T, E: ErrorBody>(what: &str, e: Error<E>) -> Result<T> {
    let (status, body) = match e {
        Error::ErrorResponse(rv) => (rv.status().as_u16(), rv.into_inner().into_bytes()),
        Error::UnexpectedResponse(r) => {
            let (status, _, body) = r.into_parts_pub();
            (status.as_u16(), body)
        }
        Error::InvalidResponsePayload(_, e) => bail!("decode komga {what}: {e}"),
        Error::CommunicationError(e) => bail!("komga {what}: {e}"),
    };
    bail!(
        "komga {what}: http {status}: {}",
        clip(&String::from_utf8_lossy(&body))
    )
}

fn value<T, E: ErrorBody>(what: &str, r: Result<ResponseValue<T>, Error<E>>) -> Result<T> {
    r.map(ResponseValue::into_inner).or_else(|e| fail(what, e))
}

/// Any 2xx is success: the spec pins one status per write, Komga may differ.
fn done(what: &str, r: Result<ResponseValue<()>, Error<ByteStream>>) -> Result<()> {
    match r {
        Ok(_) => Ok(()),
        Err(Error::UnexpectedResponse(resp)) if resp.status().is_success() => Ok(()),
        Err(e) => fail(what, e),
    }
}

/// Komga ids are alphanumeric; anything else would reshape the request path.
pub fn check_id(id: &str) -> Result<&str> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("'{id}' is not a Komga id");
    }
    Ok(id)
}

impl Komga {
    pub fn new(base_url: &str, api_key: &str, insecure: bool) -> Result<Self> {
        Ok(Self {
            api: Client::new(base_url, api_key, insecure)?,
        })
    }

    pub async fn libraries(&self) -> Result<Vec<Library>> {
        let all = value("libraries", self.api.get_libraries().await)?;
        Ok(all.into_iter().map(Into::into).collect())
    }

    pub async fn library(&self, id: &str) -> Result<Library> {
        let what = format!("libraries/{}", check_id(id)?);
        value(&what, self.api.get_library_by_id(id).await).map(Into::into)
    }

    pub async fn series(&self, id: &str) -> Result<Series> {
        let what = format!("series/{}", check_id(id)?);
        value(&what, self.api.get_series_by_id(id).await).map(Into::into)
    }

    pub async fn library_series(&self, library_id: &str) -> Result<Vec<Series>> {
        let what = format!("series?library_id={}", check_id(library_id)?);
        let ids = vec![library_id.to_string()];
        let p = value(
            &what,
            self.api
                .get_series_deprecated(
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(&ids),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(true),
                )
                .await,
        )?;
        let all: Vec<types::SeriesDto> = unpaged(
            &what,
            Page::new(&what, p.content, p.last, p.total_elements)?,
        )?;
        in_library(&what, all, library_id, |s| &s.library_id)
    }

    pub async fn library_books(&self, library_id: &str) -> Result<Vec<Book>> {
        let what = format!("books?library_id={}", check_id(library_id)?);
        let ids = vec![library_id.to_string()];
        let p = value(
            &what,
            self.api
                .get_all_books_deprecated(
                    Some(&ids),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(true),
                )
                .await,
        )?;
        let all: Vec<types::BookDto> = unpaged(
            &what,
            Page::new(&what, p.content, p.last, p.total_elements)?,
        )?;
        in_library(&what, all, library_id, |b| &b.library_id)
    }

    pub async fn series_books(&self, series_id: &str) -> Result<Vec<Book>> {
        let what = format!("series/{}/books", check_id(series_id)?);
        let p = value(
            &what,
            self.api
                .get_books_by_series_id(
                    series_id,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(true),
                )
                .await,
        )?;
        unpaged(
            &what,
            Page::new(&what, p.content, p.last, p.total_elements)?,
        )
    }

    pub async fn book(&self, id: &str) -> Result<Book> {
        let what = format!("books/{}", check_id(id)?);
        value(&what, self.api.get_book_by_id(id).await).map(Into::into)
    }

    pub async fn readlists(&self) -> Result<Vec<ReadList>> {
        let what = "readlists";
        let p = value(
            what,
            self.api
                .get_read_lists(None, None, None, None, None, Some(true))
                .await,
        )?;
        unpaged(what, Page::new(what, p.content, p.last, p.total_elements)?)
    }

    /// Returns the new readlist's id.
    pub async fn create_readlist(
        &self,
        name: &str,
        summary: &str,
        book_ids: &[String],
    ) -> Result<String> {
        let body = types::ReadListCreationDto {
            book_ids: book_ids.to_vec(),
            name: name
                .try_into()
                .map_err(|e| anyhow!("komga readlists: name: {e}"))?,
            ordered: true,
            summary: summary.to_string(),
        };
        value("readlists", self.api.create_read_list(&body).await).map(|r| r.id)
    }

    /// Replaces the book list, in order; `summary` `None` leaves it as is.
    pub async fn update_readlist(
        &self,
        id: &str,
        book_ids: &[String],
        summary: Option<&str>,
    ) -> Result<()> {
        let what = format!("readlists/{}", check_id(id)?);
        // The generated body omits an empty `bookIds`, which Komga reads as
        // "keep the current books".
        if book_ids.is_empty() {
            bail!("komga {what}: no books to write");
        }
        let body = readlist_patch(book_ids, summary);
        done(&what, self.api.update_read_list_by_id(id, &body).await)
    }

    /// Set number and sort number with both locks, so rescans keep them.
    pub async fn set_number(&self, book_id: &str, number: &str, number_sort: f64) -> Result<()> {
        let what = format!("books/{}/metadata", check_id(book_id)?);
        let body = number_patch(number, number_sort);
        done(&what, self.api.update_book_metadata(book_id, &body).await)
    }

    pub async fn series_metadata(&self, id: &str) -> Result<types::SeriesMetadataDto> {
        let what = format!("series/{}", check_id(id)?);
        value(&what, self.api.get_series_by_id(id).await).map(|s| s.metadata)
    }

    pub async fn update_series_metadata(
        &self,
        id: &str,
        body: &types::SeriesMetadataUpdateDto,
    ) -> Result<()> {
        let what = format!("series/{}/metadata", check_id(id)?);
        done(&what, self.api.update_series_metadata(id, body).await)
    }

    /// Delete the book's file from disk through Komga, which drops the book.
    pub async fn delete_book_file(&self, book_id: &str) -> Result<()> {
        let what = format!("books/{}/file", check_id(book_id)?);
        done(&what, self.api.delete_book_file(book_id).await)
    }

    pub async fn enable_empty_trash_after_scan(&self, library_id: &str) -> Result<()> {
        let what = format!("libraries/{}", check_id(library_id)?);
        let body = empty_trash_patch();
        done(
            &what,
            self.api.update_library_by_id(library_id, &body).await,
        )
    }

    pub async fn scan(&self, library_id: &str, deep: bool) -> Result<()> {
        let what = format!("libraries/{}/scan?deep={deep}", check_id(library_id)?);
        done(&what, self.api.library_scan(library_id, Some(deep)).await)
    }
}

fn readlist_patch(book_ids: &[String], summary: Option<&str>) -> types::ReadListUpdateDto {
    types::ReadListUpdateDto {
        book_ids: book_ids.to_vec(),
        name: None,
        ordered: Some(true),
        summary: summary.map(str::to_string),
    }
}

fn number_patch(number: &str, number_sort: f64) -> types::BookMetadataUpdateDto {
    types::BookMetadataUpdateDto {
        authors: Vec::new(),
        authors_lock: None,
        isbn: None,
        isbn_lock: None,
        links: Vec::new(),
        links_lock: None,
        number: Some(number.to_string()),
        number_lock: Some(true),
        number_sort: Some(number_sort as f32),
        number_sort_lock: Some(true),
        release_date: None,
        release_date_lock: None,
        summary: None,
        summary_lock: None,
        tags: None,
        tags_lock: None,
        title: None,
        title_lock: None,
    }
}

fn empty_trash_patch() -> types::LibraryUpdateDto {
    types::LibraryUpdateDto {
        analyze_dimensions: None,
        convert_to_cbz: None,
        empty_trash_after_scan: Some(true),
        hash_files: None,
        hash_koreader: None,
        hash_pages: None,
        import_barcode_isbn: None,
        import_comic_info_book: None,
        import_comic_info_collection: None,
        import_comic_info_read_list: None,
        import_comic_info_series: None,
        import_comic_info_series_append_volume: None,
        import_epub_book: None,
        import_epub_series: None,
        import_local_artwork: None,
        import_mylar_series: None,
        name: None,
        oneshots_directory: None,
        repair_extensions: None,
        root: None,
        scan_cbx: None,
        scan_directory_exclusions: None,
        scan_epub: None,
        scan_force_modified_time: None,
        scan_interval: None,
        scan_on_startup: None,
        scan_pdf: None,
        series_cover: None,
    }
}

fn clip(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_toolkit::serde_json;

    #[test]
    fn ids_must_be_alphanumeric() {
        assert!(check_id("0PS084DCHJ1HH").is_ok());
        assert!(check_id("").is_err());
        assert!(check_id("../libraries").is_err());
        assert!(check_id("a?b").is_err());
    }

    fn page(ids: &[&str], last: bool, total: usize) -> Page<Series> {
        Page {
            content: ids
                .iter()
                .map(|id| Series {
                    id: (*id).into(),
                    library_id: "L".into(),
                    name: "s".into(),
                    metadata: Default::default(),
                })
                .collect(),
            last,
            total_elements: total,
        }
    }

    fn ids(v: Vec<Series>) -> Vec<String> {
        v.into_iter().map(|s| s.id).collect()
    }

    #[test]
    fn pager_collects_pages() {
        let mut p = Pager::new();
        assert!(!p.push(page(&["a", "b"], false, 3)).unwrap());
        assert!(p.push(page(&["c"], true, 3)).unwrap());
        assert_eq!(ids(p.finish().unwrap()), vec!["a", "b", "c"]);
    }

    #[test]
    fn pager_drops_repeated_ids_and_fails_short() {
        let mut p = Pager::new();
        p.push(page(&["a", "b"], false, 3)).unwrap();
        p.push(page(&["b"], true, 3)).unwrap();
        let err = p.finish().unwrap_err().to_string();
        assert!(err.contains("2 distinct rows of 3"), "{err}");
    }

    #[test]
    fn pager_stops_on_an_empty_page_and_fails_short() {
        let mut p = Pager::new();
        p.push(page(&["a"], false, 2)).unwrap();
        assert!(p.push(page(&[], false, 2)).unwrap());
        assert!(p.finish().is_err());
    }

    #[test]
    fn pager_fails_when_the_total_moves() {
        let mut p = Pager::new();
        p.push(page(&["a"], false, 2)).unwrap();
        assert!(p.push(page(&["b"], true, 3)).is_err());
    }

    #[test]
    fn readlist_patch_omits_absent_summary() {
        let ids = vec!["a".to_string()];
        let body = |summary| serde_json::to_value(readlist_patch(&ids, summary)).unwrap();
        assert_eq!(body(None), json!({"ordered": true, "bookIds": ["a"]}));
        assert_eq!(body(Some("s"))["summary"], "s");
    }

    #[test]
    fn metadata_patch_sends_only_number_and_locks() {
        // As sent: `to_vec` writes the f32 as its shortest decimal.
        let wire = serde_json::to_vec(&number_patch("1.1", 1.1)).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(
            body,
            json!({"number": "1.1", "numberSort": 1.1, "numberLock": true, "numberSortLock": true})
        );
    }

    #[test]
    fn library_patch_sends_only_empty_trash() {
        let body = serde_json::to_value(empty_trash_patch()).unwrap();
        assert_eq!(body, json!({"emptyTrashAfterScan": true}));
    }

    #[test]
    fn book_availability() {
        let mut b = Book {
            id: "B1".into(),
            series_id: "S1".into(),
            url: "/x/a.cbz".into(),
            size_bytes: 0,
            metadata: BookMetadata {
                number: "1".into(),
                number_sort: 1.0,
                number_lock: false,
                number_sort_lock: false,
            },
            deleted: false,
            media: Some(Media {
                status: "READY".into(),
            }),
        };
        assert!(b.available());
        b.deleted = true;
        assert!(!b.available());
        b.deleted = false;
        b.media = Some(Media {
            status: "ERROR".into(),
        });
        assert!(!b.available());
        b.media = None;
        assert!(!b.available());
    }

    /// Komga only matches `library_id` sent as a plain repeated key.
    #[test]
    fn array_query_params_are_repeated_keys() {
        use plugin_toolkit::progenitor_client::QueryParam;
        let ids = vec!["L1".to_string()];
        let req = plugin_toolkit::reqwest::Client::new()
            .get("http://k/api/v1/series")
            .query(&QueryParam::new("library_id", &Some(&ids)))
            .build()
            .unwrap();
        assert!(format!("{req:?}").contains("?library_id=L1\""));
    }

    #[test]
    fn library_listing_fails_on_a_foreign_row() {
        let rows = |libs: &[&str]| -> Vec<String> { libs.iter().map(|l| l.to_string()).collect() };
        let ok: Vec<String> = in_library("x", rows(&["L1", "L1"]), "L1", String::as_str).unwrap();
        assert_eq!(ok.len(), 2);
        let bad = in_library::<_, String>("x", rows(&["L1", "L2"]), "L1", String::as_str);
        assert!(bad.unwrap_err().to_string().contains("library 'L2'"));
    }

    #[test]
    fn number_sort_widens_to_its_decimal() {
        assert_eq!(widen(1.1), 1.1);
        assert_eq!(widen(45.0), 45.0);
        assert_eq!(widen(0.5), 0.5);
    }

    #[test]
    fn page_needs_totals() {
        let none: Vec<types::SeriesDto> = Vec::new();
        assert!(Page::<Series>::new("x", none, Some(true), None).is_err());
        let none: Vec<types::SeriesDto> = Vec::new();
        let p = Page::<Series>::new("x", none, Some(true), Some(0)).unwrap();
        assert!(unpaged("x", p).unwrap().is_empty());
    }

    #[test]
    fn book_file_name() {
        let b = Book {
            id: "B1".into(),
            series_id: "S1".into(),
            url: "/data/media/comics/Saga (2012)/Saga 03 (2012).cbz".into(),
            size_bytes: 42,
            metadata: BookMetadata {
                number: "1".into(),
                number_sort: 1.0,
                number_lock: false,
                number_sort_lock: true,
            },
            deleted: false,
            media: None,
        };
        assert_eq!(b.file_name(), "Saga 03 (2012).cbz");
    }
}
