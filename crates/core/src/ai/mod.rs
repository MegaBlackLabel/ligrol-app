//! AI 向けの読み取りモデルと操作要求（承認キュー）。
//!
//! MCP / CLI はこのモジュールだけを使う。DB のテーブルや暗号文を直接返さず、
//! 所有者・非表示・スコープをここで一元的に検証する。
//!
//! 読み取りは `AiContext` のスコープに従う。書き込みは [`jobs`] に積むだけで、
//! 実行は Ligrol 本体（UI）が行う。

pub mod covers;
pub mod jobs;
pub mod library;
pub mod notes;
pub mod purchases;
pub mod stats;
pub mod tags;
pub mod tbf;
pub mod text;

pub use library::BookQuery;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::db::{SqlitePool, books};
use crate::secrets::SecretStore;

/// 読み取り / 操作の許可スコープ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiScopes {
    pub library: bool,
    pub text: bool,
    pub stats: bool,
    pub notes: bool,
    pub purchases: bool,
    pub tbf: bool,
    /// 操作要求（ダウンロード / 開く / 抽出）を受け付けるか。
    pub actions: bool,
    pub read_only: bool,
}

impl Default for AiScopes {
    fn default() -> Self {
        // 既定はメタデータ・統計・購入一覧・TBF。本文と付箋は明示 ON。
        Self {
            library: true,
            text: false,
            stats: true,
            notes: false,
            purchases: true,
            tbf: true,
            actions: false,
            read_only: true,
        }
    }
}

impl AiScopes {
    /// MCP の環境変数からスコープを作る。
    ///
    /// `LIGROL_AI_TEXT=1` / `LIGROL_AI_NOTES=1` / `LIGROL_AI_ACTIONS=1` で有効化。
    /// `LIGROL_AI_READ_ONLY=1`（既定）では操作を受け付けない。
    pub fn from_env() -> Self {
        let on = |key: &str, default: bool| match std::env::var(key) {
            Ok(value) => matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            ),
            Err(_) => default,
        };
        let read_only = on("LIGROL_AI_READ_ONLY", true);
        Self {
            library: on("LIGROL_AI_LIBRARY", true),
            text: on("LIGROL_AI_TEXT", false),
            stats: on("LIGROL_AI_STATS", true),
            notes: on("LIGROL_AI_NOTES", false),
            purchases: on("LIGROL_AI_PURCHASES", true),
            tbf: on("LIGROL_AI_TBF", true),
            actions: !read_only && on("LIGROL_AI_ACTIONS", false),
            read_only,
        }
    }

    pub fn all_for_tests() -> Self {
        Self {
            library: true,
            text: true,
            stats: true,
            notes: true,
            purchases: true,
            tbf: true,
            actions: true,
            read_only: false,
        }
    }
}

/// AI 読み取りの文脈。所有者とスコープを固定する。
#[derive(Debug, Clone)]
pub struct AiContext {
    /// 現在の Google sub。`None` = 未ログイン（未所属の本だけ）。
    pub owner_sub: Option<String>,
    /// `owner_sub` の復号鍵。`None` のときは所属本を読めない（fail-closed）。
    pub owner_key: Option<[u8; 32]>,
    pub scopes: AiScopes,
}

impl AiContext {
    /// keyring から現在の所有者を読む。鍵が無い場合は `owner_key = None`。
    pub fn from_secrets(secrets: &SecretStore, scopes: AiScopes) -> Self {
        let profile = crate::google::saved_profile(secrets);
        let owner_sub = profile.map(|profile| profile.sub);
        let owner_key = secrets.try_db_key().ok().flatten();
        Self {
            owner_sub,
            owner_key,
            scopes,
        }
    }

    /// テスト用: 未所属（owner_sub = NULL）の文脈。
    pub fn unowned(scopes: AiScopes) -> Self {
        Self {
            owner_sub: None,
            owner_key: None,
            scopes,
        }
    }

    pub fn owner_id(&self) -> String {
        self.owner_sub
            .as_deref()
            .map(nartik::derive_owner_id)
            .unwrap_or_default()
    }

    /// 現在の所有者に見える本の ID。所有者が不明なら fail-closed。
    pub fn visible_book_ids(&self, pool: &SqlitePool) -> Result<HashSet<String>, AiError> {
        match (&self.owner_key, &self.owner_sub) {
            (Some(key), sub) => {
                let ids = books::owned_book_ids(pool, key, sub.as_deref())?;
                Ok(ids)
            }
            (None, None) => {
                // 未ログイン: `owner_sub IS NULL` の本だけ。
                let rows = books::list_owner_subs(pool)?;
                Ok(rows
                    .into_iter()
                    .filter(|(_, owner_sub)| owner_sub.is_none())
                    .map(|(id, _)| id)
                    .collect())
            }
            (None, Some(_)) => Err(AiError::OwnerUnknown),
        }
    }

    pub fn require_library(&self) -> Result<(), AiError> {
        self.scopes
            .library
            .then_some(())
            .ok_or(AiError::ScopeDisabled("library.read"))
    }

    pub fn require_text(&self) -> Result<(), AiError> {
        self.scopes
            .text
            .then_some(())
            .ok_or(AiError::ScopeDisabled("text.read"))
    }

    pub fn require_stats(&self) -> Result<(), AiError> {
        self.scopes
            .stats
            .then_some(())
            .ok_or(AiError::ScopeDisabled("stats.read"))
    }

    pub fn require_notes(&self) -> Result<(), AiError> {
        self.scopes
            .notes
            .then_some(())
            .ok_or(AiError::ScopeDisabled("notes.read"))
    }

    pub fn require_purchases(&self) -> Result<(), AiError> {
        self.scopes
            .purchases
            .then_some(())
            .ok_or(AiError::ScopeDisabled("purchases.read"))
    }

    pub fn require_tbf(&self) -> Result<(), AiError> {
        self.scopes
            .tbf
            .then_some(())
            .ok_or(AiError::ScopeDisabled("tbf.read"))
    }

    pub fn require_actions(&self) -> Result<(), AiError> {
        if self.scopes.read_only || !self.scopes.actions {
            return Err(AiError::ScopeDisabled("actions.write"));
        }
        Ok(())
    }

    /// 本文を復号する鍵。無い場合は本文を返せない。
    pub fn text_key(&self) -> Result<&[u8; 32], AiError> {
        self.owner_key.as_ref().ok_or(AiError::TextUnavailable)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("not found")]
    NotFound,
    #[error("owner is unknown")]
    OwnerUnknown,
    #[error("scope is disabled: {0}")]
    ScopeDisabled(&'static str),
    #[error("text is unavailable")]
    TextUnavailable,
    #[error("not authorized")]
    NotAuthorized,
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("revision conflict")]
    Conflict,
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl AiError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Db(_) => "database_error",
            Self::NotFound => "not_found",
            Self::OwnerUnknown => "owner_unknown",
            Self::ScopeDisabled(_) => "scope_disabled",
            Self::TextUnavailable => "text_unavailable",
            Self::NotAuthorized => "not_authorized",
            Self::Invalid(_) => "invalid_request",
            Self::Conflict => "revision_conflict",
            Self::Unsupported(_) => "unsupported",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::Db(_) | Self::Conflict)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagRef {
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressView {
    /// `unread` / `reading` / `read`
    pub state: String,
    pub current_page: i64,
    pub total_pages: Option<i64>,
    pub finished_at: Option<String>,
    pub last_read_at: Option<String>,
}

impl Default for ProgressView {
    fn default() -> Self {
        Self {
            state: "unread".to_string(),
            current_page: 0,
            total_pages: None,
            finished_at: None,
            last_read_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StatsView {
    pub sessions: i64,
    pub total_seconds: i64,
    pub last_read_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverRef {
    pub uri: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookSummary {
    pub book_id: String,
    pub title: String,
    pub author: String,
    pub circle_name: String,
    pub site_id: Option<String>,
    pub purchase_date: Option<String>,
    pub release_date: Option<String>,
    pub is_favorite: bool,
    pub is_hidden: bool,
    pub page_count: Option<i64>,
    pub tags: Vec<TagRef>,
    pub progress: ProgressView,
    pub stats: StatsView,
    pub cover: Option<CoverRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatView {
    pub format_id: String,
    pub label: String,
    pub format_kind: String,
    pub page_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentView {
    pub content_id: String,
    pub display_name: String,
    pub media_kind: String,
    pub is_primary: bool,
    pub formats: Vec<FormatView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TextIndexStatus {
    pub segments: i64,
    pub chars: i64,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookDetail {
    #[serde(flatten)]
    pub summary: BookSummary,
    pub description: Option<String>,
    pub theme: Option<String>,
    pub series_name: Option<String>,
    pub media_category: Option<String>,
    pub ai_type: Option<String>,
    pub age_rating: Option<String>,
    pub is_drm: i64,
    pub contents: Vec<ContentView>,
    pub text: TextIndexStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextSegment {
    /// `page` / `section`
    pub kind: String,
    pub ordinal: i64,
    pub title: Option<String>,
    pub href: Option<String>,
    pub text: String,
    pub source: String,
    pub engine: Option<String>,
    pub confidence: Option<f64>,
    pub chars: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextMatch {
    pub book_id: String,
    pub title: String,
    pub kind: String,
    pub ordinal: i64,
    pub section_title: Option<String>,
    pub href: Option<String>,
    pub source: String,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagStat {
    pub tag: String,
    pub count: i64,
    pub finished: i64,
    pub total_seconds: i64,
    pub favorite: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatRow {
    pub key: String,
    pub label: String,
    pub books: i64,
    pub sessions: i64,
    pub total_seconds: i64,
    pub finished: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadingStats {
    pub group_by: String,
    pub rows: Vec<StatRow>,
    pub totals: StatsView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteView {
    pub book_id: String,
    pub book_title: String,
    pub content_id: String,
    pub page: i64,
    pub memo: String,
    pub is_active: bool,
    pub spread_side: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadChoice {
    pub index: usize,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseItem {
    pub site_id: String,
    pub database_id: String,
    pub title: String,
    pub circle_name: String,
    pub author: String,
    pub format: String,
    pub is_downloadable: bool,
    pub is_purchased: bool,
    pub downloaded: bool,
    pub choices: Vec<DownloadChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TbfItemView {
    pub id: String,
    pub circle_name: String,
    pub space_number: String,
    pub product_title: String,
    pub memo: String,
    pub price: Option<i64>,
    pub is_checked: bool,
    pub is_purchased: bool,
    pub has_cover: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TbfPlan {
    pub event_id: String,
    pub event_name: String,
    pub event_date: Option<String>,
    pub items: Vec<TbfItemView>,
    pub budget_total: i64,
    pub purchased_total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverImage {
    pub book_id: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
    pub base64: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobView {
    pub job_id: String,
    pub action: String,
    pub target: String,
    pub status: String,
    pub progress: f64,
    pub result: Option<serde_json::Value>,
    pub error_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<crate::db::ai_jobs::AiJob> for JobView {
    fn from(job: crate::db::ai_jobs::AiJob) -> Self {
        Self {
            job_id: job.id,
            action: job.action,
            target: job.target,
            status: job.status,
            progress: job.progress,
            result: job
                .result_json
                .and_then(|value| serde_json::from_str(&value).ok()),
            error_code: job.error_code,
            created_at: job.created_at,
            updated_at: job.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub schema_version: i64,
    pub owner: String,
    pub read_only: bool,
    pub scopes: Vec<String>,
    pub jobs_pending: i64,
    pub audit_recent: Vec<serde_json::Value>,
}

/// 本文の復号鍵を解決する。読み取り専用プロセスが鍵を新規生成しない。
pub fn key_for_read(secrets: &SecretStore) -> Option<[u8; 32]> {
    secrets.try_db_key().ok().flatten()
}

/// `document_text` の既存メタから本文インデックスの状態を作る。
pub(crate) fn text_index_status(
    pool: &SqlitePool,
    document_id: &str,
    key: Option<&[u8; 32]>,
) -> Result<TextIndexStatus, AiError> {
    let rows = crate::db::documents::all_text_rows_for_document(pool, document_id, key)?;
    let mut sources = Vec::new();
    let mut chars = 0i64;
    for row in &rows {
        if !sources.contains(&row.source) {
            sources.push(row.source.clone());
        }
        chars += row.text.as_deref().unwrap_or("").chars().count() as i64;
    }
    Ok(TextIndexStatus {
        segments: rows.len() as i64,
        chars,
        sources,
    })
}

pub fn status(pool: &SqlitePool, ctx: &AiContext) -> Result<Status, AiError> {
    let pending: i64 = crate::db::block_on(async {
        sqlx::query_scalar("SELECT COUNT(*) FROM ai_jobs WHERE status = 'pending_confirmation'")
            .fetch_one(pool)
            .await
    })?;
    let owner = match (&ctx.owner_sub, &ctx.owner_key) {
        (Some(_), Some(_)) => "authenticated",
        (None, None) => "signed_out",
        _ => "unknown",
    };
    let mut scopes = Vec::new();
    if ctx.scopes.library {
        scopes.push("library.read".to_string());
    }
    if ctx.scopes.text {
        scopes.push("text.read".to_string());
    }
    if ctx.scopes.stats {
        scopes.push("stats.read".to_string());
    }
    if ctx.scopes.notes {
        scopes.push("notes.read".to_string());
    }
    if ctx.scopes.purchases {
        scopes.push("purchases.read".to_string());
    }
    if ctx.scopes.tbf {
        scopes.push("tbf.read".to_string());
    }
    if !ctx.scopes.read_only && ctx.scopes.actions {
        scopes.push("actions.request".to_string());
    }
    Ok(Status {
        version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: 1,
        owner: owner.to_string(),
        read_only: ctx.scopes.read_only,
        scopes,
        jobs_pending: pending,
        audit_recent: crate::db::ai_jobs::recent_audit(pool, &ctx.owner_id(), 20)?,
    })
}
