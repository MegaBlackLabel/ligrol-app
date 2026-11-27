//! AI からの操作要求（承認キュー）と監査ログ。
//!
//! MCP は読み取りだけを直接行い、**状態を変える操作はこのテーブルに積む**。
//! 実行は Ligrol 本体（UI）が行い、承認・進捗・結果をここへ戻す。
//! 本文・URL・トークンは保存しない（params は対象 ID と選択肢だけ）。

use sha2::{Digest, Sha256};
use sqlx::{Row, sqlite::SqlitePool};

/// `ai_jobs.status` の値。
pub mod status {
    pub const PENDING_CONFIRMATION: &str = "pending_confirmation";
    pub const QUEUED: &str = "queued";
    pub const RUNNING: &str = "running";
    pub const SUCCEEDED: &str = "succeeded";
    pub const FAILED: &str = "failed";
    pub const CANCELLED: &str = "cancelled";
    pub const EXPIRED: &str = "expired";

    pub fn is_terminal(value: &str) -> bool {
        matches!(value, SUCCEEDED | FAILED | CANCELLED | EXPIRED)
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AiJob {
    pub id: String,
    pub client_id: String,
    pub action: String,
    pub target: String,
    pub params_json: String,
    pub params_hash: String,
    pub status: String,
    pub progress: f64,
    pub result_json: Option<String>,
    pub error_code: Option<String>,
    pub idempotency_key: Option<String>,
    pub owner_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
}

pub struct NewAiJob<'a> {
    pub client_id: &'a str,
    pub action: &'a str,
    pub target: &'a str,
    pub params_json: &'a str,
    pub idempotency_key: Option<&'a str>,
    pub owner_id: &'a str,
    /// 承認が必要なら 60 秒で期限切れにする。自動実行なら `None`。
    pub expires_in_seconds: Option<i64>,
}

pub fn hash_params(params_json: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(params_json.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 保存済みの `params_json` が enqueue 時のハッシュと一致するか。
/// 承認前に改変されていないことの確認に使う。
pub fn params_match(job: &AiJob) -> bool {
    hash_params(&job.params_json) == job.params_hash
}

pub fn enqueue(pool: &SqlitePool, job: &NewAiJob<'_>) -> Result<AiJob, sqlx::Error> {
    crate::db::block_on(async {
        if let Some(key) = job.idempotency_key {
            let existing = sqlx::query_as::<_, AiJob>(
                "SELECT id, client_id, action, target, params_json, params_hash, status, progress, \
                 result_json, error_code, idempotency_key, owner_id, created_at, updated_at, \
                 expires_at FROM ai_jobs WHERE idempotency_key = ?1 AND owner_id = ?2 \
                 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(key)
            .bind(job.owner_id)
            .fetch_optional(pool)
            .await?;
            if let Some(existing) = existing {
                return Ok(existing);
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let params_hash = hash_params(job.params_json);
        let sql = match job.expires_in_seconds {
            Some(_) => {
                "INSERT INTO ai_jobs (id, client_id, action, target, params_json, params_hash, \
                 status, progress, idempotency_key, owner_id, expires_at) VALUES \
                 (?1, ?2, ?3, ?4, ?5, ?6, 'pending_confirmation', 0, ?7, ?8, \
                 datetime('now', '+60 seconds')) RETURNING *"
            }
            None => {
                "INSERT INTO ai_jobs (id, client_id, action, target, params_json, params_hash, \
                 status, progress, idempotency_key, owner_id, expires_at) VALUES \
                 (?1, ?2, ?3, ?4, ?5, ?6, 'queued', 0, ?7, ?8, NULL) RETURNING *"
            }
        };
        let job = sqlx::query_as::<_, AiJob>(sql)
            .bind(&id)
            .bind(job.client_id)
            .bind(job.action)
            .bind(job.target)
            .bind(job.params_json)
            .bind(&params_hash)
            .bind(job.idempotency_key)
            .bind(job.owner_id)
            .fetch_one(pool)
            .await?;
        Ok(job)
    })
}

pub fn get(pool: &SqlitePool, id: &str) -> Result<Option<AiJob>, sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query_as::<_, AiJob>(
            "SELECT id, client_id, action, target, params_json, params_hash, status, progress, \
             result_json, error_code, idempotency_key, owner_id, created_at, updated_at, \
             expires_at FROM ai_jobs WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    })
}

/// 承認待ちのジョブ（古い順）。UI がポーリングする。
pub fn pending(pool: &SqlitePool) -> Result<Vec<AiJob>, sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query_as::<_, AiJob>(
            "SELECT id, client_id, action, target, params_json, params_hash, status, progress, \
             result_json, error_code, idempotency_key, owner_id, created_at, updated_at, \
             expires_at FROM ai_jobs WHERE status = 'pending_confirmation' \
             ORDER BY created_at ASC LIMIT 20",
        )
        .fetch_all(pool)
        .await
    })
}

/// 承認済みで実行待ち / 実行中のジョブ（UI が完了を検出するため）。
pub fn active(pool: &SqlitePool) -> Result<Vec<AiJob>, sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query_as::<_, AiJob>(
            "SELECT id, client_id, action, target, params_json, params_hash, status, progress, \
             result_json, error_code, idempotency_key, owner_id, created_at, updated_at, \
             expires_at FROM ai_jobs WHERE status IN ('queued', 'running') \
             ORDER BY created_at ASC LIMIT 50",
        )
        .fetch_all(pool)
        .await
    })
}

pub fn set_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    progress: f64,
    error_code: Option<&str>,
) -> Result<(), sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query(
            "UPDATE ai_jobs SET status = ?2, progress = ?3, error_code = ?4, \
             updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        )
        .bind(id)
        .bind(status)
        .bind(progress)
        .bind(error_code)
        .execute(pool)
        .await?;
        Ok(())
    })
}

pub fn finish(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    result_json: Option<&str>,
    error_code: Option<&str>,
) -> Result<(), sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query(
            "UPDATE ai_jobs SET status = ?2, progress = 1, result_json = ?3, \
             error_code = ?4, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        )
        .bind(id)
        .bind(status)
        .bind(result_json)
        .bind(error_code)
        .execute(pool)
        .await?;
        Ok(())
    })
}

pub fn expire_stale(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    crate::db::block_on(async {
        let result = sqlx::query(
            "UPDATE ai_jobs SET status = 'expired', error_code = 'confirmation_expired', \
             updated_at = CURRENT_TIMESTAMP \
             WHERE status = 'pending_confirmation' AND expires_at IS NOT NULL \
               AND expires_at <= CURRENT_TIMESTAMP",
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    })
}

/// 監査ログ。本文・URL・トークンは入れない。
pub fn audit(
    pool: &SqlitePool,
    owner_id: &str,
    client_id: &str,
    action: &str,
    target: &str,
    status: &str,
    error_code: Option<&str>,
) -> Result<(), sqlx::Error> {
    crate::db::block_on(async {
        sqlx::query(
            "INSERT INTO ai_audit_log (id, owner_id, client_id, action, target, status, error_code) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(owner_id)
        .bind(client_id)
        .bind(action)
        .bind(target)
        .bind(status)
        .bind(error_code)
        .execute(pool)
        .await?;
        Ok(())
    })
}

pub fn recent_audit(
    pool: &SqlitePool,
    owner_id: &str,
    limit: i64,
) -> Result<Vec<serde_json::Value>, sqlx::Error> {
    crate::db::block_on(async {
        let rows = sqlx::query(
            "SELECT client_id, action, target, status, error_code, created_at \
             FROM ai_audit_log WHERE owner_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )
        .bind(owner_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "client_id": row.get::<String, _>("client_id"),
                    "action": row.get::<String, _>("action"),
                    "target": row.get::<String, _>("target"),
                    "status": row.get::<String, _>("status"),
                    "error_code": row.try_get::<Option<String>, _>("error_code").ok().flatten(),
                    "created_at": row.get::<String, _>("created_at"),
                })
            })
            .collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_is_idempotent_and_expires_pending() {
        let pool = crate::db::test_pool();
        let job = enqueue(
            &pool,
            &NewAiJob {
                client_id: "client",
                action: "download",
                target: "techbookfest:123",
                params_json: r#"{"site_id":"techbookfest","database_id":"123"}"#,
                idempotency_key: Some("idem-1"),
                owner_id: "owner",
                expires_in_seconds: Some(60),
            },
        )
        .unwrap();
        assert_eq!(job.status, status::PENDING_CONFIRMATION);
        assert!(job.expires_at.is_some());

        let again = enqueue(
            &pool,
            &NewAiJob {
                client_id: "client",
                action: "download",
                target: "techbookfest:123",
                params_json: r#"{"site_id":"techbookfest","database_id":"123"}"#,
                idempotency_key: Some("idem-1"),
                owner_id: "owner",
                expires_in_seconds: Some(60),
            },
        )
        .unwrap();
        assert_eq!(again.id, job.id);

        crate::db::block_on(async {
            sqlx::query(
                "UPDATE ai_jobs SET expires_at = datetime('now', '-1 second') WHERE id = ?1",
            )
            .bind(&job.id)
            .execute(&pool)
            .await
            .unwrap();
        });
        assert_eq!(expire_stale(&pool).unwrap(), 1);
        let expired = get(&pool, &job.id).unwrap().unwrap();
        assert_eq!(expired.status, status::EXPIRED);
    }

    #[test]
    fn params_match_detects_tampering() {
        let pool = crate::db::test_pool();
        let job = enqueue(
            &pool,
            &NewAiJob {
                client_id: "client",
                action: "download",
                target: "techbookfest:123",
                params_json: r#"{"site_id":"techbookfest","database_id":"123"}"#,
                idempotency_key: None,
                owner_id: "",
                expires_in_seconds: Some(60),
            },
        )
        .unwrap();
        assert!(params_match(&job));
        let mut tampered = job.clone();
        tampered.params_json = r#"{"site_id":"techbookfest","database_id":"999"}"#.to_string();
        assert!(!params_match(&tampered));
    }

    #[test]
    fn pending_lists_only_confirmation_jobs() {
        let pool = crate::db::test_pool();
        let job = enqueue(
            &pool,
            &NewAiJob {
                client_id: "client",
                action: "open_book",
                target: "b1",
                params_json: r#"{"book_id":"b1"}"#,
                idempotency_key: None,
                owner_id: "",
                expires_in_seconds: Some(60),
            },
        )
        .unwrap();
        assert_eq!(pending(&pool).unwrap().len(), 1);
        set_status(&pool, &job.id, status::QUEUED, 0.0, None).unwrap();
        assert!(pending(&pool).unwrap().is_empty());
    }
}
