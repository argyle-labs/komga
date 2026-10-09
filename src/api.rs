//! Komga REST client (`/api/v1`), authenticated with an `X-API-Key` header.
//! Requests ride orca's `http.request` capability (`delegated-http`).

use std::collections::HashSet;

use plugin_toolkit::prelude::*;
use plugin_toolkit::reqwest::{Client, RequestBuilder};
use plugin_toolkit::serde_json;

pub struct Komga {
    http: Client,
    base: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "plugin_toolkit::serde", rename_all = "camelCase")]
pub struct Library {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub empty_trash_after_scan: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "plugin_toolkit::serde", rename_all = "camelCase")]
pub struct Series {
    pub id: String,
    pub library_id: String,
    /// Folder name; the title can differ when ComicInfo sets one.
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "plugin_toolkit::serde", rename_all = "camelCase")]
pub struct BookMetadata {
    pub number: String,
    pub number_sort: f64,
    #[serde(default)]
    pub number_lock: bool,
    #[serde(default)]
    pub number_sort_lock: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "plugin_toolkit::serde", rename_all = "camelCase")]
pub struct Book {
    pub id: String,
    pub series_id: String,
    /// Absolute file path inside Komga's container.
    pub url: String,
    #[serde(default)]
    pub size_bytes: i64,
    pub metadata: BookMetadata,
    /// Komga keeps a book whose file vanished, flagged, until the trash is
    /// emptied.
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub media: Option<Media>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "plugin_toolkit::serde")]
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

#[derive(Deserialize)]
#[serde(crate = "plugin_toolkit::serde", rename_all = "camelCase")]
struct Page<T> {
    content: Vec<T>,
    last: bool,
    total_elements: usize,
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

#[derive(Serialize)]
#[serde(crate = "plugin_toolkit::serde", rename_all = "camelCase")]
struct NumberPatch<'a> {
    number: &'a str,
    number_sort: f64,
    number_lock: bool,
    number_sort_lock: bool,
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
        let http = plugin_toolkit::api_client::ApiClientBuilder::new()
            .header("X-API-Key", api_key)?
            .insecure(insecure)
            .build()?;
        Ok(Self {
            http,
            base: base_url.trim_end_matches('/').to_string(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1/{path}", self.base)
    }

    async fn send(&self, req: RequestBuilder, what: &str) -> Result<Vec<u8>> {
        let resp = req.send().await.map_err(|e| anyhow!("komga {what}: {e}"))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| anyhow!("komga {what}: {e}"))?;
        if !status.is_success() {
            bail!(
                "komga {what}: http {}: {}",
                status.as_u16(),
                clip(&String::from_utf8_lossy(&body))
            );
        }
        Ok(body)
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let body = self.send(self.http.get(self.url(path)), path).await?;
        serde_json::from_slice(&body).map_err(|e| anyhow!("decode komga {path}: {e}"))
    }

    /// Every row in one unpaged read: Komga ignores `sort=id`, so paged reads
    /// have no stable order a rescan could not reshuffle between pages.
    async fn listing<T: for<'de> Deserialize<'de> + HasId>(&self, path: &str) -> Result<Vec<T>> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut pager = Pager::new();
        let page: Page<T> = self.get(&format!("{path}{sep}unpaged=true")).await?;
        if !pager.push(page)? {
            bail!("komga {path}: unpaged listing reported more pages");
        }
        pager.finish()
    }

    pub async fn libraries(&self) -> Result<Vec<Library>> {
        self.get("libraries").await
    }

    pub async fn library(&self, id: &str) -> Result<Library> {
        self.get(&format!("libraries/{}", check_id(id)?)).await
    }

    pub async fn series(&self, id: &str) -> Result<Series> {
        self.get(&format!("series/{}", check_id(id)?)).await
    }

    pub async fn library_series(&self, library_id: &str) -> Result<Vec<Series>> {
        self.listing(&format!("series?library_id={}", check_id(library_id)?))
            .await
    }

    pub async fn library_books(&self, library_id: &str) -> Result<Vec<Book>> {
        self.listing(&format!("books?library_id={}", check_id(library_id)?))
            .await
    }

    pub async fn series_books(&self, series_id: &str) -> Result<Vec<Book>> {
        self.listing(&format!("series/{}/books", check_id(series_id)?))
            .await
    }

    /// Set number and sort number with both locks, so rescans keep them.
    pub async fn set_number(&self, book_id: &str, number: &str, number_sort: f64) -> Result<()> {
        let path = format!("books/{}/metadata", check_id(book_id)?);
        let body = NumberPatch {
            number,
            number_sort,
            number_lock: true,
            number_sort_lock: true,
        };
        self.send(self.http.patch(self.url(&path)).json(&body), &path)
            .await
            .map(drop)
    }

    pub async fn scan(&self, library_id: &str, deep: bool) -> Result<()> {
        let path = format!("libraries/{}/scan?deep={deep}", check_id(library_id)?);
        self.send(self.http.post(self.url(&path)), &path)
            .await
            .map(drop)
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
    fn book_availability() {
        let mut b: Book = serde_json::from_value(json!({
            "id": "B1", "seriesId": "S1", "url": "/x/a.cbz",
            "metadata": {"number": "1", "numberSort": 1.0},
            "deleted": false, "media": {"status": "READY"}
        }))
        .unwrap();
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

    #[test]
    fn book_metadata_decodes_from_komga_json() {
        let b: Book = serde_json::from_value(json!({
            "id": "B1", "seriesId": "S1", "libraryId": "L1",
            "url": "/data/media/comics/Saga (2012)/Saga 03 (2012).cbz",
            "sizeBytes": 42,
            "metadata": {"number": "1", "numberSort": 1.0, "numberLock": false, "numberSortLock": true}
        }))
        .unwrap();
        assert_eq!(b.file_name(), "Saga 03 (2012).cbz");
        assert_eq!(b.metadata.number_sort, 1.0);
        assert!(b.metadata.number_sort_lock);
    }
}
