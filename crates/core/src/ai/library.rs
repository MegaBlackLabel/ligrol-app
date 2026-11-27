//! 本棚の検索・1 冊の詳細。所有者と `is_hidden` をここで落とす。

use std::collections::HashMap;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::ai::{
    AiContext, AiError, BookDetail, BookSummary, ContentView, FormatView, Page, ProgressView,
    StatsView, TagRef, key_for_read, text_index_status,
};
use crate::db::{SqlitePool, books, contents, documents, progress, tags, view_history};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BookQuery {
    pub query: Option<String>,
    pub site_id: Option<String>,
    pub tag: Option<String>,
    /// `unread` / `reading` / `read`
    pub state: Option<String>,
    pub favorite_only: bool,
    /// `last-read`（既定）/ `title` / `purchase-date` / `release-date` / `added`
    pub sort: Option<String>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

pub fn state_label(state: progress::ReadingState) -> &'static str {
    match state {
        progress::ReadingState::Unread => "unread",
        progress::ReadingState::Reading => "reading",
        progress::ReadingState::Read => "read",
    }
}

pub fn progress_view(value: Option<&progress::ReadingProgress>) -> ProgressView {
    match value {
        Some(value) => ProgressView {
            state: state_label(progress::ReadingState::from_progress(Some(value))).to_string(),
            current_page: value.current_page,
            total_pages: value.total_pages,
            finished_at: value.finished_at.clone(),
            last_read_at: Some(value.last_read_at.clone()),
        },
        None => ProgressView::default(),
    }
}

pub(crate) fn stats_view(stats: Option<&view_history::ViewStats>) -> StatsView {
    match stats {
        Some(stats) => StatsView {
            sessions: stats.count,
            total_seconds: stats.total_seconds,
            last_read_at: stats.last_viewed_at.clone(),
        },
        None => StatsView::default(),
    }
}

fn tag_refs(tags: &[tags::BookTag]) -> Vec<TagRef> {
    tags.iter()
        .map(|tag| TagRef {
            name: tag.tag_name.clone(),
            source: tag.source.clone(),
        })
        .collect()
}

pub(crate) fn decode_cursor(cursor: Option<&str>) -> Result<usize, AiError> {
    let Some(cursor) = cursor.filter(|value| !value.is_empty()) else {
        return Ok(0);
    };
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| AiError::Invalid("cursor".to_string()))?;
    let value: serde_json::Value =
        serde_json::from_slice(&decoded).map_err(|_| AiError::Invalid("cursor".to_string()))?;
    value
        .get("offset")
        .and_then(|value| value.as_u64())
        .map(|value| value as usize)
        .ok_or_else(|| AiError::Invalid("cursor".to_string()))
}

pub(crate) fn encode_cursor(offset: usize) -> String {
    let value = serde_json::json!({ "offset": offset });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string())
}

fn progress_map(
    pool: &SqlitePool,
) -> Result<HashMap<String, progress::ReadingProgress>, AiError> {
    let rows = crate::db::block_on(async {
        sqlx::query_as::<_, progress::ReadingProgress>(
            "SELECT book_id, content_id, current_page, total_pages, finished_at, last_read_at \
             FROM reading_progress",
        )
        .fetch_all(pool)
        .await
    })?;
    let mut map = HashMap::new();
    for row in rows {
        // 同じ本に複数コンテンツの進捗がある場合は、既定表示に近い行を優先する。
        map.entry(row.book_id.clone())
            .and_modify(|current: &mut progress::ReadingProgress| {
                if row.content_id.is_empty() && !current.content_id.is_empty() {
                    *current = row.clone();
                }
            })
            .or_insert(row);
    }
    Ok(map)
}

pub fn build_summary(
    pool: &SqlitePool,
    ctx: &AiContext,
    book: &books::Book,
    book_tags: &HashMap<String, Vec<String>>,
    book_tag_rows: Option<&[tags::BookTag]>,
    stats: Option<&view_history::ViewStats>,
    progress: Option<&progress::ReadingProgress>,
) -> Result<BookSummary, AiError> {
    let tag_refs = match book_tag_rows {
        Some(rows) => tag_refs(rows),
        None => book_tags
            .get(&book.id)
            .map(|names| {
                names
                    .iter()
                    .map(|name| TagRef {
                        name: name.clone(),
                        source: "generated".to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    };
    Ok(BookSummary {
        book_id: book.id.clone(),
        title: book.title.clone(),
        author: book.author.clone(),
        circle_name: book.circle_name.clone(),
        site_id: book.site_id.clone(),
        purchase_date: book.purchase_date.clone(),
        release_date: book.release_date.clone(),
        is_favorite: book.is_favorite != 0,
        is_hidden: book.is_hidden != 0,
        page_count: book.page_count,
        tags: tag_refs,
        progress: progress_view(progress),
        stats: stats_view(stats),
        cover: crate::ai::covers::cover_ref(pool, ctx, book)?,
    })
}

pub fn search(
    pool: &SqlitePool,
    ctx: &AiContext,
    query: &BookQuery,
) -> Result<Page<BookSummary>, AiError> {
    ctx.require_library()?;
    let visible = ctx.visible_book_ids(pool)?;
    let all_books = books::list(pool)?;
    let tag_names = tags::tag_names_by_book(pool)?;
    let stats = view_history::view_stats(pool)?;
    let progress_by_book = progress_map(pool)?;
    let needle = query
        .query
        .as_deref()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty());
    let tag_filter = query
        .tag
        .as_deref()
        .map(|value| value.to_lowercase())
        .filter(|value| !value.is_empty());

    let mut matched: Vec<(&books::Book, progress::ReadingState)> = Vec::new();
    for book in &all_books {
        if !visible.contains(&book.id) || book.is_hidden != 0 {
            continue;
        }
        if let Some(site) = query.site_id.as_deref()
            && book.site_id.as_deref() != Some(site)
        {
            continue;
        }
        if query.favorite_only && book.is_favorite == 0 {
            continue;
        }
        if let Some(needle) = needle.as_deref() {
            let tags_text = tag_names
                .get(&book.id)
                .cloned()
                .unwrap_or_default()
                .join(" ");
            let haystack = format!(
                "{} {} {} {} {} {} {}",
                book.title,
                book.author,
                book.circle_name,
                tags_text,
                book.description.as_deref().unwrap_or(""),
                book.theme.as_deref().unwrap_or(""),
                book.series_name.as_deref().unwrap_or(""),
            )
            .to_lowercase();
            if !haystack.contains(needle) {
                continue;
            }
        }
        if let Some(tag_filter) = tag_filter.as_deref() {
            let has = tag_names
                .get(&book.id)
                .is_some_and(|names| names.iter().any(|name| name.to_lowercase() == tag_filter));
            if !has {
                continue;
            }
        }
        let state = progress::ReadingState::from_progress(progress_by_book.get(&book.id));
        if let Some(wanted) = query.state.as_deref()
            && state_label(state) != wanted
        {
            continue;
        }
        matched.push((book, state));
    }
    let sort = query.sort.as_deref().unwrap_or("last-read");
    matched.sort_by(|(a, _), (b, _)| {
        let order = match sort {
            "title" => a.title.cmp(&b.title),
            "purchase-date" => b.purchase_date.cmp(&a.purchase_date),
            "release-date" => b.release_date.cmp(&a.release_date),
            "added" => b.created_at.cmp(&a.created_at),
            _ => {
                let a_last = progress_by_book
                    .get(&a.id)
                    .map(|progress| progress.last_read_at.clone())
                    .or_else(|| {
                        stats
                            .get(&a.id)
                            .and_then(|stats| stats.last_viewed_at.clone())
                    })
                    .unwrap_or_default();
                let b_last = progress_by_book
                    .get(&b.id)
                    .map(|progress| progress.last_read_at.clone())
                    .or_else(|| {
                        stats
                            .get(&b.id)
                            .and_then(|stats| stats.last_viewed_at.clone())
                    })
                    .unwrap_or_default();
                b_last.cmp(&a_last)
            }
        };
        order
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.id.cmp(&b.id))
    });

    let limit = query.limit.unwrap_or(20).clamp(1, 50) as usize;
    let offset = decode_cursor(query.cursor.as_deref())?;
    let page_books = matched.iter().skip(offset).take(limit);
    let mut items = Vec::new();
    for (book, _) in page_books {
        let tag_rows = tags::list_for_book(pool, &book.id)?;
        items.push(build_summary(
            pool,
            ctx,
            book,
            &tag_names,
            Some(&tag_rows),
            stats.get(&book.id),
            progress_by_book.get(&book.id),
        )?);
    }
    let next = offset + items.len();
    Ok(Page {
        truncated: next < matched.len(),
        next_cursor: (next < matched.len()).then(|| encode_cursor(next)),
        items,
    })
}

pub fn get_book(pool: &SqlitePool, ctx: &AiContext, book_id: &str) -> Result<BookDetail, AiError> {
    ctx.require_library()?;
    let visible = ctx.visible_book_ids(pool)?;
    if !visible.contains(book_id) {
        return Err(AiError::NotFound);
    }
    let book = books::get(pool, book_id)?.ok_or(AiError::NotFound)?;
    if book.is_hidden != 0 {
        return Err(AiError::NotFound);
    }
    let tag_names = tags::tag_names_by_book(pool)?;
    let tag_rows = tags::list_for_book(pool, book_id)?;
    let stats = view_history::view_stats(pool)?;
    let summary = build_summary(
        pool,
        ctx,
        &book,
        &tag_names,
        Some(&tag_rows),
        stats.get(book_id),
        progress::get(pool, book_id)?.as_ref(),
    )?;
    let contents = contents::list_with_formats(pool, book_id)?
        .into_iter()
        .map(|(content, formats)| ContentView {
            content_id: content.content_id,
            display_name: content.display_name,
            media_kind: content.media_kind,
            is_primary: content.is_primary != 0,
            formats: formats
                .into_iter()
                .map(|format| FormatView {
                    format_id: format.format_id,
                    label: format.label,
                    format_kind: format.format_kind,
                    page_count: format.page_count,
                })
                .collect(),
        })
        .collect();
    let text = match documents::get_document_by_book_id(pool, book_id)? {
        Some(document) => {
            let key = key_for_read(&crate::secrets::SecretStore::new());
            text_index_status(pool, &document.id, key.as_ref())?
        }
        None => Default::default(),
    };
    Ok(BookDetail {
        summary,
        description: book.description,
        theme: book.theme,
        series_name: book.series_name,
        media_category: book.media_category,
        ai_type: book.ai_type,
        age_rating: book.age_rating,
        is_drm: book.is_drm,
        contents,
        text,
    })
}

/// 検索結果の 1 冊（カバー画像を含めない）。MCP の `ligrol_get_book` 用。
pub fn summary_only(
    pool: &SqlitePool,
    ctx: &AiContext,
    book_id: &str,
) -> Result<BookSummary, AiError> {
    get_book(pool, ctx, book_id).map(|detail| detail.summary)
}

/// 検索結果から本のタイトルを引く（本文検索の結果に付ける）。
pub(crate) fn title_of(pool: &SqlitePool, book_id: &str) -> Result<String, AiError> {
    Ok(books::get(pool, book_id)?
        .map(|book| book.title)
        .unwrap_or_else(|| book_id.to_string()))
}
