//! タグ別の冊数・読了率・累計時間・お気に入り・Zenn の説明。

use std::collections::{HashMap, HashSet};

use sqlx::Row;

use crate::ai::{AiContext, AiError, TagRef, TagStat, stats};
use crate::db::{SqlitePool, books, progress, tags, view_history};

pub fn list_tags(
    pool: &SqlitePool,
    ctx: &AiContext,
    min_books: Option<i64>,
) -> Result<Vec<TagStat>, AiError> {
    ctx.require_library()?;
    let visible = ctx.visible_book_ids(pool)?;
    let all_books = books::list(pool)?;
    let tag_names = tags::tag_names_by_book(pool)?;
    let favorites: HashSet<String> = tags::list_favorites(pool)?.into_iter().collect();
    let stats = view_history::view_stats(pool)?;
    let min_books = min_books.unwrap_or(1).max(1);

    let mut aggregated: HashMap<String, TagStat> = HashMap::new();
    for book in &all_books {
        if !visible.contains(&book.id) || book.is_hidden != 0 {
            continue;
        }
        let finished = progress::ReadingState::from_progress(
            progress::get(pool, &book.id).ok().flatten().as_ref(),
        ) == progress::ReadingState::Read;
        let seconds = stats::book_seconds(&stats, &book.id);
        for tag in tag_names.get(&book.id).cloned().unwrap_or_default() {
            let row = aggregated.entry(tag.clone()).or_insert_with(|| TagStat {
                tag: tag.clone(),
                count: 0,
                finished: 0,
                total_seconds: 0,
                favorite: favorites.contains(&tag),
                description: None,
            });
            row.count += 1;
            if finished {
                row.finished += 1;
            }
            row.total_seconds += seconds;
        }
    }
    // Zenn のタグ説明（あれば）。
    let descriptions: HashMap<String, String> = crate::db::block_on(async {
        let rows = sqlx::query("SELECT tag_name, description FROM zenn_tag_metadata")
            .fetch_all(pool)
            .await?;
        Ok::<_, sqlx::Error>(
            rows.into_iter()
                .filter_map(|row| {
                    let name: String = row.get("tag_name");
                    let description: Option<String> = row.get("description");
                    description.map(|description| (name, description))
                })
                .collect(),
        )
    })?;
    let mut rows: Vec<TagStat> = aggregated
        .into_values()
        .filter(|row| row.count >= min_books)
        .map(|mut row| {
            row.description = descriptions.get(&row.tag).cloned();
            row
        })
        .collect();
    rows.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| b.total_seconds.cmp(&a.total_seconds))
            .then_with(|| a.tag.cmp(&b.tag))
    });
    Ok(rows)
}


fn validate_tag(tag: &str) -> Result<&str, AiError> {
    let tag = tag.trim();
    if tag.is_empty() || tag.chars().count() > 64 || tag.chars().any(char::is_control) {
        return Err(AiError::Invalid("tag".to_string()));
    }
    Ok(tag)
}

/// 手動タグを追加する（既にあればそのまま返す）。
pub fn add_tag(
    pool: &SqlitePool,
    ctx: &AiContext,
    book_id: &str,
    tag: &str,
) -> Result<TagRef, AiError> {
    ctx.require_write_tags()?;
    let tag = validate_tag(tag)?;
    let visible = ctx.visible_book_ids(pool)?;
    if !visible.contains(book_id) {
        return Err(AiError::NotFound);
    }
    let book = books::get(pool, book_id)?.ok_or(AiError::NotFound)?;
    if book.is_hidden != 0 {
        return Err(AiError::NotFound);
    }
    if let Some(existing) = tags::list_for_book(pool, book_id)?
        .into_iter()
        .find(|row| row.tag_name == tag)
    {
        return Ok(TagRef {
            name: existing.tag_name,
            source: existing.source,
        });
    }
    crate::db::block_on(async {
        sqlx::query(
            "INSERT INTO book_tags (id, book_id, tag_name, source) VALUES (?1, ?2, ?3, 'manual')",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(book_id)
        .bind(tag)
        .execute(pool)
        .await?;
        Ok::<_, sqlx::Error>(())
    })?;
    Ok(TagRef {
        name: tag.to_string(),
        source: "manual".to_string(),
    })
}

/// タグを削除する（生成タグも削除対象。再取り込みで再生成され得る）。
pub fn remove_tag(
    pool: &SqlitePool,
    ctx: &AiContext,
    book_id: &str,
    tag: &str,
) -> Result<bool, AiError> {
    ctx.require_write_tags()?;
    let tag = validate_tag(tag)?;
    let visible = ctx.visible_book_ids(pool)?;
    if !visible.contains(book_id) {
        return Err(AiError::NotFound);
    }
    let deleted = crate::db::block_on(async {
        let result = sqlx::query("DELETE FROM book_tags WHERE book_id = ?1 AND tag_name = ?2")
            .bind(book_id)
            .bind(tag)
            .execute(pool)
            .await?;
        Ok::<_, sqlx::Error>(result.rows_affected() > 0)
    })?;
    Ok(deleted)
}
