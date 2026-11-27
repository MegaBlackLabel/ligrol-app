//! 表紙の参照と画像取得。パスやファイル名は返さない。

use base64::Engine as _;
use sha2::{Digest, Sha256};

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
    if let Some(entry) = thumbs::get(pool, thumbs::KIND_SHELF, &key)?
        && (entry.owner_id.is_empty() || entry.owner_id == ctx.owner_id())
    {
        return Ok(CoverImage {
                book_id: book_id.to_string(),
                mime: entry.mime,
                width: entry.width,
                height: entry.height,
                sha256: entry.sha256,
            base64: base64::engine::general_purpose::STANDARD.encode(entry.bytes),
        });
    }
    // `thumbnail_share` は Drive 用の派生キャッシュ。まだ作られていない本は
    // pack 内の `thumbnail.webp` から読む（PRK が keyring にある場合だけ）。
    if let Some(data_dir) = ctx.data_dir.as_deref()
        && let Some(owner_id) = ctx.owner_sub.as_deref().map(nartik::derive_owner_id)
        && let Some(encoded) = crate::secrets::SecretStore::new()
            .load_pack_root_key(&owner_id)
            .ok()
            .flatten()
        && let Some(root) = nartik::PackRootKey::from_base64(&encoded)
    {
        let pack_id = book.pack_id.as_deref().unwrap_or(&book.id);
        if let Ok(path) = crate::pack_path::pack_path(&data_dir.join("packs"), pack_id)
            && let Ok(reader) = nartik::PackFileReader::open(&path)
            && let Ok(bytes) = reader.read_entry_with_key(
                "thumbnail.webp",
                Some(&root.derive_pack_key(pack_id)),
            )
            && let Ok(image) = image::load_from_memory(&bytes)
        {
            let sha256 = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
            return Ok(CoverImage {
                book_id: book_id.to_string(),
                mime: "image/webp".to_string(),
                width: image.width(),
                height: image.height(),
                sha256,
                base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            });
        }
    }
    Err(AiError::NotFound)
}
