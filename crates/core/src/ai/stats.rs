//! 読書統計。日別履歴を正として、本 / タグ / サイト / 月に集計する。

use std::collections::HashMap;

use crate::ai::{AiContext, AiError, ReadingStats, StatRow, StatsView};
use crate::db::{SqlitePool, books, progress, tags, view_history};

fn in_range(day: &str, from: Option<&str>, to: Option<&str>) -> bool {
    if let Some(from) = from
        && day < from
    {
        return false;
    }
    if let Some(to) = to
        && day > to
    {
        return false;
    }
    true
}

pub fn reading_stats(
    pool: &SqlitePool,
    ctx: &AiContext,
    group_by: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<ReadingStats, AiError> {
    ctx.require_stats()?;
    let visible = ctx.visible_book_ids(pool)?;
    let all_books = books::list(pool)?;
    let by_id: HashMap<&str, &books::Book> = all_books
        .iter()
        .map(|book| (book.id.as_str(), book))
        .collect();
    let daily = view_history::list_daily(pool)?;
    let mut per_book: HashMap<String, (i64, i64)> = HashMap::new();
    for entry in daily {
        if !in_range(&entry.day, from, to) || !visible.contains(&entry.book_id) {
            continue;
        }
        let slot = per_book.entry(entry.book_id).or_default();
        slot.0 += entry.sessions;
        slot.1 += entry.duration_secs;
    }
    let stats = view_history::view_stats(pool)?;
    let mut totals = StatsView::default();
    for (book_id, (sessions, seconds)) in &per_book {
        totals.sessions += sessions;
        totals.total_seconds += seconds;
        if let Some(value) = stats.get(book_id).and_then(|s| s.last_viewed_at.clone())
            && totals.last_read_at.as_deref().unwrap_or("") < value.as_str()
        {
            totals.last_read_at = Some(value);
        }
    }

    let mut rows: HashMap<String, StatRow> = HashMap::new();
    if group_by == "month" {
        for entry in view_history::list_daily(pool)? {
            if !in_range(&entry.day, from, to) || !visible.contains(&entry.book_id) {
                continue;
            }
            let month = entry.day.get(..7).unwrap_or(&entry.day).to_string();
            let row = rows.entry(month.clone()).or_insert_with(|| StatRow {
                key: month.clone(),
                label: month,
                books: 0,
                sessions: 0,
                total_seconds: 0,
                finished: 0,
            });
            row.sessions += entry.sessions;
            row.total_seconds += entry.duration_secs;
        }
        // 月の books / finished は、その月に読んだ本の集合で数える。
        let mut month_books: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for entry in view_history::list_daily(pool)? {
            if !in_range(&entry.day, from, to) || !visible.contains(&entry.book_id) {
                continue;
            }
            let month = entry.day.get(..7).unwrap_or(&entry.day).to_string();
            month_books.entry(month).or_default().insert(entry.book_id);
        }
        for (month, books) in month_books {
            if let Some(row) = rows.get_mut(&month) {
                row.books = books.len() as i64;
                row.finished = books
                    .iter()
                    .filter(|book_id| {
                        progress::ReadingState::from_progress(
                            progress::get(pool, book_id).ok().flatten().as_ref(),
                        ) == progress::ReadingState::Read
                    })
                    .count() as i64;
            }
        }
    } else {
        let tag_names = tags::tag_names_by_book(pool)?;
        for (book_id, (sessions, seconds)) in &per_book {
            let Some(book) = by_id.get(book_id.as_str()) else {
                continue;
            };
            let finished = progress::ReadingState::from_progress(
                progress::get(pool, book_id).ok().flatten().as_ref(),
            ) == progress::ReadingState::Read;
            let keys: Vec<(String, String)> = match group_by {
                "book" => vec![(book.id.clone(), book.title.clone())],
                "site" => vec![(
                    book.site_id.clone().unwrap_or_default(),
                    book.site_id
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                )],
                "tag" => tag_names
                    .get(book_id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tag| (tag.clone(), tag))
                    .collect(),
                other => {
                    return Err(AiError::Invalid(format!("group_by: {other}")));
                }
            };
            for (key, label) in keys {
                if key.is_empty() {
                    continue;
                }
                let row = rows.entry(key.clone()).or_insert_with(|| StatRow {
                    key,
                    label,
                    books: 0,
                    sessions: 0,
                    total_seconds: 0,
                    finished: 0,
                });
                row.books += 1;
                row.sessions += sessions;
                row.total_seconds += seconds;
                if finished {
                    row.finished += 1;
                }
            }
        }
    }
    let mut rows: Vec<StatRow> = rows.into_values().collect();
    rows.sort_by(|a, b| {
        b.total_seconds
            .cmp(&a.total_seconds)
            .then_with(|| a.label.cmp(&b.label))
    });
    Ok(ReadingStats {
        group_by: group_by.to_string(),
        rows,
        totals,
    })
}

pub(crate) fn book_seconds(stats: &HashMap<String, view_history::ViewStats>, book_id: &str) -> i64 {
    stats
        .get(book_id)
        .map(|value| value.total_seconds)
        .unwrap_or(0)
}
