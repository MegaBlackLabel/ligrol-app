//! `tbf_events` / `checked_items` repository (チェックリスト).

use crate::db::SqlitePool;
use sqlx::Row;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TbfEvent {
    pub id: String,
    pub site_id: String,
    pub slug: Option<String>,
    pub tbf_event_id: Option<String>,
    pub event_name: String,
    pub event_date: Option<String>,
    pub event_start_date: Option<String>,
    pub event_end_date: Option<String>,
    pub event_format: String,
    pub is_cancelled: i64,
    pub display_order: i64,
    pub is_featured: i64,
    /// 「このイベントについて、技術書典から最新状況を同期する」トグル。
    /// 1 = ポーリング対象。注目イベントは新規作成時に既定で 1 になり、
    /// 既存行ではユーザーの設定を保持する（サーバー同期で上書きしない）。
    pub poll_sync_enabled: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CheckedItem {
    pub id: String,
    pub event_id: String,
    pub circle_name: String,
    pub space_number: String,
    pub memo: String,
    pub is_checked: i64,
    pub sort_order: i64,
    pub tbf_circle_id: Option<String>,
    pub product_id: Option<String>,
    pub product_title: String,
    pub thumbnail_url: Option<String>,
    pub thumbnail_data: Option<String>,
    pub price: Option<i64>,
    pub is_purchased: i64,
    /// 初出イベント名（技術書典の ProductInfo.firstAppearanceEventName）。
    pub first_appearance_event_name: Option<String>,
    pub sample_fetch_attempted_at: Option<String>,
    #[sqlx(rename = "createdAt")]
    pub created_at: String,
}

/// Upsert a `tbf_events` row.
///
/// `poll_sync_enabled` は**新規行では呼び出し側の値をそのまま**入れる
/// （`tbf::sync` が注目イベントなら 1 を渡す）。既存行ではユーザーの設定を
/// 保持するが、**非注目 → 注目**に変わったときだけ既定（呼び出し側の値）を入れる
/// （本棚同期が先に作った行でも、注目イベントになった時点でポーリングが始まる）。
pub fn upsert_event(pool: &SqlitePool, event: &TbfEvent) -> Result<(), sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query(
            "INSERT INTO tbf_events (id, site_id, slug, tbf_event_id, event_name, event_date, \
             event_start_date, event_end_date, event_format, is_cancelled, display_order, \
             is_featured, poll_sync_enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO UPDATE SET
               site_id = excluded.site_id,
               slug = excluded.slug,
               tbf_event_id = excluded.tbf_event_id,
               event_name = excluded.event_name,
               event_date = excluded.event_date,
               event_start_date = excluded.event_start_date,
               event_end_date = excluded.event_end_date,
               event_format = excluded.event_format,
               is_cancelled = excluded.is_cancelled,
               display_order = excluded.display_order,
               is_featured = excluded.is_featured,
               poll_sync_enabled = CASE
                 WHEN tbf_events.is_featured = 0 AND excluded.is_featured <> 0
                 THEN excluded.poll_sync_enabled
                 ELSE tbf_events.poll_sync_enabled
               END,
               updated_at = excluded.updated_at",
        )
        .bind(&event.id)
        .bind(&event.site_id)
        .bind(&event.slug)
        .bind(&event.tbf_event_id)
        .bind(&event.event_name)
        .bind(&event.event_date)
        .bind(&event.event_start_date)
        .bind(&event.event_end_date)
        .bind(&event.event_format)
        .bind(event.is_cancelled)
        .bind(event.display_order)
        .bind(event.is_featured)
        .bind(event.poll_sync_enabled)
        .bind(&event.created_at)
        .bind(&event.updated_at)
        .execute(pool)
        .await?;
        Ok(())
    })
}

pub fn list_events(pool: &SqlitePool) -> Result<Vec<TbfEvent>, sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query_as::<_, TbfEvent>(
            "SELECT id, site_id, slug, tbf_event_id, event_name, event_date, event_start_date, \
             event_end_date, event_format, is_cancelled, display_order, is_featured, \
             poll_sync_enabled, created_at, updated_at FROM tbf_events \
             ORDER BY display_order, created_at DESC",
        )
        .fetch_all(pool)
        .await
    })
}

/// 「このイベントについて、技術書典から最新状況を同期する」トグルを保存する。
pub fn set_poll_enabled(pool: &SqlitePool, slug: &str, enabled: bool) -> Result<(), sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query("UPDATE tbf_events SET poll_sync_enabled = ?1 WHERE slug = ?2")
            .bind(enabled as i64)
            .bind(slug)
            .execute(pool)
            .await?;
        Ok(())
    })
}

/// `keep_slug` 以外の注目イベントを非注目へ戻す（注目イベントは 1 つだけ）。
///
/// canonical イベントを DB へ種まきしたとき、旧版が注目にしていた行
/// （技術書典20 など）が残ると注目が 2 つになるため、現在の注目イベントへ揃える。
/// `poll_sync_enabled` は変更しない（利用者のポーリング設定を保持する）。
pub fn demote_featured_except(pool: &SqlitePool, keep_slug: &str) -> Result<u64, sqlx::Error> {
    crate::db::block_on(async {
        let result = sqlx::query(
            "UPDATE tbf_events SET is_featured = 0, updated_at = CURRENT_TIMESTAMP \
             WHERE is_featured <> 0 AND (slug IS NULL OR slug <> ?1)",
        )
        .bind(keep_slug)
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    })
}

/// ポーリング対象（poll_sync_enabled = 1）のイベント slug 一覧を返す。
pub fn list_enabled_slugs(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query_scalar::<_, String>(
            "SELECT slug FROM tbf_events WHERE poll_sync_enabled = 1 AND slug IS NOT NULL \
             ORDER BY display_order",
        )
        .fetch_all(pool)
        .await
    })
}

pub fn upsert_item(pool: &SqlitePool, item: &CheckedItem) -> Result<(), sqlx::Error> {
    upsert_item_for_owner(pool, item, "")
}

/// 所有者を明示してチェックリスト項目を upsert する。
pub fn upsert_item_for_owner(
    pool: &SqlitePool,
    item: &CheckedItem,
    owner_id: &str,
) -> Result<(), sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query(
            "INSERT INTO checked_items (id, owner_id, event_id, circle_name, space_number, memo, \
             is_checked, sort_order, tbf_circle_id, product_id, product_title, thumbnail_url, \
             thumbnail_data, price, is_purchased, first_appearance_event_name, \
             sample_fetch_attempted_at, createdAt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
             ON CONFLICT(id, owner_id) DO UPDATE SET
               event_id = excluded.event_id,
               circle_name = excluded.circle_name,
               space_number = excluded.space_number,
               memo = excluded.memo,
               is_checked = checked_items.is_checked,
               sort_order = excluded.sort_order,
               tbf_circle_id = excluded.tbf_circle_id,
               product_id = excluded.product_id,
               product_title = excluded.product_title,
               thumbnail_url = excluded.thumbnail_url,
               thumbnail_data = COALESCE(excluded.thumbnail_data, checked_items.thumbnail_data),
               price = excluded.price,
               is_purchased = excluded.is_purchased,
               first_appearance_event_name = excluded.first_appearance_event_name,
               sample_fetch_attempted_at = excluded.sample_fetch_attempted_at",
        )
        .bind(&item.id)
        .bind(owner_id)
        .bind(&item.event_id)
        .bind(&item.circle_name)
        .bind(&item.space_number)
        .bind(&item.memo)
        .bind(item.is_checked)
        .bind(item.sort_order)
        .bind(&item.tbf_circle_id)
        .bind(&item.product_id)
        .bind(&item.product_title)
        .bind(&item.thumbnail_url)
        .bind(&item.thumbnail_data)
        .bind(item.price)
        .bind(item.is_purchased)
        .bind(&item.first_appearance_event_name)
        .bind(&item.sample_fetch_attempted_at)
        .bind(&item.created_at)
        .execute(pool)
        .await?;
        Ok(())
    })
}

/// 未所属（`owner_sub IS NULL`）の行に所有者（暗号化済み sub）を与える。
///
/// 本棚（`bookshelf::attribute_owner`）と同じ帰属規則。既に所有者が付いている
/// 行は書き換えない。戻り値は更新した行数。
pub fn attribute_owner(
    pool: &SqlitePool,
    event_id: &str,
    owner: Option<&str>,
) -> Result<u64, sqlx::Error> {
    attribute_owner_with_id(pool, event_id, owner, None)
}

/// owner_sub と owner_id を同時に付ける。同じ id の現所有者行が既にある場合は
/// owner_id を移さない（PK 衝突を避け、旧行は未所属のまま非表示に残す）。
pub fn attribute_owner_with_id(
    pool: &SqlitePool,
    event_id: &str,
    owner: Option<&str>,
    owner_id: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let Some(owner) = owner else {
        return Ok(0);
    };
    crate::db::block_on(async {
        let result = sqlx::query(
            "UPDATE checked_items AS old SET owner_sub = ?1, \
               owner_id = CASE \
                 WHEN ?2 <> '' AND old.owner_id = '' AND NOT EXISTS ( \
                   SELECT 1 FROM checked_items AS current \
                   WHERE current.id = old.id AND current.owner_id = ?2 \
                 ) THEN ?2 ELSE old.owner_id END \
             WHERE old.event_id = ?3 AND old.owner_sub IS NULL",
        )
        .bind(owner)
        .bind(owner_id.unwrap_or(""))
        .bind(event_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    })
}

/// 既存行の `owner_sub`（暗号文）から `owner_id` を埋める。
///
/// `checked_items` の後、試し読みページの `owner_id` を親に合わせる。FK を壊さない
/// よう、コミット時まで検査を遅らせる。復号できない行は空のまま残す。
pub fn backfill_owner_ids(pool: &SqlitePool, key: &[u8; 32]) -> Result<u64, sqlx::Error> {
    crate::db::block_on(async {
        let mut tx = pool.begin().await?;
        sqlx::query("PRAGMA defer_foreign_keys = ON")
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(
            "SELECT id, owner_sub FROM checked_items \
             WHERE owner_id = '' AND owner_sub IS NOT NULL",
        )
        .fetch_all(&mut *tx)
        .await?;
        let mut updated = 0u64;
        for row in rows {
            let id: String = row.try_get("id")?;
            let stored: Option<String> = row.try_get("owner_sub")?;
            let Some(sub) = stored
                .as_deref()
                .and_then(|value| crate::owner::decrypt(key, value))
            else {
                continue;
            };
            let owner_id = nartik::derive_owner_id(&sub);
            let affected = sqlx::query(
                "UPDATE checked_items SET owner_id = ?1 \
                 WHERE id = ?2 AND owner_id = '' \
                   AND NOT EXISTS ( \
                     SELECT 1 FROM checked_items AS current \
                     WHERE current.id = ?2 AND current.owner_id = ?1 \
                   )",
            )
            .bind(&owner_id)
            .bind(&id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            updated += affected;
            if affected > 0 {
                updated += sqlx::query(
                    "UPDATE product_sample_pages SET owner_id = ?1 \
                     WHERE checklist_item_id = ?2 AND owner_id = ''",
                )
                .bind(&owner_id)
                .bind(&id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            }
        }
        tx.commit().await?;
        Ok(updated)
    })
}

pub fn list_items(pool: &SqlitePool, event_id: &str) -> Result<Vec<CheckedItem>, sqlx::Error> {
    list_items_for_owner(pool, event_id, "")
}

/// 所有者を明示してイベントのチェックリストを返す。
pub fn list_items_for_owner(
    pool: &SqlitePool,
    event_id: &str,
    owner_id: &str,
) -> Result<Vec<CheckedItem>, sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query_as::<_, CheckedItem>(
            "SELECT id, event_id, circle_name, space_number, memo, is_checked, sort_order, \
             tbf_circle_id, product_id, product_title, thumbnail_url, thumbnail_data, price, \
             is_purchased, first_appearance_event_name, sample_fetch_attempted_at, createdAt \
             FROM checked_items WHERE event_id = ?1 AND owner_id = ?2 \
             ORDER BY sort_order, circle_name",
        )
        .bind(event_id)
        .bind(owner_id)
        .fetch_all(pool)
        .await
    })
}

pub fn set_checked(pool: &SqlitePool, item_id: &str, checked: bool) -> Result<(), sqlx::Error> {
    set_checked_for_owner(pool, item_id, "", checked)
}

/// 所有者を明示してチェック状態を更新する。
pub fn set_checked_for_owner(
    pool: &SqlitePool,
    item_id: &str,
    owner_id: &str,
    checked: bool,
) -> Result<(), sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query("UPDATE checked_items SET is_checked = ?1 WHERE id = ?2 AND owner_id = ?3")
            .bind(checked as i64)
            .bind(item_id)
            .bind(owner_id)
            .execute(pool)
            .await?;
        Ok(())
    })
}

/// 指定した id のサムネイル（base64 の 256px JPEG）。
///
/// 表紙バンドル（`crates/core/src/thumbs.rs`）が、まだ派生キャッシュへ入れて
/// いない項目の分だけを読むために使う。`thumbnail_data` は 1 件 20KB 程度あるので
/// **全件は読まない**（`json_each` で id を渡して絞る）。
pub fn thumbnails_of(
    pool: &SqlitePool,
    ids: &[String],
) -> Result<Vec<(String, String)>, sqlx::Error> {
    thumbnails_of_for_owner(pool, "", ids)
}

/// 所有者を明示して、指定 id のサムネイルを返す。
pub fn thumbnails_of_for_owner(
    pool: &SqlitePool,
    owner_id: &str,
    ids: &[String],
) -> Result<Vec<(String, String)>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let json = serde_json::to_string(ids).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    crate::db::block_on(async {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT id, thumbnail_data FROM checked_items \
             WHERE owner_id = ?1 AND thumbnail_data IS NOT NULL \
               AND id IN (SELECT value FROM json_each(?2))",
        )
        .bind(owner_id)
        .bind(json)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    })
}

/// サムネイル未保存（`thumbnail_data` が空）で、取得先 URL がある項目。
pub fn missing_thumbnail_items(
    pool: &SqlitePool,
    limit: usize,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    missing_thumbnail_items_for_owner(pool, "", limit)
}

/// 所有者を明示して、未取得サムネイルの (id, url) を返す。
pub fn missing_thumbnail_items_for_owner(
    pool: &SqlitePool,
    owner_id: &str,
    limit: usize,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query_as::<_, (String, String)>(
            "SELECT id, thumbnail_url FROM checked_items \
             WHERE owner_id = ?1 AND (thumbnail_data IS NULL OR thumbnail_data = '') \
               AND thumbnail_url IS NOT NULL AND thumbnail_url <> '' \
             ORDER BY event_id, sort_order, circle_name \
             LIMIT ?2",
        )
        .bind(owner_id)
        .bind(limit as i64)
        .fetch_all(pool)
        .await
    })
}

/// チェックリスト項目のサムネイル（base64 データ）を保存する。
pub fn update_thumbnail_data(
    pool: &SqlitePool,
    item_id: &str,
    thumbnail_data: &str,
) -> Result<(), sqlx::Error> {
    update_thumbnail_data_for_owner(pool, item_id, "", thumbnail_data)
}

/// 所有者を明示してサムネイルを保存する。
pub fn update_thumbnail_data_for_owner(
    pool: &SqlitePool,
    item_id: &str,
    owner_id: &str,
    thumbnail_data: &str,
) -> Result<(), sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query(
            "UPDATE checked_items SET thumbnail_data = ?1 WHERE id = ?2 AND owner_id = ?3",
        )
        .bind(thumbnail_data)
        .bind(item_id)
        .bind(owner_id)
        .execute(pool)
        .await?;
        Ok(())
    })
}

/// チェックリスト項目を削除する。
pub fn delete_item(pool: &SqlitePool, item_id: &str) -> Result<(), sqlx::Error> {
    delete_item_for_owner(pool, item_id, "")
}

/// 所有者を明示して削除する。
pub fn delete_item_for_owner(
    pool: &SqlitePool,
    item_id: &str,
    owner_id: &str,
) -> Result<(), sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query("DELETE FROM checked_items WHERE id = ?1 AND owner_id = ?2")
            .bind(item_id)
            .bind(owner_id)
            .execute(pool)
            .await?;
        Ok(())
    })
}

pub fn get_item(pool: &SqlitePool, item_id: &str) -> Result<Option<CheckedItem>, sqlx::Error> {
    get_item_for_owner(pool, item_id, "")
}

/// 所有者を明示して 1 件取得する。
pub fn get_item_for_owner(
    pool: &SqlitePool,
    item_id: &str,
    owner_id: &str,
) -> Result<Option<CheckedItem>, sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query_as::<_, CheckedItem>(
            "SELECT id, event_id, circle_name, space_number, memo, is_checked, sort_order, \
             tbf_circle_id, product_id, product_title, thumbnail_url, thumbnail_data, price, \
             is_purchased, first_appearance_event_name, sample_fetch_attempted_at, createdAt \
             FROM checked_items WHERE id = ?1 AND owner_id = ?2",
        )
        .bind(item_id)
        .bind(owner_id)
        .fetch_optional(pool)
        .await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tbf::SITE_ID_TECHBOOKFEST;

    fn event(slug: &str) -> TbfEvent {
        let ts = "2026-01-01 00:00:00".to_string();
        TbfEvent {
            id: slug.to_string(),
            site_id: SITE_ID_TECHBOOKFEST.into(),
            slug: Some(slug.to_string()),
            tbf_event_id: Some(format!("Event:{slug}")),
            event_name: format!("イベント{slug}"),
            event_date: None,
            event_start_date: None,
            event_end_date: None,
            event_format: "offline".to_string(),
            is_cancelled: 0,
            display_order: 0,
            is_featured: 0,
            created_at: ts.clone(),
            updated_at: ts,
            poll_sync_enabled: 0,
        }
    }

    #[test]
    fn set_poll_enabled_saves() {
        let pool = crate::db::test_pool();
        upsert_event(&pool, &event("tbf30")).unwrap();
        set_poll_enabled(&pool, "tbf30", true).unwrap();
        let events = list_events(&pool).unwrap();
        assert_eq!(
            events
                .iter()
                .find(|e| e.slug.as_deref() == Some("tbf30"))
                .unwrap()
                .poll_sync_enabled,
            1
        );
        set_poll_enabled(&pool, "tbf30", false).unwrap();
        let events = list_events(&pool).unwrap();
        assert_eq!(
            events
                .iter()
                .find(|e| e.slug.as_deref() == Some("tbf30"))
                .unwrap()
                .poll_sync_enabled,
            0
        );
    }

    #[test]
    fn list_enabled_slugs_returns_only_enabled() {
        let pool = crate::db::test_pool();
        upsert_event(&pool, &event("tbf30")).unwrap();
        upsert_event(&pool, &event("tbf29")).unwrap();
        set_poll_enabled(&pool, "tbf30", true).unwrap();
        assert_eq!(
            list_enabled_slugs(&pool).unwrap(),
            vec!["tbf30".to_string()]
        );
    }

    #[test]
    fn upsert_event_preserves_poll_enabled() {
        let pool = crate::db::test_pool();
        upsert_event(&pool, &event("tbf30")).unwrap();
        set_poll_enabled(&pool, "tbf30", true).unwrap();
        // 再 upsert（poll_sync_enabled = 0 で来る）してもトグルは保持される
        let fresh = TbfEvent {
            poll_sync_enabled: 0,
            ..event("tbf30")
        };
        upsert_event(&pool, &fresh).unwrap();
        let events = list_events(&pool).unwrap();
        assert_eq!(
            events
                .iter()
                .find(|e| e.slug.as_deref() == Some("tbf30"))
                .unwrap()
                .poll_sync_enabled,
            1
        );
    }

    #[test]
    fn upsert_item_preserves_downloaded_thumbnail_data() {
        let pool = crate::db::test_pool();
        upsert_event(&pool, &event("tbf20")).unwrap();
        let item = CheckedItem {
            id: "tbf20:p1".into(),
            event_id: "tbf20".into(),
            circle_name: "サークル".into(),
            space_number: "あ-01".into(),
            memo: String::new(),
            is_checked: 0,
            sort_order: 0,
            tbf_circle_id: None,
            product_id: Some("p1".into()),
            product_title: "本".into(),
            thumbnail_url: Some("https://example.com/img.png".into()),
            thumbnail_data: Some("aGVsbG8=".into()),
            price: None,
            is_purchased: 0,
            first_appearance_event_name: None,
            sample_fetch_attempted_at: None,
            created_at: "2026-01-01 00:00:00".into(),
        };
        upsert_item(&pool, &item).unwrap();
        // 同期は thumbnail_data を持たない（None）で来る。既存の画像を消さない。
        let fresh = CheckedItem {
            thumbnail_data: None,
            thumbnail_url: Some("https://example.com/img2.png".into()),
            ..item
        };
        upsert_item(&pool, &fresh).unwrap();
        let stored = get_item(&pool, "tbf20:p1").unwrap().unwrap();
        assert_eq!(stored.thumbnail_data.as_deref(), Some("aGVsbG8="));
        assert_eq!(
            stored.thumbnail_url.as_deref(),
            Some("https://example.com/img2.png")
        );
    }

    /// 同じ技術書典の項目 ID を 2 アカウントが持てる（PK は id + owner_id）。
    #[test]
    fn the_same_checklist_item_id_coexists_per_owner() {
        let pool = crate::db::test_pool();
        upsert_event(&pool, &event("tbf21")).unwrap();
        let owner_a = nartik::derive_owner_id("A");
        let owner_b = nartik::derive_owner_id("B");
        let item = CheckedItem {
            id: "tbf21:same".into(),
            event_id: "tbf21".into(),
            circle_name: "サークル".into(),
            space_number: "あ-01".into(),
            memo: String::new(),
            is_checked: 0,
            sort_order: 0,
            tbf_circle_id: None,
            product_id: Some("p1".into()),
            product_title: "本".into(),
            thumbnail_url: None,
            thumbnail_data: None,
            price: None,
            is_purchased: 0,
            first_appearance_event_name: None,
            sample_fetch_attempted_at: None,
            created_at: "2026-01-01 00:00:00".into(),
        };
        upsert_item_for_owner(&pool, &item, &owner_a).unwrap();
        upsert_item_for_owner(&pool, &item, &owner_b).unwrap();

        assert_eq!(list_items_for_owner(&pool, "tbf21", &owner_a).unwrap().len(), 1);
        assert_eq!(list_items_for_owner(&pool, "tbf21", &owner_b).unwrap().len(), 1);
        set_checked_for_owner(&pool, "tbf21:same", &owner_a, true).unwrap();
        assert_eq!(
            get_item_for_owner(&pool, "tbf21:same", &owner_a)
                .unwrap()
                .unwrap()
                .is_checked,
            1
        );
        assert_eq!(
            get_item_for_owner(&pool, "tbf21:same", &owner_b)
                .unwrap()
                .unwrap()
                .is_checked,
            0
        );

        // 試し読みも (checklist_item_id, owner_id) で分かれる。
        for (id, owner) in [("sample-a", &owner_a), ("sample-b", &owner_b)] {
            crate::db::samples::insert_sample_page_for_owner(
                &pool,
                &crate::db::samples::SamplePageRow {
                    id: id.into(),
                    checklist_item_id: "tbf21:same".into(),
                    product_id: Some("p1".into()),
                    page_number: 1,
                    image_url: None,
                    image_data: Some("eA==".into()),
                    mime_type: "image/jpeg".into(),
                    width: Some(1),
                    height: Some(1),
                    file_size: Some(1),
                    fetched_at: "2026-01-01T00:00:00Z".into(),
                },
                owner,
            )
            .unwrap();
        }
        assert_eq!(
            crate::db::samples::list_for_item_for_owner(&pool, "tbf21:same", &owner_a)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::db::samples::list_for_item_for_owner(&pool, "tbf21:same", &owner_b)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn migrate_creates_poll_sync_enabled_column_default_zero() {
        let pool = crate::db::test_pool();
        // migrate() 適用後の tbf_events に、default 0 の poll_sync_enabled 列が存在する
        let dflt: Option<String> = crate::db::block_on(async {
            sqlx::query_scalar::<_, String>(
                "SELECT dflt_value FROM pragma_table_info('tbf_events') WHERE name = 'poll_sync_enabled'",
            )
            .fetch_optional(&pool)
            .await
        })
        .unwrap();
        assert_eq!(dflt.as_deref(), Some("0"));
    }
}
