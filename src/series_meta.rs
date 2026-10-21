use plugin_toolkit::prelude::*;

use crate::komga_api::types;

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesMetadataView {
    pub title: String,
    pub title_sort: String,
    pub status: String,
    pub summary: String,
    pub publisher: String,
    pub reading_direction: String,
    pub language: String,
    pub age_rating: Option<i32>,
    pub total_book_count: Option<i32>,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub locked: Vec<String>,
}

impl From<&types::SeriesMetadataDto> for SeriesMetadataView {
    fn from(m: &types::SeriesMetadataDto) -> Self {
        let locked = [
            ("title", m.title_lock),
            ("title_sort", m.title_sort_lock),
            ("status", m.status_lock),
            ("summary", m.summary_lock),
            ("publisher", m.publisher_lock),
            ("reading_direction", m.reading_direction_lock),
            ("language", m.language_lock),
            ("age_rating", m.age_rating_lock),
            ("total_book_count", m.total_book_count_lock),
            ("genres", m.genres_lock),
            ("tags", m.tags_lock),
        ]
        .into_iter()
        .filter(|(_, l)| *l)
        .map(|(f, _)| f.to_string())
        .collect();
        let mut genres = m.genres.clone();
        genres.sort();
        let mut tags = m.tags.clone();
        tags.sort();
        Self {
            title: m.title.clone(),
            title_sort: m.title_sort.clone(),
            status: m.status.clone(),
            summary: m.summary.clone(),
            publisher: m.publisher.clone(),
            reading_direction: m.reading_direction.clone(),
            language: m.language.clone(),
            age_rating: m.age_rating,
            total_book_count: m.total_book_count,
            genres,
            tags,
            locked,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Wanted {
    pub title: Option<String>,
    pub title_sort: Option<String>,
    pub status: Option<String>,
    pub summary: Option<String>,
    pub publisher: Option<String>,
    pub reading_direction: Option<String>,
    pub language: Option<String>,
    pub age_rating: Option<i32>,
    pub total_book_count: Option<i32>,
    pub genres: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
}

fn sorted(v: &[String]) -> Vec<String> {
    let mut v = v.to_vec();
    v.sort();
    v.dedup();
    v
}

pub fn plan(
    current: &SeriesMetadataView,
    want: &Wanted,
) -> Result<(types::SeriesMetadataUpdateDto, Vec<String>)> {
    let mut body = types::SeriesMetadataUpdateDto::default();
    let mut changes = Vec::new();
    let mut text = |field: &str, have: &str, want: &Option<String>| -> Option<String> {
        let w = want.as_ref()?;
        (w != have).then(|| {
            changes.push(format!("{field}: {have:?} -> {w:?}"));
            w.clone()
        })
    };
    if let Some(v) = text("title", &current.title, &want.title) {
        body.title = Some(v);
        body.title_lock = Some(true);
    }
    if let Some(v) = text("title_sort", &current.title_sort, &want.title_sort) {
        body.title_sort = Some(v);
        body.title_sort_lock = Some(true);
    }
    if let Some(v) = text("summary", &current.summary, &want.summary) {
        body.summary = Some(v);
        body.summary_lock = Some(true);
    }
    if let Some(v) = text("publisher", &current.publisher, &want.publisher) {
        body.publisher = Some(v);
        body.publisher_lock = Some(true);
    }
    if let Some(v) = text("language", &current.language, &want.language) {
        body.language = Some(v);
        body.language_lock = Some(true);
    }
    let status = want.status.as_ref().map(|s| s.to_uppercase());
    if let Some(v) = text("status", &current.status, &status) {
        body.status = Some(
            v.parse()
                .map_err(|_| anyhow!("status must be ENDED, ONGOING, ABANDONED or HIATUS"))?,
        );
        body.status_lock = Some(true);
    }
    let direction = want.reading_direction.as_ref().map(|s| s.to_uppercase());
    if let Some(v) = text("reading_direction", &current.reading_direction, &direction) {
        body.reading_direction = Some(v.parse().map_err(|_| {
            anyhow!("reading_direction must be LEFT_TO_RIGHT, RIGHT_TO_LEFT, VERTICAL or WEBTOON")
        })?);
        body.reading_direction_lock = Some(true);
    }
    if let Some(w) = want.age_rating.filter(|w| current.age_rating != Some(*w)) {
        changes.push(format!("age_rating: {:?} -> {w}", current.age_rating));
        body.age_rating = Some(w);
        body.age_rating_lock = Some(true);
    }
    if let Some(w) = want
        .total_book_count
        .filter(|w| current.total_book_count != Some(*w))
    {
        changes.push(format!(
            "total_book_count: {:?} -> {w}",
            current.total_book_count
        ));
        body.total_book_count = Some(w);
        body.total_book_count_lock = Some(true);
    }
    if let Some(w) = want
        .genres
        .as_deref()
        .map(sorted)
        .filter(|w| *w != current.genres)
    {
        changes.push(format!("genres: {:?} -> {w:?}", current.genres));
        body.genres = Some(w);
        body.genres_lock = Some(true);
    }
    if let Some(w) = want
        .tags
        .as_deref()
        .map(sorted)
        .filter(|w| *w != current.tags)
    {
        changes.push(format!("tags: {:?} -> {w:?}", current.tags));
        body.tags = Some(w);
        body.tags_lock = Some(true);
    }
    Ok((body, changes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current() -> SeriesMetadataView {
        SeriesMetadataView {
            title: "Secret Wars".into(),
            title_sort: "Secret Wars".into(),
            status: "ONGOING".into(),
            summary: String::new(),
            publisher: "Marvel".into(),
            reading_direction: "LEFT_TO_RIGHT".into(),
            language: "en".into(),
            age_rating: None,
            total_book_count: None,
            genres: vec![],
            tags: vec!["event".into()],
            locked: vec![],
        }
    }

    #[test]
    fn in_sync_fields_produce_no_changes() {
        let want = Wanted {
            title: Some("Secret Wars".into()),
            status: Some("ongoing".into()),
            tags: Some(vec!["event".into(), "event".into()]),
            ..Default::default()
        };
        let (_, changes) = plan(&current(), &want).unwrap();
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn changed_fields_are_sent_and_locked() {
        let want = Wanted {
            status: Some("ended".into()),
            total_book_count: Some(9),
            genres: Some(vec!["superhero".into()]),
            ..Default::default()
        };
        let (body, changes) = plan(&current(), &want).unwrap();
        assert_eq!(changes.len(), 3, "{changes:?}");
        assert_eq!(body.status_lock, Some(true));
        assert_eq!(body.total_book_count, Some(9));
        assert_eq!(body.genres.as_deref(), Some(&["superhero".to_string()][..]));
        assert!(body.title.is_none() && body.title_lock.is_none());
        let json = plugin_toolkit::serde_json::to_value(&body).unwrap();
        assert_eq!(json["status"], "ENDED");
        assert!(json.get("links").is_none() && json.get("alternateTitles").is_none());
    }

    #[test]
    fn bad_enum_values_are_refused() {
        let want = Wanted {
            status: Some("finished".into()),
            ..Default::default()
        };
        assert!(plan(&current(), &want).is_err());
    }
}
