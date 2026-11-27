//! OCR（任意機能）。
//!
//! 空の `document_text` ページだけを `pure-onnx-ocr`（PP-OCRv6 small）で埋める。
//! モデルは `<data_dir>/models/ocr/ppocrv6-small/{det,rec}/inference.onnx` に置く。
//! モデルが無い場合は `ModelMissing` を返し、本の取り込みや閲覧には影響しない。
//!
//! OCR 結果は `document_text` に `source='ocr'` / `engine` / `confidence` で保存する。
//! 既存の PDF / EPUB 本文（空でない行）は上書きしない。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use pure_onnx_ocr::OcrEngineBuilder;

use crate::db::{SqlitePool, books, column_crypto, contents, documents};
use crate::pack_path::pack_path;

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    #[error("OCR model is missing: {0}")]
    ModelMissing(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("pack error: {0}")]
    Pack(#[from] nartik::PackError),
    #[error("image error: {0}")]
    Image(String),
    #[error("ocr error: {0}")]
    Engine(String),
    #[error("book has no page images")]
    NoPages,
    #[error("cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrOutcome {
    pub pages: i64,
    pub chars: i64,
    /// 既に本文があり、OCR をスキップしたページ数。
    pub skipped: i64,
}

fn model_ready(model_dir: &Path) -> bool {
    ["det", "rec"].iter().all(|kind| {
        let dir = model_dir.join(kind);
        dir.join("inference.onnx").is_file() && dir.join("inference.yml").is_file()
    })
}

/// 空のページだけ OCR する。`progress` は 0..1、`cancel` が立ったら中止。
pub fn ocr_book(
    pool: &SqlitePool,
    packs_dir: &Path,
    book_id: &str,
    root_key: Option<&nartik::PackRootKey>,
    model_dir: &Path,
    progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<OcrOutcome, OcrError> {
    if !model_ready(model_dir) {
        return Err(OcrError::ModelMissing(model_dir.display().to_string()));
    }
    let book = books::get(pool, book_id)?.ok_or(OcrError::NoPages)?;
    let document = documents::get_document_by_book_id(pool, book_id)?.ok_or(OcrError::NoPages)?;
    let content_id = contents::primary_for_book(pool, book_id)?
        .map(|content| content.content_id)
        .unwrap_or_default();
    let key = column_crypto::db_key()?;
    let existing = documents::all_text_rows_for_document(pool, &document.id, Some(&key))?;
    let with_text: std::collections::HashSet<i64> = existing
        .iter()
        .filter(|row| {
            row.text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        })
        .map(|row| row.page_number)
        .collect();

    let images: Vec<documents::DocumentImage> = documents::images_for_book(pool, book_id)?
        .into_iter()
        .filter(|image| image.image_type == "page" && image.pack_entry_path.is_some())
        .collect();
    if images.is_empty() {
        return Err(OcrError::NoPages);
    }

    let engine = OcrEngineBuilder::new()
        .det_model_dir(model_dir.join("det"))
        .rec_model_dir(model_dir.join("rec"))
        .build()
        .map_err(|error| OcrError::Engine(error.to_string()))?;

    let pack_id = book.pack_id.as_deref().unwrap_or(&book.id);
    let path = pack_path(packs_dir, pack_id).map_err(|error| OcrError::Image(error.to_string()))?;
    let reader = nartik::PackFileReader::open(&path)?;
    let pack_key = root_key.map(|root| root.derive_pack_key(pack_id));

    let mut rows = Vec::new();
    let mut skipped = 0i64;
    let mut chars = 0i64;
    let total = images.len();
    for (index, image) in images.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return Err(OcrError::Cancelled);
        }
        if with_text.contains(&image.page_number) {
            skipped += 1;
            progress((index + 1) as f32 / total as f32);
            continue;
        }
        let Some(entry) = image.pack_entry_path.as_deref() else {
            continue;
        };
        let bytes = reader.read_entry_with_key(entry, pack_key.as_ref())?;
        let decoded =
            image::load_from_memory(&bytes).map_err(|error| OcrError::Image(error.to_string()))?;
        let lines = engine
            .run_from_image(&decoded)
            .map_err(|error| OcrError::Engine(error.to_string()))?;
        let text = lines
            .iter()
            .map(|line| line.text.trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            progress((index + 1) as f32 / total as f32);
            continue;
        }
        let confidence = if lines.is_empty() {
            None
        } else {
            Some(lines.iter().map(|line| line.confidence as f64).sum::<f64>() / lines.len() as f64)
        };
        chars += text.chars().count() as i64;
        rows.push(documents::DocumentText {
            id: uuid::Uuid::new_v4().to_string(),
            document_id: document.id.clone(),
            content_id: content_id.clone(),
            page_number: image.page_number,
            text_content: text,
            source: "ocr".to_string(),
            engine: Some("pure-onnx-ocr/ppocrv6".to_string()),
            confidence,
            section_title: None,
            section_href: None,
            created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        });
        progress((index + 1) as f32 / total as f32);
    }

    if !rows.is_empty() {
        // 再実行で同じページが重複しないよう、この document の OCR 行だけ入れ替える。
        crate::db::block_on(async {
            sqlx::query("DELETE FROM document_text WHERE document_id = ?1 AND source = 'ocr'")
                .bind(&document.id)
                .execute(pool)
                .await?;
            Ok::<_, sqlx::Error>(())
        })?;
        // 同じページを再 OCR したときに形態素が重複しないよう、対象ページの
        // token_analysis だけ入れ替える（抽出済みページのトークンは残す）。
        let pages: Vec<i64> = rows.iter().map(|row| row.page_number).collect();
        crate::db::block_on(async {
            for page in &pages {
                sqlx::query(
                    "DELETE FROM token_analysis WHERE document_id = ?1 AND page_number = ?2",
                )
                .bind(&document.id)
                .bind(page)
                .execute(pool)
                .await?;
            }
            Ok::<_, sqlx::Error>(())
        })?;
        let mut tokens = Vec::new();
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        for row in &rows {
            for (word, count) in crate::tags::extract_nouns(&row.text_content, &[&book.title]) {
                tokens.push(documents::TokenRow {
                    id: uuid::Uuid::new_v4().to_string(),
                    document_id: document.id.clone(),
                    page_number: row.page_number,
                    token: word,
                    pos: "名詞".to_string(),
                    base_form: None,
                    reading: None,
                    frequency: count as i64,
                    created_at: timestamp.clone(),
                });
            }
        }
        if !tokens.is_empty() {
            documents::insert_tokens_batch(pool, &tokens)?;
        }
        documents::insert_texts_batch(pool, &rows)?;
    }

    Ok(OcrOutcome {
        pages: rows.len() as i64,
        chars,
        skipped,
    })
}

/// モデルの置き場所（アプリのデータディレクトリ基準）。
pub fn default_model_dir(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("models").join("ocr").join("ppocrv6-small")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_models_are_reported_without_touching_db() {
        let pool = crate::db::test_pool();
        let dir = std::env::temp_dir().join(format!("ligrol-ocr-{}", uuid::Uuid::new_v4()));
        let mut progress = |_| {};
        let cancel = AtomicBool::new(false);
        let result = ocr_book(&pool, &dir, "missing", None, &dir, &mut progress, &cancel);
        assert!(matches!(result, Err(OcrError::ModelMissing(_))));
    }
}
