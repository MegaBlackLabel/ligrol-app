//! 購入済み・未取込の一覧。AI のダウンロード要求の検証にも使う。

use std::collections::HashSet;

use crate::ai::{AiContext, AiError, DownloadChoice, PurchaseItem};
use crate::db::{SqlitePool, books, bookshelf};

fn downloaded_keys(pool: &SqlitePool) -> Result<HashSet<(String, String)>, AiError> {
    Ok(books::list(pool)?
        .into_iter()
        .filter_map(|book| {
            let site = book.site_id?;
            let product = book.tbf_product_id?;
            Some((site, product))
        })
        .collect())
}

pub fn list_purchases(
    pool: &SqlitePool,
    ctx: &AiContext,
    site_id: Option<&str>,
    downloadable_only: bool,
) -> Result<Vec<PurchaseItem>, AiError> {
    ctx.require_purchases()?;
    let owner_id = ctx.owner_id();
    let downloaded = downloaded_keys(pool)?;
    let mut items: Vec<PurchaseItem> = bookshelf::list_all_for_owner(pool, &owner_id)?
        .into_iter()
        .filter(|item| item.is_hidden == 0)
        .filter(|item| site_id.is_none_or(|site| item.site_id == site))
        .filter(|item| !downloadable_only || item.is_downloadable != 0)
        .map(|item| {
            let key = (item.site_id.clone(), item.database_id.clone());
            let choices = item
                .download_choices()
                .into_iter()
                .enumerate()
                .map(|(index, choice)| DownloadChoice {
                    index,
                    name: choice.name,
                })
                .collect();
            PurchaseItem {
                downloaded: downloaded.contains(&key),
                site_id: item.site_id,
                database_id: item.database_id,
                title: item.title,
                circle_name: item.circle_name,
                author: item.author,
                format: item.format,
                is_downloadable: item.is_downloadable != 0,
                is_purchased: item.is_purchased != 0,
                choices,
            }
        })
        .collect();
    items.sort_by(|a, b| {
        a.title
            .cmp(&b.title)
            .then_with(|| a.site_id.cmp(&b.site_id))
            .then_with(|| a.database_id.cmp(&b.database_id))
    });
    Ok(items)
}

pub(crate) fn find_purchase(
    pool: &SqlitePool,
    ctx: &AiContext,
    site_id: &str,
    database_id: &str,
) -> Result<Option<bookshelf::BookshelfItem>, AiError> {
    let owner_id = ctx.owner_id();
    Ok(bookshelf::list_all_for_owner(pool, &owner_id)?
        .into_iter()
        .find(|item| item.site_id == site_id && item.database_id == database_id))
}
