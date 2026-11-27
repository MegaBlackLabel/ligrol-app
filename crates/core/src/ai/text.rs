//! 本文セグメントの取得と検索。PDF はページ、EPUB は章、OCR はページ。

use crate::ai::{AiContext, AiError, TextMatch, TextSegment, key_for_read, library};
use crate::db::{SqlitePool, books, documents};
use crate::secrets::SecretStore;

fn segment_kind(source: &str) -> &'static str {
    if source == "epub" { "section" } else { "page" }
}

fn snippet(text: &str, needle: &str, limit: usize) -> Option<String> {
    let haystack = text.to_lowercase();
    let needle = needle.to_lowercase();
    let position = haystack.find(&needle)?;
    // 文字境界で前後を切る。
    let start_char = text[..position].chars().count().saturating_sub(60);
    let end_char = start_char + limit.min(240);
    let mut out: String = text
        .chars()
        .skip(start_char)
        .take(end_char.saturating_sub(start_char))
        .collect();
    if start_char > 0 {
        out.insert(0, '…');
    }
    if end_char < text.chars().count() {
        out.push('…');
    }
    Some(out)
}

pub fn get_book_text(
    pool: &SqlitePool,
    ctx: &AiContext,
    book_id: &str,
    from: Option<i64>,
    to: Option<i64>,
    max_bytes: Option<usize>,
) -> Result<Vec<TextSegment>, AiError> {
    ctx.require_text()?;
    let visible = ctx.visible_book_ids(pool)?;
    if !visible.contains(book_id) {
        return Err(AiError::NotFound);
    }
    let book = books::get(pool, book_id)?.ok_or(AiError::NotFound)?;
    if book.is_hidden != 0 {
        return Err(AiError::NotFound);
    }
    let Some(document) = documents::get_document_by_book_id(pool, book_id)? else {
        return Err(AiError::TextUnavailable);
    };
    let key = key_for_read(&SecretStore::new());
    let rows = documents::all_text_rows_for_document(pool, &document.id, key.as_ref())?;
    let from = from.unwrap_or(0);
    let to = to.unwrap_or(i64::MAX);
    let budget = max_bytes.unwrap_or(64 * 1024).clamp(512, 512 * 1024);
    let mut used = 0usize;
    let mut out = Vec::new();
    for row in rows {
        if row.page_number < from || row.page_number > to {
            continue;
        }
        let Some(text) = row.text else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if !out.is_empty() && used + text.len() > budget {
            break;
        }
        used += text.len();
        out.push(TextSegment {
            kind: segment_kind(&row.source).to_string(),
            ordinal: row.page_number,
            title: row.section_title,
            href: row.section_href,
            chars: text.chars().count() as i64,
            text,
            source: row.source,
            engine: row.engine,
            confidence: row.confidence,
        });
    }
    if out.is_empty() {
        return Err(AiError::TextUnavailable);
    }
    Ok(out)
}

pub fn search_text(
    pool: &SqlitePool,
    ctx: &AiContext,
    book_ids: &[String],
    query: &str,
    limit: Option<i64>,
) -> Result<Vec<TextMatch>, AiError> {
    ctx.require_text()?;
    let needle = query.trim();
    if needle.is_empty() {
        return Err(AiError::Invalid("query".to_string()));
    }
    if book_ids.is_empty() || book_ids.len() > 5 {
        return Err(AiError::Invalid("book_ids (1..=5)".to_string()));
    }
    let visible = ctx.visible_book_ids(pool)?;
    let key = key_for_read(&SecretStore::new());
    let limit = limit.unwrap_or(20).clamp(1, 50) as usize;
    let mut out = Vec::new();
    for book_id in book_ids {
        if !visible.contains(book_id) {
            continue;
        }
        let Ok(Some(book)) = books::get(pool, book_id) else {
            continue;
        };
        if book.is_hidden != 0 {
            continue;
        }
        let Some(document) = documents::get_document_by_book_id(pool, book_id)? else {
            continue;
        };
        let rows = documents::all_text_rows_for_document(pool, &document.id, key.as_ref())?;
        for row in rows {
            let Some(text) = row.text else {
                continue;
            };
            if text.to_lowercase().contains(&needle.to_lowercase())
                && let Some(snippet) = snippet(&text, needle, 160)
            {
                out.push(TextMatch {
                    book_id: book_id.clone(),
                    title: library::title_of(pool, book_id)?,
                    kind: segment_kind(&row.source).to_string(),
                    ordinal: row.page_number,
                    section_title: row.section_title,
                    href: row.section_href,
                    source: row.source,
                    snippet,
                });
                if out.len() >= limit {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}
