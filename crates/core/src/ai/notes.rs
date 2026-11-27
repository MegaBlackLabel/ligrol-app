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
