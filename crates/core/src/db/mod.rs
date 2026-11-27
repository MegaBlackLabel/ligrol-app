//! SQLite storage: connection handling, sqlx migrations and repository
//! layers (books / bookshelf / checklist / progress / tags / ...).
//!
//! The app talks to the DB synchronously (blocking) from GPUI background
//! tasks; sqlx is async-only, so every repository function wraps its work in
//! [`block_on`] against a process-wide tokio runtime.

pub mod ai_jobs;
pub mod backup;
pub mod books;
pub mod bookshelf;
pub mod checklist;
pub mod column_crypto;
pub mod contents;
pub mod documents;
pub mod favorites;
pub mod notes;
pub mod page_views;
pub mod progress;
pub mod samples;
pub mod settings;
pub mod sync_state;
pub mod tags;
pub mod thumbs;
pub mod view_history;

use std::path::Path;

use sqlx::Acquire as _;
use sqlx::SqliteConnection;
pub use sqlx::sqlite::SqlitePool;

/// Process-wide tokio runtime for bridging sqlx (async) and the synchronous
/// repository API.
static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime")
    })
}

/// Run a sqlx future to completion on the process-wide tokio runtime.
pub fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    runtime().block_on(fut)
}

/// 内部で組み立てた SQL を sqlx の [`sqlx::AssertSqlSafe`] で包む。
///
/// 補間してよいのはテーブル名・列名・プレースホルダだけで、いずれも内部の定数か
/// 既存スキーマ（`PRAGMA table_info`）から得た識別子に限る。**値は必ず `.bind()`**。
/// sqlx 0.9 は動的 SQL を型で拒否するため、この関数が唯一の通過点になる。
pub(crate) fn dynamic_sql(sql: impl Into<String>) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(sql.into())
}

/// AI / MCP 用の接続。**マイグレーションを実行しない**。
///
/// アプリが作ったスキーマをそのまま使う。読み取りと `ai_jobs` への enqueue だけを
/// 想定し、スキーマが古い場合はクエリ側でエラーになる（勝手に ALTER しない）。
pub fn connect_for_ai(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .create_if_missing(false);
    block_on(SqlitePool::connect_with(options))
}

/// Open a pooled connection with foreign keys enabled and apply pending
/// migrations (`migrations/` directory, managed via `_sqlx_migrations`).
pub fn connect(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .create_if_missing(true);
    let pool = block_on(SqlitePool::connect_with(options))?;
    migrate(&pool)?;
    Ok(pool)
}

/// 開発中のためマイグレーションファイルは作らず、既存の runtime DDL（hidden_at /
/// owner_sub 等）と同様に PRAGMA で列の有無を確認してから `ALTER TABLE ... ADD COLUMN` で
/// 冪等に列を追加する。
/// 既存の `bookshelf_items` / `book_first_events` を owner 込みの PK へ作り替える。
///
/// SQLite は PK を ALTER できないため、旧テーブルを rename して新テーブルへ
/// `INSERT ... SELECT` する。`owner_id` は既存行では空文字（未バックフィル）にし、
/// 起動後に DB 鍵で `owner_sub` から埋める。`thumbnail_share` は派生キャッシュなので
/// 旧スキーマを捨てて作り直す。
async fn migrate_owner_aware_shelf(conn: &mut sqlx::SqliteConnection) -> Result<(), sqlx::Error> {
    let has_owner_id = has_column(&mut *conn, "bookshelf_items", "owner_id").await?;
    let has_first_owner_id = has_column(&mut *conn, "book_first_events", "owner_id").await?;
    if has_owner_id && has_first_owner_id {
        return Ok(());
    }

    let fk_enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *conn)
        .await?;
    if fk_enabled != 0 {
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await?;
    }

    let result = async {
        let mut tx = conn.begin().await?;
        if !has_owner_id {
            sqlx::query("ALTER TABLE bookshelf_items RENAME TO bookshelf_items_old")
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "CREATE TABLE bookshelf_items (
                  site_id TEXT NOT NULL REFERENCES sites(id),
                  database_id TEXT NOT NULL,
                  title TEXT NOT NULL,
                  circle_name TEXT NOT NULL DEFAULT '',
                  thumbnail_url TEXT,
                  format TEXT NOT NULL DEFAULT '',
                  causedAt TEXT,
                  event_name TEXT,
                  event_slug TEXT,
                  event_id TEXT REFERENCES tbf_events(id),
                  file_name TEXT,
                  download_url TEXT,
                  is_downloadable INTEGER NOT NULL DEFAULT 0,
                  is_checked INTEGER NOT NULL DEFAULT 0,
                  is_purchased INTEGER NOT NULL DEFAULT 0,
                  is_new INTEGER NOT NULL DEFAULT 0,
                  is_active INTEGER NOT NULL DEFAULT 1,
                  is_favorite INTEGER NOT NULL DEFAULT 0,
                  is_hidden INTEGER NOT NULL DEFAULT 0,
                  tags_json TEXT,
                  synced_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                  hidden_at TEXT,
                  author TEXT NOT NULL DEFAULT '',
                  tags_fetched INTEGER NOT NULL DEFAULT 0,
                  download_options TEXT,
                  media_category TEXT,
                  ai_type TEXT,
                  is_drm INTEGER NOT NULL DEFAULT 2,
                  release_date TEXT,
                  description TEXT,
                  theme TEXT,
                  maker_id TEXT,
                  page_count INTEGER,
                  age_rating TEXT,
                  series_name TEXT,
                  owner_sub TEXT,
                  owner_id TEXT NOT NULL DEFAULT '',
                  PRIMARY KEY (site_id, database_id, owner_id)
                )",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO bookshelf_items (
                  site_id, database_id, title, circle_name, thumbnail_url, format, causedAt,
                  event_name, event_slug, event_id, file_name, download_url, is_downloadable,
                  is_checked, is_purchased, is_new, is_active, is_favorite, is_hidden, tags_json,
                  synced_at, created_at, updated_at, hidden_at, author, tags_fetched,
                  download_options, media_category, ai_type, is_drm, release_date, description,
                  theme, maker_id, page_count, age_rating, series_name, owner_sub
                ) SELECT
                  site_id, database_id, title, circle_name, thumbnail_url, format, causedAt,
                  event_name, event_slug, event_id, file_name, download_url, is_downloadable,
                  is_checked, is_purchased, is_new, is_active, is_favorite, is_hidden, tags_json,
                  synced_at, created_at, updated_at, hidden_at, author, tags_fetched,
                  download_options, media_category, ai_type, is_drm, release_date, description,
                  theme, maker_id, page_count, age_rating, series_name, owner_sub
                FROM bookshelf_items_old",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query("DROP TABLE bookshelf_items_old")
                .execute(&mut *tx)
                .await?;
        }
        if !has_first_owner_id {
            sqlx::query("ALTER TABLE book_first_events RENAME TO book_first_events_old")
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "CREATE TABLE book_first_events (
                  site_id TEXT NOT NULL,
                  database_id TEXT NOT NULL,
                  first_event_name TEXT,
                  first_event_slug TEXT,
                  fetched_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                  owner_sub TEXT,
                  owner_id TEXT NOT NULL DEFAULT '',
                  PRIMARY KEY (site_id, database_id, owner_id),
                  FOREIGN KEY (site_id, database_id, owner_id)
                    REFERENCES bookshelf_items(site_id, database_id, owner_id)
                )",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO book_first_events (
                  site_id, database_id, first_event_name, first_event_slug, fetched_at, owner_sub
                ) SELECT site_id, database_id, first_event_name, first_event_slug, fetched_at, owner_sub
                  FROM book_first_events_old",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query("DROP TABLE book_first_events_old")
                .execute(&mut *tx)
                .await?;
        }
        // 派生キャッシュ。旧 owner 無しの行はどこにも帰属できないので捨てる。
        sqlx::query("DROP TABLE IF EXISTS thumbnail_share")
            .execute(&mut *tx)
            .await?;
        // rename / drop で旧テーブルの索引も消えるため作り直す。
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS bookshelf_items_site_id_idx \
             ON bookshelf_items(site_id)",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS bookshelf_items_event_id_idx \
             ON bookshelf_items(event_id)",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS bookshelf_items_owner_id_idx \
             ON bookshelf_items(owner_id)",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    }
    .await;

    if fk_enabled != 0 {
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *conn)
            .await?;
    }
    result
}

/// 既存の `checked_items` / `product_sample_pages` を owner 込みの PK へ作り替える。
///
/// 技術書典の項目 ID はアカウント間で同じため、`id` 単独の PK では 2 アカウント目が
/// 同じ項目を持てない。`checked_items` は `(id, owner_id)`、試し読みは
/// `(checklist_item_id, owner_id)` の複合 FK にする。
async fn migrate_owner_aware_checklist(
    conn: &mut sqlx::SqliteConnection,
) -> Result<(), sqlx::Error> {
    let has_item_owner = has_column(&mut *conn, "checked_items", "owner_id").await?;
    let has_sample_owner = has_column(&mut *conn, "product_sample_pages", "owner_id").await?;
    if has_item_owner && has_sample_owner {
        return Ok(());
    }

    let fk_enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *conn)
        .await?;
    if fk_enabled != 0 {
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await?;
    }

    let result = async {
        let mut tx = conn.begin().await?;
        if !has_item_owner {
            sqlx::query("ALTER TABLE checked_items RENAME TO checked_items_old")
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "CREATE TABLE checked_items (
                  id TEXT NOT NULL,
                  event_id TEXT NOT NULL REFERENCES tbf_events(id),
                  circle_name TEXT NOT NULL,
                  space_number TEXT NOT NULL DEFAULT '',
                  memo TEXT NOT NULL DEFAULT '',
                  is_checked INTEGER NOT NULL DEFAULT 0,
                  sort_order INTEGER NOT NULL DEFAULT 0,
                  tbf_circle_id TEXT,
                  product_id TEXT,
                  product_title TEXT NOT NULL DEFAULT '',
                  thumbnail_url TEXT,
                  thumbnail_data TEXT,
                  price INTEGER,
                  is_purchased INTEGER NOT NULL DEFAULT 0,
                  sample_fetch_attempted_at TEXT,
                  createdAt TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                  first_appearance_event_name TEXT,
                  owner_sub TEXT,
                  owner_id TEXT NOT NULL DEFAULT '',
                  PRIMARY KEY (id, owner_id)
                )",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO checked_items (
                  id, event_id, circle_name, space_number, memo, is_checked, sort_order,
                  tbf_circle_id, product_id, product_title, thumbnail_url, thumbnail_data,
                  price, is_purchased, sample_fetch_attempted_at, createdAt,
                  first_appearance_event_name, owner_sub
                ) SELECT
                  id, event_id, circle_name, space_number, memo, is_checked, sort_order,
                  tbf_circle_id, product_id, product_title, thumbnail_url, thumbnail_data,
                  price, is_purchased, sample_fetch_attempted_at, createdAt,
                  first_appearance_event_name, owner_sub
                FROM checked_items_old",
            )
            .execute(&mut *tx)
            .await?;
        }

        if !has_sample_owner {
            sqlx::query("ALTER TABLE product_sample_pages RENAME TO product_sample_pages_old")
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "CREATE TABLE product_sample_pages (
                  id TEXT PRIMARY KEY,
                  checklist_item_id TEXT NOT NULL,
                  owner_id TEXT NOT NULL DEFAULT '',
                  product_id TEXT,
                  page_number INTEGER NOT NULL,
                  image_url TEXT,
                  image_data TEXT,
                  mime_type TEXT NOT NULL DEFAULT 'image/jpeg',
                  width INTEGER,
                  height INTEGER,
                  file_size INTEGER,
                  pack_id TEXT,
                  pack_entry_path TEXT,
                  fetched_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                  FOREIGN KEY (checklist_item_id, owner_id)
                    REFERENCES checked_items(id, owner_id)
                )",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO product_sample_pages (
                  id, checklist_item_id, owner_id, product_id, page_number, image_url,
                  image_data, mime_type, width, height, file_size, pack_id, pack_entry_path,
                  fetched_at, created_at
                ) SELECT
                  p.id, p.checklist_item_id, COALESCE(c.owner_id, ''),
                  p.product_id, p.page_number, p.image_url, p.image_data, p.mime_type,
                  p.width, p.height, p.file_size, p.pack_id, p.pack_entry_path,
                  p.fetched_at, p.created_at
                FROM product_sample_pages_old p
                LEFT JOIN checked_items c ON c.id = p.checklist_item_id",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query("DROP TABLE product_sample_pages_old")
                .execute(&mut *tx)
                .await?;
        }

        if !has_item_owner {
            sqlx::query("DROP TABLE checked_items_old")
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS checked_items_owner_id_idx \
             ON checked_items(owner_id, event_id)",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok::<(), sqlx::Error>(())
    }
    .await;

    if fk_enabled != 0 {
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *conn)
            .await?;
    }
    result
}

async fn ensure_column(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2")
            .bind(table)
            .bind(column)
            .fetch_one(&mut *conn)
            .await?;
    if exists == 0 {
        sqlx::query(dynamic_sql(format!(
            "ALTER TABLE {table} ADD COLUMN {definition}"
        )))
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// テーブルに列があるか（`PRAGMA table_info`）。
async fn has_column(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    column: &str,
) -> Result<bool, sqlx::Error> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2")
            .bind(table)
            .bind(column)
            .fetch_one(&mut *conn)
            .await?;
    Ok(count > 0)
}

/// `reading_progress` / `page_views` を**コンテンツ単位**に作り替えるデータ移行。
///
/// PK 変更は `ALTER TABLE` でできないため、新テーブルへ `INSERT ... SELECT` して
/// 入れ替える。既存行の `content_id` は「その本の優先コンテンツ」、無ければ `''`
/// （旧データ / 単一コンテンツ）。`content_id` 列の有無で判定するので何度でも安全。
pub(crate) async fn migrate_progress_content_id(
    conn: &mut sqlx::SqliteConnection,
) -> Result<u64, sqlx::Error> {
    let mut moved = 0u64;

    if !has_column(&mut *conn, "reading_progress", "content_id").await? {
        sqlx::query("ALTER TABLE reading_progress RENAME TO reading_progress_old")
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "CREATE TABLE reading_progress (               book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,               content_id TEXT NOT NULL DEFAULT '',               current_page INTEGER NOT NULL DEFAULT 0,               total_pages INTEGER,               finished_at TEXT,               last_read_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,               scroll_position REAL NOT NULL DEFAULT 0,               PRIMARY KEY (book_id, content_id)             )",
        )
        .execute(&mut *conn)
        .await?;
        moved += sqlx::query(
            "INSERT INTO reading_progress (book_id, content_id, current_page, total_pages, \
             finished_at, last_read_at, scroll_position) \
             SELECT old.book_id, \
                    COALESCE((SELECT content_id FROM book_contents WHERE book_id = old.book_id \
                              ORDER BY is_primary DESC, sort_order, content_id LIMIT 1), ''), \
                    old.current_page, old.total_pages, old.finished_at, old.last_read_at, \
                    old.scroll_position \
             FROM reading_progress_old old",
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();
        sqlx::query("DROP TABLE reading_progress_old")
            .execute(&mut *conn)
            .await?;
    }

    if !has_column(&mut *conn, "page_views", "content_id").await? {
        sqlx::query("ALTER TABLE page_views RENAME TO page_views_old")
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "CREATE TABLE page_views (               book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,               content_id TEXT NOT NULL DEFAULT '',               page_number INTEGER NOT NULL,               view_count INTEGER NOT NULL DEFAULT 0,               total_seconds REAL NOT NULL DEFAULT 0,               last_viewed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,               PRIMARY KEY (book_id, content_id, page_number)             )",
        )
        .execute(&mut *conn)
        .await?;
        moved += sqlx::query(
            "INSERT INTO page_views (book_id, content_id, page_number, view_count, total_seconds, \
             last_viewed_at) \
             SELECT old.book_id, \
                    COALESCE((SELECT content_id FROM book_contents WHERE book_id = old.book_id \
                              ORDER BY is_primary DESC, sort_order, content_id LIMIT 1), ''), \
                    old.page_number, old.view_count, old.total_seconds, old.last_viewed_at \
             FROM page_views_old old",
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();
        sqlx::query("DROP TABLE page_views_old")
            .execute(&mut *conn)
            .await?;
    }

    // ページテキスト・形態素解析にもコンテンツを紐づける（列追加のみ）
    ensure_column(
        &mut *conn,
        "document_text",
        "content_id",
        "content_id TEXT NOT NULL DEFAULT ''",
    )
    .await?;
    ensure_column(
        &mut *conn,
        "token_analysis",
        "content_id",
        "content_id TEXT NOT NULL DEFAULT ''",
    )
    .await?;

    Ok(moved)
}

/// テスト・ワンショット用の同期ラッパー。
pub fn run_progress_content_migration(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    block_on(async {
        let mut conn = pool.acquire().await?;
        migrate_progress_content_id(&mut conn).await
    })
}

/// Apply pending migrations. Idempotent; tracked in the `_sqlx_migrations`
/// table (independent of the legacy `user_version` marker).
pub fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    block_on(async {
        let mut conn = pool.acquire().await?;
        sqlx::migrate!("./migrations")
            .run(&mut conn)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        // 非表示日時カラム（プログラム的マイグレーション: SQLite の PRAGMA で
        // 存在確認してから ALTER TABLE する。新規 DB は schema.sql が含む）
        {
            let has_hidden_at: bool = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('bookshelf_items') \
                 WHERE name = 'hidden_at'",
            )
            .fetch_one(&mut *conn)
            .await?;
            if !has_hidden_at {
                sqlx::query("ALTER TABLE bookshelf_items ADD COLUMN hidden_at TEXT")
                    .execute(&mut *conn)
                    .await?;
            }
        }
        // 作者名カラム（プログラム的マイグレーション。hidden_at と同様に PRAGMA で
        // 存在確認してから ALTER TABLE する。技術書典は author を持たないため空のまま、
        // BOOTH 等は作成者（shop）に相当する作者名を格納する）
        {
            let has_author: bool = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('bookshelf_items') \
                 WHERE name = 'author'",
            )
            .fetch_one(&mut *conn)
            .await?;
            if !has_author {
                sqlx::query(
                    "ALTER TABLE bookshelf_items ADD COLUMN author TEXT NOT NULL DEFAULT ''",
                )
                .execute(&mut *conn)
                .await?;
            }
        }
        // Drive バックアップ対象外（大きな pack を上げない。α版なので
        // PRAGMA で存在確認してから ALTER TABLE する。migration ファイルは
        // checksum 管理なので触らない）。
        {
            let has_backup_excluded: bool = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('books') WHERE name = 'backup_excluded'",
            )
            .fetch_one(&mut *conn)
            .await?;
            if !has_backup_excluded {
                sqlx::query(
                    "ALTER TABLE books ADD COLUMN backup_excluded INTEGER NOT NULL DEFAULT 0",
                )
                .execute(&mut *conn)
                .await?;
            }
        }
        // 所有者 sub（暗号化済み / NULL = 未所属）。アカウント切替・複数アカウント対応
        // （改訂版）。既存の migration ファイルは checksum 管理されるため変更せず、
        // PRAGMA で存在確認してから ALTER TABLE する。
        {
            let has_owner_sub: bool = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('books') WHERE name = 'owner_sub'",
            )
            .fetch_one(&mut *conn)
            .await?;
            if !has_owner_sub {
                sqlx::query("ALTER TABLE books ADD COLUMN owner_sub TEXT")
                    .execute(&mut *conn)
                    .await?;
            }
        }
        // 複数アカウント対応: 同一 source を owner ごとに複数行持てるようにするため、
        // `(site_id, tbf_product_id)` の UNIQUE 制約を外す（乙案）。
        // 既存 DB / 新規 DB とも、ここで冪等に DROP する。
        sqlx::query("DROP INDEX IF EXISTS books_site_tbf_product_id_unique")
            .execute(&mut *conn)
            .await?;
        // チェックリストのポーリング有効フラグ（プログラム的マイグレーション。
        // hidden_at と同様に PRAGMA で存在確認してから ALTER TABLE する。
        // 既存の migration ファイルは checksum 管理されるため変更しない）
        {
            let has_poll_enabled: bool = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('tbf_events') \
                 WHERE name = 'poll_sync_enabled'",
            )
            .fetch_one(&mut *conn)
            .await?;
            if !has_poll_enabled {
                sqlx::query(
                    "ALTER TABLE tbf_events ADD COLUMN poll_sync_enabled INTEGER NOT NULL DEFAULT 0",
                )
                .execute(&mut *conn)
                .await?;
            }
        }
        // チェックリスト項目の初出イベント名（プログラム的マイグレーション）。
        // `checkedProductInfos` は対象イベントに出展しているサークルの商品を返すため、
        // 同じサークルが過去に出した本も混ざる。初出イベントで抽出できるように保存する。
        ensure_column(
            &mut conn,
            "checked_items",
            "first_appearance_event_name",
            "first_appearance_event_name TEXT",
        )
        .await?;
        // 本ごとの綴じ方向（プログラム的マイグレーション。NULL = サイト別設定
        // `viewer.page_turn.{site}` に従う。既存の migration ファイルは checksum
        // 管理されるため変更せず、PRAGMA で存在確認してから ALTER TABLE する）
        ensure_column(&mut conn, "books", "page_turn", "page_turn TEXT").await?;
        // 表紙タグの取得済みフラグ（プログラム的マイグレーション。未ダウンロード本の
        // タグを同期のたびに少しずつ取るため、「取得済み」を持って再取得を防ぐ。
        // 0 = 未取得。タグが 0 件でも 1 を立てる）
        ensure_column(
            &mut conn,
            "bookshelf_items",
            "tags_fetched",
            "tags_fetched INTEGER NOT NULL DEFAULT 0",
        )
        .await?;
        // ダウンロード候補（BOOTH の「1 商品に複数のダウンロード（PDF + 画像 ZIP 等）」）。
        // JSON 配列 `[{"name":...,"url":...}]`。Web スキーマに無い列なので、
        // hidden_at / tags_fetched と同じく PRAGMA で確認してから追加する。
        // NULL = 候補なし（`download_url` 1 件として扱う）。
        ensure_column(
            &mut conn,
            "bookshelf_items",
            "download_options",
            "download_options TEXT",
        )
        .await?;
        // 閲覧履歴（プログラム的マイグレーション。既存の migration ファイルは
        // checksum 管理されるため変更せず、IF NOT EXISTS で冪等に適用する）
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS view_history (               id TEXT PRIMARY KEY,               book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,               started_at TEXT NOT NULL,               ended_at TEXT             );             CREATE INDEX IF NOT EXISTS idx_view_history_book ON view_history(book_id)",
        )
        .execute(&mut *conn)
        .await?;
        // ページ毎の閲覧記録（プログラム的マイグレーション。view_history と同様に
        // マイグレーションファイルは checksum 管理されるため変更せず、IF NOT EXISTS で
        // 冪等に適用する。1 冊 × 1 ページの累計表示回数・累計滞在秒数を持つ集計表）
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS page_views (               book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,               content_id TEXT NOT NULL DEFAULT '',               page_number INTEGER NOT NULL,               view_count INTEGER NOT NULL DEFAULT 0,               total_seconds REAL NOT NULL DEFAULT 0,               last_viewed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,               PRIMARY KEY (book_id, content_id, page_number)             );             CREATE INDEX IF NOT EXISTS idx_page_views_book ON page_views(book_id)",
        )
        .execute(&mut *conn)
        .await?;
        // 付箋（ページ単位のメモ）。開発中のためマイグレーションファイルは作らず、
        // 他の後発テーブルと同様に IF NOT EXISTS で冪等に適用する。
        // 1 ページ 1 件（book_id + content_id + page で一意）。spread_side は付けたときの
        // 見開きの左右（単一表示は NULL）で、「本を見る」から同じ側に開くために使う。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS page_notes (               id TEXT PRIMARY KEY,               book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,               content_id TEXT NOT NULL DEFAULT '',               page INTEGER NOT NULL,               memo TEXT NOT NULL DEFAULT '',               spread_side TEXT,               is_active INTEGER NOT NULL DEFAULT 1,               created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,               updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,               UNIQUE (book_id, content_id, page)             );             CREATE INDEX IF NOT EXISTS idx_page_notes_book ON page_notes(book_id)",
        )
        .execute(&mut *conn)
        .await?;
        // 既に page_notes がある DB 向け: 付箋の ON / OFF 列を足す（IF NOT EXISTS では
        // 列は増えないため、pragma で確認してから ALTER する）
        {
            let has_is_active: bool = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('page_notes') WHERE name = 'is_active'",
            )
            .fetch_one(&mut *conn)
            .await?;
            if !has_is_active {
                sqlx::query(
                    "ALTER TABLE page_notes ADD COLUMN is_active INTEGER NOT NULL DEFAULT 1",
                )
                .execute(&mut *conn)
                .await?;
            }
        }
        // サークル / 作者のお気に入り（タグのお気に入り `favorite_tags` と同じ
        // チップのハート）。開発中のためマイグレーションファイルは作らず、
        // 他の後発テーブルと同様に IF NOT EXISTS で冪等に適用する。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS favorite_entities (               entity_kind TEXT NOT NULL,               entity_name TEXT NOT NULL,               created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,               PRIMARY KEY (entity_kind, entity_name)             )",
        )
        .execute(&mut *conn)
        .await?;
        // コンテンツ（読む単位）とレンディション（切替可能な表示形態）。1 冊に複数の
        // 本文・別冊・PDF版/画像版を持たせるための構造（docs/import-patterns.md §3.3）。
        // 開発中のためマイグレーションファイルは作らず、他の後発テーブルと同様に
        // IF NOT EXISTS で冪等に適用する。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS book_contents (               content_id TEXT PRIMARY KEY,               book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,               display_name TEXT NOT NULL,               media_kind TEXT NOT NULL,               is_primary INTEGER NOT NULL DEFAULT 0,               sort_order INTEGER NOT NULL DEFAULT 0,               created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP             );             CREATE INDEX IF NOT EXISTS idx_book_contents_book ON book_contents(book_id)",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS content_formats (               format_id TEXT PRIMARY KEY,               content_id TEXT NOT NULL REFERENCES book_contents(content_id) ON DELETE CASCADE,               label TEXT NOT NULL,               format_kind TEXT NOT NULL,               page_count INTEGER NOT NULL DEFAULT 0,               pack_entry_prefix TEXT,               sort_order INTEGER NOT NULL DEFAULT 0,               created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP             );             CREATE INDEX IF NOT EXISTS idx_content_formats_content ON content_formats(content_id)",
        )
        .execute(&mut *conn)
        .await?;
        // `document_images` にコンテンツ / レンディションへの参照を追加する
        // （NULL = フェーズ2以前に取り込んだ旧データ。単一コンテンツ扱い）。
        ensure_column(
            &mut conn,
            "document_images",
            "content_id",
            "content_id TEXT REFERENCES book_contents(content_id)",
        )
        .await?;
        ensure_column(
            &mut conn,
            "document_images",
            "format_id",
            "format_id TEXT REFERENCES content_formats(format_id)",
        )
        .await?;
        // フェーズ2以前の旧ラベル（画像 / PDF / EPUB）を実データに合わせて書き換える
        // （データ移行。対象が無ければ何もしない）
        let migrated_labels = contents::migrate_legacy_labels(&mut conn).await?;
        if migrated_labels > 0 {
            log::info!("migrate: content_formats.label を {migrated_labels} 件更新");
        }
        // 進捗・ページ毎記録をコンテンツ単位に作り替える（旧スキーマのときだけ実行）
        let migrated_progress = migrate_progress_content_id(&mut conn).await?;
        if migrated_progress > 0 {
            log::info!("migrate: 進捗を {migrated_progress} 件コンテンツ単位へ移行");
        }
        // 共有ソースメタ列（FANZA同人 / DLsite）。開発中のためマイグレーションファイルは
        // 作らず、既存 runtime DDL（hidden_at / owner_sub 等）と同様に冪等に適用する。
        {
            let cols = [
                "media_category TEXT",
                "ai_type TEXT",
                // 既定は「不明（2）」。`0` を既定にすると、列を指定しない INSERT が
                // 「DRM なしと確認済み」の意味になってしまう（同期は明示的に値を入れる）。
                // 既存 DB は列追加済みなので既定は 0 のまま変わる（この既定は新規 DB に効く）。
                "is_drm INTEGER NOT NULL DEFAULT 2",
                "release_date TEXT",
                "description TEXT",
                "theme TEXT",
                "maker_id TEXT",
                "page_count INTEGER",
                "age_rating TEXT",
                "series_name TEXT",
            ];
            for table in ["bookshelf_items", "books"] {
                for def in cols {
                    let col = def.split_whitespace().next().unwrap();
                    ensure_column(&mut conn, table, col, def).await?;
                }
            }
        }
        // FANZA同人 / DLsite の sites 行（冪等）。
        sqlx::query(
            "INSERT OR IGNORE INTO sites (id, name, url, display_order, is_visible) \
             VALUES ('fanza', 'FANZA同人', 'https://www.dmm.co.jp/dc/doujin/', 2, 1)",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT OR IGNORE INTO sites (id, name, url, display_order, is_visible) \
             VALUES ('dlsite', 'DLsite', 'https://www.dlsite.com/', 3, 1)",
        )
        .execute(&mut *conn)
        .await?;
        // Fantia（`bookshelf_items.site_id` は `sites` への FK。行が無いと同期が落ちる）
        sqlx::query(
            "INSERT OR IGNORE INTO sites (id, name, url, display_order, is_visible) \
             VALUES ('fantia', 'Fantia', 'https://fantia.jp/', 4, 1)",
        )
        .execute(&mut *conn)
        .await?;
        // 複数アカウント対応: `books.owner_sub` と同じ所有者列を、アカウントに
        // 紐づく他テーブルにも持たせる。DB バックアップを現在 sub の行だけに
        // 絞るために使う（値は `books.owner_sub` と同じ keyring 鍵による暗号文）。
        // `favorite_entities` 等はこの関数の中で後から CREATE されるため、
        // **テーブル作成がすべて終わったここで**冪等に追加する。
        for table in [
            "bookshelf_items",
            "checked_items",
            "book_first_events",
            "favorite_tags",
            "favorite_entities",
        ] {
            ensure_column(&mut conn, table, "owner_sub", "owner_sub TEXT").await?;
        }
        // 複数アカウント対応: 同じ商品を owner ごとに持てるよう、shelf の PK を
        // `(site_id, database_id, owner_id)` に作り替える。既存 DB はテーブル再構築。
        migrate_owner_aware_shelf(&mut conn).await?;
        // 既存 DB は移行内で索引を作るが、移行済み DB でも owner 検索用の索引を保証する。
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS bookshelf_items_owner_id_idx \
             ON bookshelf_items(owner_id)",
        )
        .execute(&mut *conn)
        .await?;
        // 複数アカウント対応: 技術書典のチェックリストも owner ごとに持てるよう、
        // `checked_items` の PK と試し読みの FK を作り替える。
        migrate_owner_aware_checklist(&mut conn).await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS checked_items_owner_id_idx \
             ON checked_items(owner_id, event_id)",
        )
        .execute(&mut *conn)
        .await?;
        // 表紙バンドル（`ligrol-thumbs.json`）に載せる画像の派生キャッシュ
        // （`db::thumbs`）。α版なのでマイグレーションファイルを足さず、
        // 上の runtime DDL と同様に冪等に作る（新規 DB も schema.sql が持つ）。
        // **`db::backup::TABLES` の許可リストには入れない**（復元先で作り直せる
        // 派生データで、DB バックアップを太らせる意味がない）。
        // 旧 `thumbnail_share`（owner_id 無し）は PK を変更できないので作り直す。
        if !has_column(&mut conn, "thumbnail_share", "owner_id").await? {
            sqlx::query("DROP TABLE IF EXISTS thumbnail_share")
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS thumbnail_share (
                 kind TEXT NOT NULL,
                 item_key TEXT NOT NULL,
                 owner_id TEXT NOT NULL DEFAULT '',
                 mime TEXT NOT NULL,
                 width INTEGER NOT NULL,
                 height INTEGER NOT NULL,
                 sha256 TEXT NOT NULL,
                 bytes BLOB NOT NULL,
                 source_mtime INTEGER,
                 source_size INTEGER,
                 updated_at TEXT NOT NULL,
                 PRIMARY KEY (kind, item_key, owner_id)
             )",
        )
        .execute(&mut *conn)
        .await?;
        // AI / 抽出用の本文メタデータ。`document_text` は暗号化列だけを持つが、
        // どの経路で作られた本文か（PDF 抽出 / EPUB / OCR）と章情報を持たせる。
        // 開発中のため migration は増やさず、PRAGMA で存在確認して追加する。
        for (column, definition) in [
            ("source", "source TEXT NOT NULL DEFAULT 'extracted'"),
            ("engine", "engine TEXT"),
            ("confidence", "confidence REAL"),
            ("section_title", "section_title TEXT"),
            ("section_href", "section_href TEXT"),
        ] {
            ensure_column(&mut conn, "document_text", column, definition).await?;
        }
        // AI の操作要求（承認キュー）。状態を変える操作は MCP から直接実行せず、
        // ここに積んで Ligrol 本体が承認・実行する。device-local（Drive 対象外）。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS ai_jobs (
                 id TEXT PRIMARY KEY,
                 client_id TEXT NOT NULL,
                 action TEXT NOT NULL,
                 target TEXT NOT NULL,
                 params_json TEXT NOT NULL,
                 params_hash TEXT NOT NULL,
                 status TEXT NOT NULL DEFAULT 'pending_confirmation',
                 progress REAL NOT NULL DEFAULT 0,
                 result_json TEXT,
                 error_code TEXT,
                 idempotency_key TEXT,
                 owner_id TEXT NOT NULL DEFAULT '',
                 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 expires_at TEXT
             )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS ai_jobs_status_idx ON ai_jobs(status, created_at)",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS ai_jobs_idempotency_idx \
             ON ai_jobs(idempotency_key, owner_id) WHERE idempotency_key IS NOT NULL",
        )
        .execute(&mut *conn)
        .await?;
        // 監査ログ。本文・URL・トークンは保存しない。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS ai_audit_log (
                 id TEXT PRIMARY KEY,
                 owner_id TEXT NOT NULL DEFAULT '',
                 client_id TEXT NOT NULL,
                 action TEXT NOT NULL,
                 target TEXT NOT NULL,
                 status TEXT NOT NULL,
                 error_code TEXT,
                 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             )",
        )
        .execute(&mut *conn)
        .await?;
        ensure_column(&mut conn, "ai_audit_log", "owner_id", "owner_id TEXT NOT NULL DEFAULT ''").await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS ai_audit_log_created_idx \
             ON ai_audit_log(created_at)",
        )
        .execute(&mut *conn)
        .await?;
        // 古い取り込みドキュメントが残っている本を掃除する（冪等）。残すと
        // `images_for_selection` が同じページを複数返し、見開きに同じページが
        // 左右へ出る（実データで 1 ページが 3 行になっていた）。
        documents::prune_stale_documents_conn(&mut conn).await?;
        Ok(())
    })
}

/// P4: 初回起動（`owner_sub_model.initialized` フラグが無い）で既存データをクリアして
/// 新モデル（ownersub）で開始する。2 回目以降は何もしない。
/// 既存データは「開発版・旧フォーマット」のため削除して作り直す。
/// 戻り値: 初回でクリアしたら `true`。
pub fn clear_owner_model_if_first_run(
    pool: &SqlitePool,
    packs_dir: &Path,
    thumbnails_dir: &Path,
) -> Result<bool, sqlx::Error> {
    let initialized: i64 = block_on(async {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM app_settings WHERE key = 'owner_sub_model.initialized'",
        )
        .fetch_one(pool)
        .await
    })?;
    if initialized > 0 {
        return Ok(false);
    }
    // FK 安全の順（参照子 → 親）でクリアする。sites / tbf_events 等の参照テーブルは残す。
    for table in [
        "product_sample_pages",
        "token_analysis",
        "document_text",
        "document_images",
        "content_formats",
        "book_contents",
        "imported_documents",
        "book_tags",
        "reading_progress",
        "view_history",
        "page_views",
        "book_first_events",
        "checked_items",
        "bookshelf_items",
        "books",
        "drive_sync_state",
        "ai_jobs",
        "ai_audit_log",
        // 表紙バンドルの派生キャッシュ（親を消すので一緒に消す）
        "thumbnail_share",
    ] {
        let _ = block_on(async {
            sqlx::query(dynamic_sql(format!("DELETE FROM {table}")))
                .execute(pool)
                .await
        });
    }
    let _ = std::fs::remove_dir_all(packs_dir);
    let _ = std::fs::create_dir_all(packs_dir);
    let _ = std::fs::remove_dir_all(thumbnails_dir);
    let _ = std::fs::create_dir_all(thumbnails_dir);
    // 同期状態も「未同期」に戻す（新モデル開始時に最初のログインで
    // 「Google Drive と同期しますか？」を促すため）。
    for key in [
        "drive.last_sync_at",
        "drive.sync.enabled",
        "drive.sync.folder_id",
        crate::drive::sync::BACKUP_BASELINE_KEY,
        crate::drive::sync::THUMBS_BASELINE_KEY,
        crate::drive::sync::THUMBS_FAILED_KEY,
    ] {
        let _ = settings::delete(pool, key);
    }
    settings::set(pool, "owner_sub_model.initialized", "1")?;
    Ok(true)
}

/// `is_drm` の旧データを 3 状態の意味へ移す（一度きり）。
///
/// 以前の同期は `is_drm = 0` を「未検証」の意味で書いていた（[`crate::drm::DrmStatus`]
/// の doc 参照）。そのままだと本棚が「DRM なしと確認済み」と嘘をつくので、**一度だけ**
/// `0` → `2`（不明）へ移す。移行後に記録される `0` は「確認できた なし」なので
/// 上書きしない（フラグで 2 回目以降は何もしない）。
///
/// 戻り値: 移行したら `true`。
pub fn migrate_drm_status_once(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    let key = "drm.status.unknown_migrated";
    let migrated: i64 = block_on(async {
        sqlx::query_scalar("SELECT COUNT(*) FROM app_settings WHERE key = ?1")
            .bind(key)
            .fetch_one(pool)
            .await
    })?;
    if migrated > 0 {
        return Ok(false);
    }
    block_on(async {
        for table in ["bookshelf_items", "books"] {
            sqlx::query(dynamic_sql(format!(
                "UPDATE {table} SET is_drm = 2 WHERE is_drm = 0"
            )))
            .execute(pool)
            .await?;
        }
        Ok::<_, sqlx::Error>(())
    })?;
    settings::set(pool, key, "1")?;
    Ok(true)
}

/// 機密列（`document_text.text_content` / `token_analysis` の 4 列 / `page_notes.memo`）の
/// 平文を**一度だけ**暗号化する（セキュリティ評価 F02）。
///
/// 以前のバージョンはこれらを平文で書いていた。起動時に一度だけ暗号化して置き換え、
/// フラグ（`app_settings['column_crypto.v1_migrated']`）で 2 回目以降は何もしない。
/// **鍵が取れない・書き込みに失敗したときはフラグを立てない**ので次回起動で再試行する
/// （移行前の平文は読み取りでは平文として読めるので、失敗しても表示は壊れない）。
/// 平文の行が 1 つも無ければ**鍵（keyring）に触れずに**フラグだけ立てる。
///
/// 戻り値: 暗号化した行数。
pub fn migrate_column_crypto_once(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let key = "column_crypto.v1_migrated";
    let migrated: i64 = block_on(async {
        sqlx::query_scalar("SELECT COUNT(*) FROM app_settings WHERE key = ?1")
            .bind(key)
            .fetch_one(pool)
            .await
    })?;
    if migrated > 0 {
        return Ok(0);
    }
    if !block_on(column_crypto::has_plaintext_rows(pool))? {
        settings::set(pool, key, "1")?;
        return Ok(0);
    }
    // 鍵が無いなら**平文のまま放置せず**移行を失敗させる（フラグも立てない）。
    let crypto_key = column_crypto::db_key()?;
    let updated = block_on(async {
        let mut tx = pool.begin().await?;
        let updated = column_crypto::encrypt_plaintext_rows(&mut tx, &crypto_key).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(updated)
    })?;
    settings::set(pool, key, "1")?;
    if updated > 0 {
        // 書き換える前の平文は WAL に残る（データディレクトリのコピーに含まれる）。
        // 本体へ反映して WAL を切り詰め、平文の残骸を残さない。失敗しても移行自体は
        // 成立するのでエラーにはしない（次の書き込みで上書きされる）。
        if let Err(error) = block_on(async {
            sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                .fetch_all(pool)
                .await
        }) {
            log::warn!("column crypto: WAL の切り詰めに失敗: {error}");
        }
    }
    Ok(updated)
}

/// テスト用のインメモリプール（1 接続固定で同一メモリを共有）＋マイグレーション適用。
pub fn test_pool() -> SqlitePool {
    // 機密列の暗号化（`column_crypto`）が keyring に触れないようにする。テストは暗号鍵の
    // 生成・読み出しの経路で OS の許可ダイアログ（開発機）や keyring の不在（CI）に当たる。
    // プロセス内メモリのバックエンドに固定する（`secrets` のテストと app の
    // `AppState::init_test` と同じ扱い）。
    crate::secrets::SecretStore::use_memory_backend();
    let pool = block_on(async {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(":memory:")
            .foreign_keys(true)
            .create_if_missing(true);
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            // `:memory:` は接続ごとに別 DB。既定の寿命（30 分）・アイドル（10 分）で接続が
            // 入れ替わるとテーブルの無い空の DB に繋がるので、1 本を張りっぱなしにする
            // （アプリ側の `app_state::in_memory_pool` と同じ理由）。
            .min_connections(1)
            .max_lifetime(None)
            .idle_timeout(None)
            .connect_with(options)
            .await
    })
    .expect("test pool");
    migrate(&pool).expect("test migrate");
    pool
}

/// Acquire a connection for a synchronous callback (used by repository
/// functions that need raw connection access).
pub fn with_conn<T>(
    pool: &SqlitePool,
    f: impl FnOnce(&mut SqliteConnection) -> Result<T, sqlx::Error>,
) -> Result<T, sqlx::Error> {
    block_on(async {
        let mut conn = pool.acquire().await?;
        f(&mut conn)
    })
}
