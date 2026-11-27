//! 付箋の読み取り。メモは既存の `db::notes` が復号する。

use crate::ai::{AiContext, AiError, NoteView};
use crate::db::{SqlitePool, books, notes};

pub fn list_notes(
    pool: &SqlitePool,
    ctx: &AiContext,
    book_id: Option<&str>,
    page: Option<i64>,
) -> Result<Vec<NoteView>, AiError> {
    ctx.require_notes()?;
    let visible = ctx.visible_book_ids(pool)?;
    let all = notes::list_newest_first(pool)?;
    let mut out = Vec::new();
    for note in all {
        if !visible.contains(&note.book_id) {
            continue;
        }
        if let Some(book_id) = book_id
            && note.book_id != book_id
        {
            continue;
        }
        if let Some(page) = page
            && note.page != page
        {
            continue;
        }
        let book = books::get(pool, &note.book_id)?;
        if book.as_ref().is_some_and(|book| book.is_hidden != 0) {
            continue;
        }
        out.push(NoteView {
            book_id: note.book_id.clone(),
            book_title: book.map(|book| book.title).unwrap_or_default(),
            content_id: note.content_id,
            page: note.page,
            memo: note.memo,
            is_active: note.is_active,
            spread_side: note.spread_side.map(|side| side.as_str().to_string()),
            updated_at: note.updated_at,
        });
    }
    Ok(out)
}


/// 付箋を追加 / 更新する。`expected_updated_at` が現在値と違えば競合。
pub fn add_note(
    pool: &SqlitePool,
    ctx: &AiContext,
    book_id: &str,
    content_id: &str,
    page: i64,
    memo: &str,
    expected_updated_at: Option<&str>,
) -> Result<NoteView, AiError> {
    ctx.require_write_notes()?;
    if page < 1 {
        return Err(AiError::Invalid("page must be >= 1".to_string()));
    }
    if memo.chars().count() > 10_000 {
        return Err(AiError::Invalid("memo is too long".to_string()));
    }
    let visible = ctx.visible_book_ids(pool)?;
    if !visible.contains(book_id) {
        return Err(AiError::NotFound);
    }
    let book = books::get(pool, book_id)?.ok_or(AiError::NotFound)?;
    if book.is_hidden != 0 {
        return Err(AiError::NotFound);
    }
    let current = notes::get_for_page(pool, book_id, content_id, page)?;
    if let Some(expected) = expected_updated_at {
        match &current {
            Some(current) if current.updated_at == expected => {}
            _ => return Err(AiError::Conflict),
        }
    }
    let id = format!("note-{book_id}-{content_id}-{page}");
    notes::upsert(
        pool,
        &notes::PageNoteInput {
            id: &id,
            book_id,
            content_id,
            page,
            memo,
            spread_side: None,
        },
    )?;
    let note = notes::get_for_page(pool, book_id, content_id, page)?.ok_or(AiError::NotFound)?;
    Ok(NoteView {
        book_id: note.book_id,
        book_title: book.title,
        content_id: note.content_id,
        page: note.page,
        memo: note.memo,
        is_active: note.is_active,
        spread_side: note.spread_side.map(|side| side.as_str().to_string()),
        updated_at: note.updated_at,
    })
}

/// 付箋を外す（メモは残す）。`expected_updated_at` が現在値と違えば競合。
pub fn remove_note(
    pool: &SqlitePool,
    ctx: &AiContext,
    book_id: &str,
    content_id: &str,
    page: i64,
    expected_updated_at: Option<&str>,
) -> Result<NoteView, AiError> {
    ctx.require_write_notes()?;
    let visible = ctx.visible_book_ids(pool)?;
    if !visible.contains(book_id) {
        return Err(AiError::NotFound);
    }
    let current = notes::get_for_page(pool, book_id, content_id, page)?.ok_or(AiError::NotFound)?;
    if let Some(expected) = expected_updated_at
        && current.updated_at != expected
    {
        return Err(AiError::Conflict);
    }
    notes::set_active(pool, book_id, content_id, page, false)?;
    let note = notes::get_for_page(pool, book_id, content_id, page)?.ok_or(AiError::NotFound)?;
    let title = books::get(pool, book_id)?
        .map(|book| book.title)
        .unwrap_or_default();
    Ok(NoteView {
        book_id: note.book_id,
        book_title: title,
        content_id: note.content_id,
        page: note.page,
        memo: note.memo,
        is_active: note.is_active,
        spread_side: note.spread_side.map(|side| side.as_str().to_string()),
        updated_at: note.updated_at,
    })
}
