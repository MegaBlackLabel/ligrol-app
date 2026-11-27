//! 表紙の参照と画像取得。パスやファイル名は返さない。

use base64::Engine as _;

use crate::ai::{AiContext, AiError, CoverImage, CoverRef};
use crate::db::{SqlitePool, books, thumbs};

fn shelf_key(book: &books::Book) -> Option<String> {
    let site = book.site_id.as_deref()?;
    let database_id = book.tbf_product_id.as_deref()?;
    Some(thumbs::shelf_key(site, database_id))
}

pub fn cover_ref(
    pool: &SqlitePool,
    ctx: &AiContext,
    book: &books::Book,
) -> Result<Option<CoverRef>, AiError> {
    let Some(key) = shelf_key(book) else {
        return Ok(None);
    };
    let Some(entry) = thumbs::get(pool, thumbs::KIND_SHELF, &key)? else {
        return Ok(None);
    };
    // 所有者が付いているキャッシュは現在の owner のものだけ返す。
    if !entry.owner_id.is_empty() && entry.owner_id != ctx.owner_id() {
        return Ok(None);
    }
    Ok(Some(CoverRef {
        uri: format!("ligrol://book/{}/cover", book.id),
        mime: entry.mime,
        width: entry.width,
        height: entry.height,
        sha256: entry.sha256,
    }))
}

pub fn cover_image(
    pool: &SqlitePool,
    ctx: &AiContext,
    book_id: &str,
) -> Result<CoverImage, AiError> {
    let visible = ctx.visible_book_ids(pool)?;
    if !visible.contains(book_id) {
        return Err(AiError::NotFound);
    }
    let book = books::get(pool, book_id)?.ok_or(AiError::NotFound)?;
    if book.is_hidden != 0 {
        return Err(AiError::NotFound);
    }
    let Some(key) = shelf_key(&book) else {
        return Err(AiError::NotFound);
    };
    let Some(entry) = thumbs::get(pool, thumbs::KIND_SHELF, &key)? else {
        return Err(AiError::NotFound);
    };
    if !entry.owner_id.is_empty() && entry.owner_id != ctx.owner_id() {
        return Err(AiError::NotFound);
    }
    Ok(CoverImage {
        book_id: book_id.to_string(),
        mime: entry.mime,
        width: entry.width,
        height: entry.height,
        sha256: entry.sha256,
        base64: base64::engine::general_purpose::STANDARD.encode(entry.bytes),
    })
}
