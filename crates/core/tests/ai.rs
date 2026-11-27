//! AI 読み取りモデル / 承認キューの統合テスト。
//!
//! 実データは使わない。`test_pool` に最小の本・本文・付箋・購入行を入れて、
//! 所有者・非表示・スコープ・ジョブ検証を固定する。

use ligrol_core::ai::{
    AiContext, AiError, AiScopes, BookQuery, jobs, library, notes, purchases, stats, tags, tbf, text,
};
use ligrol_core::db::{
    SqlitePool, block_on, books, bookshelf, documents, notes as notes_db, progress, tags as tags_db,
};
use ligrol_core::secrets::SecretStore;

fn pool() -> SqlitePool {
    SecretStore::use_memory_backend();
    ligrol_core::db::test_pool()
}

fn ctx() -> AiContext {
    AiContext::unowned(AiScopes::all_for_tests())
}

fn seed_book(pool: &SqlitePool, id: &str, title: &str, hidden: i64) {
    books::insert(
        pool,
        &books::Book {
            id: id.into(),
            title: title.into(),
            author: "著者".into(),
            circle_name: "サークル".into(),
            purchase_date: Some("2026-05-05".into()),
            file_name: format!("{id}.pdf"),
            file_size: 100,
            opfs_path: format!("{id}.nartik"),
            cover_thumbnail: None,
            tbf_product_id: None,
            site_id: Some("techbookfest".into()),
            tags_fetched: 1,
            pack_id: Some(id.into()),
            is_favorite: 0,
            is_hidden: hidden,
            created_at: "2026-11-01 00:00:00".into(),
            updated_at: "2026-11-01 00:00:00".into(),
            media_category: None,
            ai_type: None,
            is_drm: 0,
            release_date: None,
            description: Some("説明".into()),
            theme: None,
            maker_id: None,
            page_count: Some(10),
            age_rating: None,
            series_name: None,
        },
    )
    .unwrap();
}

fn seed_document(pool: &SqlitePool, book_id: &str) {
    documents::insert_document(
        pool,
        &documents::ImportedDocument {
            id: format!("doc-{book_id}"),
            book_id: book_id.into(),
            source_type: "epub".into(),
            file_hash: "hash".into(),
            total_pages: 1,
            metadata: None,
            status: "done".into(),
            created_at: "2026-11-01 00:00:00".into(),
            updated_at: "2026-11-01 00:00:00".into(),
        },
    )
    .unwrap();
}

fn seed_shelf_item(pool: &SqlitePool, database_id: &str, downloadable: bool) {
    bookshelf::upsert(
        pool,
        &bookshelf::BookshelfItem {
            site_id: "techbookfest".into(),
            database_id: database_id.into(),
            title: format!("購入本 {database_id}"),
            circle_name: "サークル".into(),
            author: "著者".into(),
            thumbnail_url: None,
            format: "PDF".into(),
            caused_at: Some("2026-05-05".into()),
            event_name: None,
            event_slug: None,
            event_id: None,
            file_name: Some("book.pdf".into()),
            download_url: Some("https://example.invalid/book.pdf".into()),
            download_options: None,
            is_downloadable: downloadable as i64,
            is_checked: 0,
            is_purchased: 1,
            is_new: 0,
            is_active: 1,
            is_favorite: 0,
            is_hidden: 0,
            hidden_at: None,
            tags_json: None,
            synced_at: "2026-11-01 00:00:00".into(),
            created_at: "2026-11-01 00:00:00".into(),
            updated_at: "2026-11-01 00:00:00".into(),
            media_category: None,
            ai_type: None,
            is_drm: 0,
            release_date: None,
            description: None,
            theme: None,
            maker_id: None,
            page_count: None,
            age_rating: None,
            series_name: None,
        },
    )
    .unwrap();
}

#[test]
fn search_filters_hidden_and_tag_and_reports_progress() {
    let pool = pool();
    seed_book(&pool, "b1", "Rust 非同期", 0);
    seed_book(&pool, "b2", "隠し本", 1);
    tags_db::set_for_book(&pool, "b1", &[("Rust", "generated")]).unwrap();
    progress::upsert(
        &pool,
        &progress::ReadingProgress {
            book_id: "b1".into(),
            content_id: String::new(),
            current_page: 5,
            total_pages: Some(10),
            finished_at: None,
            last_read_at: "2026-11-20 10:00:00".into(),
        },
    )
    .unwrap();

    let page = library::search(
        &pool,
        &ctx(),
        &BookQuery {
            query: Some("rust".into()),
            tag: Some("Rust".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].book_id, "b1");
    assert_eq!(page.items[0].progress.state, "reading");
    assert_eq!(page.items[0].progress.current_page, 5);

    let hidden = library::search(
        &pool,
        &ctx(),
        &BookQuery {
            query: Some("隠し".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(hidden.items.is_empty(), "hidden 本が検索に出ている");
}

#[test]
fn text_and_search_return_epub_sections_and_respect_scope() {
    let pool = pool();
    seed_book(&pool, "b1", "EPUB の本", 0);
    seed_document(&pool, "b1");
    documents::insert_text(
        &pool,
        &documents::DocumentText {
            id: "dt-1".into(),
            document_id: "doc-b1".into(),
            content_id: "c1".into(),
            page_number: 1,
            text_content: "第1章 Rust の非同期処理".into(),
            source: "epub".into(),
            engine: Some("epub-xhtml".into()),
            confidence: None,
            section_title: Some("第1章".into()),
            section_href: Some("OEBPS/ch01.xhtml".into()),
            created_at: "2026-11-01 00:00:00".into(),
        },
    )
    .unwrap();

    let segments = text::get_book_text(&pool, &ctx(), "b1", None, None, None).unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].kind, "section");
    assert_eq!(segments[0].title.as_deref(), Some("第1章"));
    assert_eq!(segments[0].source, "epub");

    let matches = text::search_text(&pool, &ctx(), &[], "非同期", None, None).unwrap();
    assert_eq!(matches.items.len(), 1);
    assert_eq!(matches.items[0].ordinal, 1);

    let mut scopes = AiScopes::all_for_tests();
    scopes.text = false;
    let no_text = AiContext::unowned(scopes);
    assert!(matches!(
        text::get_book_text(&pool, &no_text, "b1", None, None, None),
        Err(AiError::ScopeDisabled("text.read"))
    ));
}

#[test]
fn notes_are_scoped_and_decrypted() {
    let pool = pool();
    seed_book(&pool, "b1", "付箋の本", 0);
    notes_db::upsert(
        &pool,
        &notes_db::PageNoteInput {
            id: "note-b1-1",
            book_id: "b1",
            content_id: "",
            page: 1,
            memo: "ここ重要",
            spread_side: None,
        },
    )
    .unwrap();

    let rows = notes::list_notes(&pool, &ctx(), Some("b1"), None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memo, "ここ重要");

    let mut scopes = AiScopes::all_for_tests();
    scopes.notes = false;
    let no_notes = AiContext::unowned(scopes);
    assert!(matches!(
        notes::list_notes(&pool, &no_notes, None, None),
        Err(AiError::ScopeDisabled("notes.read"))
    ));
}

#[test]
fn download_request_validates_purchase_and_is_idempotent() {
    let pool = pool();
    seed_shelf_item(&pool, "p1", true);
    seed_shelf_item(&pool, "p2", false);

    let request = jobs::DownloadRequest {
        site_id: "techbookfest",
        database_id: "p1",
        option_index: Some(0),
        idempotency_key: Some("idem-1"),
    };
    let job = jobs::request_download(&pool, &ctx(), "test-client", &request).unwrap();
    assert_eq!(job.status, "pending_confirmation");
    let again = jobs::request_download(&pool, &ctx(), "test-client", &request).unwrap();
    assert_eq!(again.job_id, job.job_id);

    let not_downloadable = jobs::DownloadRequest {
        site_id: "techbookfest",
        database_id: "p2",
        option_index: None,
        idempotency_key: None,
    };
    assert!(matches!(
        jobs::request_download(&pool, &ctx(), "test-client", &not_downloadable),
        Err(AiError::Unsupported(_))
    ));

    let unknown = jobs::DownloadRequest {
        site_id: "techbookfest",
        database_id: "nope",
        option_index: None,
        idempotency_key: None,
    };
    assert!(matches!(
        jobs::request_download(&pool, &ctx(), "test-client", &unknown),
        Err(AiError::NotFound)
    ));
}

#[test]
fn stats_group_by_tag_uses_daily_history() {
    let pool = pool();
    seed_book(&pool, "b1", "集計の本", 0);
    tags_db::set_for_book(&pool, "b1", &[("Rust", "generated")]).unwrap();
    block_on(async {
        sqlx::query(
            "INSERT INTO view_history (id, book_id, started_at, ended_at) \
             VALUES ('s1', 'b1', '2026-11-01 10:00:00', '2026-11-01 10:10:00')",
        )
        .execute(&pool)
        .await
        .unwrap();
    });

    let report = stats::reading_stats(&pool, &ctx(), "tag", None, None).unwrap();
    assert_eq!(report.rows.len(), 1);
    assert_eq!(report.rows[0].key, "Rust");
    assert_eq!(report.rows[0].sessions, 1);
    assert_eq!(report.rows[0].total_seconds, 600);
}

#[test]
fn purchase_list_marks_downloaded_items() {
    let pool = pool();
    seed_book(&pool, "b1", "取込済み", 0);
    seed_shelf_item(&pool, "p1", true);
    // 本と購入行を紐づける（TBF は database_id = tbf_product_id）。
    block_on(async {
        sqlx::query(
            "UPDATE books SET site_id = 'techbookfest', tbf_product_id = 'p1' WHERE id = 'b1'",
        )
        .execute(&pool)
        .await
        .unwrap();
    });
    let items = purchases::list_purchases(&pool, &ctx(), None, true).unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0].downloaded);
}


#[test]
fn search_includes_description_theme_and_sorts_by_title() {
    let pool = pool();
    seed_book(&pool, "b1", "A の本", 0);
    seed_book(&pool, "b2", "B の本", 0);
    ligrol_core::db::block_on(async {
        sqlx::query("UPDATE books SET description = '固有の手がかり' WHERE id = 'b1'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE books SET theme = '固有の手がかり' WHERE id = 'b2'")
            .execute(&pool)
            .await
            .unwrap();
    });

    let page = library::search(
        &pool,
        &ctx(),
        &BookQuery {
            query: Some("固有の手がかり".into()),
            sort: Some("title".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.items.len(), 2);
    assert!(page.items[0].title.starts_with('A'));
    assert!(page.items[1].title.starts_with('B'));
}

#[test]
fn text_search_pages_with_an_opaque_cursor() {
    let pool = pool();
    seed_book(&pool, "b1", "ページングの本", 0);
    seed_document(&pool, "b1");
    for (page, text) in [(1, "非同期の話"), (2, "非同期の続き"), (3, "非同期の結論")] {
        documents::insert_text(
            &pool,
            &documents::DocumentText {
                id: format!("dt-{page}"),
                document_id: "doc-b1".into(),
                content_id: String::new(),
                page_number: page,
                text_content: text.into(),
                source: "extracted".into(),
                engine: Some("pdfium".into()),
                confidence: None,
                section_title: None,
                section_href: None,
                created_at: "2026-11-01 00:00:00".into(),
            },
        )
        .unwrap();
    }

    let first = text::search_text(&pool, &ctx(), &["b1".into()], "非同期", Some(1), None).unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].ordinal, 1);
    assert!(first.next_cursor.is_some());

    let second = text::search_text(
        &pool,
        &ctx(),
        &["b1".into()],
        "非同期",
        Some(1),
        first.next_cursor.as_deref(),
    )
    .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].ordinal, 2);
}


#[test]
fn tbf_events_lists_events() {
    let pool = pool();
    ligrol_core::db::checklist::upsert_event(
        &pool,
        &ligrol_core::db::checklist::TbfEvent {
            id: "tbf18".into(),
            site_id: "techbookfest".into(),
            slug: Some("tbf18".into()),
            tbf_event_id: Some("Event:tbf18".into()),
            event_name: "技術書典18".into(),
            event_date: Some("2026-05-05".into()),
            event_start_date: Some("2026-05-01".into()),
            event_end_date: Some("2026-05-31".into()),
            event_format: "offline".into(),
            is_cancelled: 0,
            display_order: 0,
            is_featured: 1,
            poll_sync_enabled: 1,
            created_at: "2026-11-01 00:00:00".into(),
            updated_at: "2026-11-01 00:00:00".into(),
        },
    )
    .unwrap();
    let events = tbf::events(&pool, &ctx()).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, "tbf18");
    assert!(events[0].is_featured);
}


#[test]
fn note_writes_use_revision_conflicts_and_soft_delete() {
    let pool = pool();
    seed_book(&pool, "b1", "書き込みの本", 0);
    let ctx = ctx();
    let note = notes::add_note(&pool, &ctx, "b1", "", 3, "最初のメモ", None).unwrap();
    assert_eq!(note.memo, "最初のメモ");
    assert!(note.is_active);

    assert!(matches!(
        notes::add_note(
            &pool,
            &ctx,
            "b1",
            "",
            3,
            "競合する更新",
            Some("2000-01-01 00:00:00")
        ),
        Err(AiError::Conflict)
    ));

    let updated = notes::add_note(
        &pool,
        &ctx,
        "b1",
        "",
        3,
        "更新したメモ",
        Some(&note.updated_at),
    )
    .unwrap();
    assert_eq!(updated.memo, "更新したメモ");

    let removed = notes::remove_note(&pool, &ctx, "b1", "", 3, Some(&updated.updated_at)).unwrap();
    assert!(!removed.is_active);
    assert_eq!(removed.memo, "更新したメモ");
}


#[test]
fn tag_writes_add_and_remove_manual_tags() {
    let pool = pool();
    seed_book(&pool, "b1", "タグ書き込みの本", 0);
    let ctx = ctx();
    let added = tags::add_tag(&pool, &ctx, "b1", "手動タグ").unwrap();
    assert_eq!(added.name, "手動タグ");
    assert_eq!(added.source, "manual");
    // 同じタグを足しても重複しない
    let again = tags::add_tag(&pool, &ctx, "b1", "手動タグ").unwrap();
    assert_eq!(again, added);
    let rows = tags_db::list_for_book(&pool, "b1").unwrap();
    assert_eq!(rows.len(), 1);

    assert!(tags::remove_tag(&pool, &ctx, "b1", "手動タグ").unwrap());
    assert!(tags_db::list_for_book(&pool, "b1").unwrap().is_empty());
}
