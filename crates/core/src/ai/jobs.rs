//! 操作要求（承認キュー）。実行は Ligrol 本体が行う。

use serde::Serialize;

use crate::ai::{AiContext, AiError, JobView, purchases};
use crate::db::ai_jobs::{self, NewAiJob, status};
use crate::db::{SqlitePool, books};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownloadRequest<'a> {
    pub site_id: &'a str,
    pub database_id: &'a str,
    pub option_index: Option<usize>,
    pub idempotency_key: Option<&'a str>,
}

pub fn request_download(
    pool: &SqlitePool,
    ctx: &AiContext,
    client_id: &str,
    request: &DownloadRequest<'_>,
) -> Result<JobView, AiError> {
    ctx.require_actions()?;
    if request.site_id.trim().is_empty() || request.database_id.trim().is_empty() {
        return Err(AiError::Invalid("site_id/database_id".to_string()));
    }
    let Some(item) = purchases::find_purchase(pool, ctx, request.site_id, request.database_id)?
    else {
        ai_jobs::audit(
            pool,
            &ctx.owner_id(),
            client_id,
            "download",
            &format!("{}:{}", request.site_id, request.database_id),
            status::FAILED,
            Some("not_found"),
        )?;
        return Err(AiError::NotFound);
    };
    if item.is_hidden != 0 {
        return Err(AiError::NotFound);
    }
    if item.is_downloadable == 0 {
        return Err(AiError::Unsupported("not_downloadable".to_string()));
    }
    let choices = item.download_choices();
    if choices.is_empty() {
        return Err(AiError::Unsupported("no_download_choices".to_string()));
    }
    let option_index = request.option_index.unwrap_or(0);
    if option_index >= choices.len() {
        return Err(AiError::Invalid("option_index".to_string()));
    }
    let params = serde_json::json!({
        "site_id": request.site_id,
        "database_id": request.database_id,
        "option_index": option_index,
    })
    .to_string();
    let target = format!("{}:{}", request.site_id, request.database_id);
    let job = ai_jobs::enqueue(
        pool,
        &NewAiJob {
            client_id,
            action: "download",
            target: &target,
            params_json: &params,
            idempotency_key: request.idempotency_key,
            owner_id: &ctx.owner_id(),
            expires_in_seconds: Some(60),
        },
    )?;
    ai_jobs::audit(
        pool,
        &ctx.owner_id(),
        client_id,
        "download",
        &target,
        status::PENDING_CONFIRMATION,
        None,
    )?;
    Ok(job.into())
}

pub fn request_ocr(
    pool: &SqlitePool,
    ctx: &AiContext,
    client_id: &str,
    book_id: &str,
) -> Result<JobView, AiError> {
    ctx.require_actions()?;
    let visible = ctx.visible_book_ids(pool)?;
    if !visible.contains(book_id) {
        return Err(AiError::NotFound);
    }
    let book = books::get(pool, book_id)?.ok_or(AiError::NotFound)?;
    if book.is_hidden != 0 {
        return Err(AiError::NotFound);
    }
    let params = serde_json::json!({ "book_id": book_id }).to_string();
    let job = ai_jobs::enqueue(
        pool,
        &NewAiJob {
            client_id,
            action: "ocr",
            target: book_id,
            params_json: &params,
            idempotency_key: None,
            owner_id: &ctx.owner_id(),
            expires_in_seconds: Some(60),
        },
    )?;
    ai_jobs::audit(
        pool,
        &ctx.owner_id(),
        client_id,
        "ocr",
        book_id,
        status::PENDING_CONFIRMATION,
        None,
    )?;
    Ok(job.into())
}

pub fn request_open_book(
    pool: &SqlitePool,
    ctx: &AiContext,
    client_id: &str,
    book_id: &str,
) -> Result<JobView, AiError> {
    ctx.require_actions()?;
    let visible = ctx.visible_book_ids(pool)?;
    if !visible.contains(book_id) {
        return Err(AiError::NotFound);
    }
    let book = books::get(pool, book_id)?.ok_or(AiError::NotFound)?;
    if book.is_hidden != 0 {
        return Err(AiError::NotFound);
    }
    let params = serde_json::json!({ "book_id": book_id }).to_string();
    let job = ai_jobs::enqueue(
        pool,
        &NewAiJob {
            client_id,
            action: "open_book",
            target: book_id,
            params_json: &params,
            idempotency_key: None,
            owner_id: &ctx.owner_id(),
            expires_in_seconds: Some(60),
        },
    )?;
    ai_jobs::audit(
        pool,
        &ctx.owner_id(),
        client_id,
        "open_book",
        book_id,
        status::PENDING_CONFIRMATION,
        None,
    )?;
    Ok(job.into())
}

fn owned_job(pool: &SqlitePool, ctx: &AiContext, job_id: &str) -> Result<ai_jobs::AiJob, AiError> {
    let job = ai_jobs::get(pool, job_id)?.ok_or(AiError::NotFound)?;
    if job.owner_id != ctx.owner_id() {
        return Err(AiError::NotFound);
    }
    Ok(job)
}

pub fn get_job(pool: &SqlitePool, ctx: &AiContext, job_id: &str) -> Result<JobView, AiError> {
    owned_job(pool, ctx, job_id).map(JobView::from)
}

pub fn cancel_job(
    pool: &SqlitePool,
    ctx: &AiContext,
    client_id: &str,
    job_id: &str,
) -> Result<JobView, AiError> {
    let job = owned_job(pool, ctx, job_id)?;
    if status::is_terminal(&job.status) {
        return Err(AiError::Conflict);
    }
    ai_jobs::set_status(pool, &job.id, status::CANCELLED, job.progress, None)?;
    ai_jobs::audit(
        pool,
        &ctx.owner_id(),
        client_id,
        &job.action,
        &job.target,
        status::CANCELLED,
        None,
    )?;
    get_job(pool, ctx, job_id)
}

/// UI が承認待ちを拾うための一覧（owner で絞る）。
pub fn pending_for_owner(pool: &SqlitePool, owner_id: &str) -> Result<Vec<JobView>, AiError> {
    Ok(ai_jobs::pending(pool)?
        .into_iter()
        .filter(|job| job.owner_id == owner_id)
        .map(JobView::from)
        .collect())
}
