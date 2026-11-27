//! タグ別の冊数・読了率・累計時間・お気に入り・Zenn の説明。

use std::collections::{HashMap, HashSet};

use sqlx::Row;

use crate::ai::{AiContext, AiError, TagStat, stats};
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
