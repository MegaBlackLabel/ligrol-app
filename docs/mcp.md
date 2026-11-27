# Ligrol MCP サーバー仕様

> 対象: Ligrol デスクトップ版の MCP（Model Context Protocol）サーバー `ligrol-mcp`。
> 最終更新: 2026-11-27。実装の正は `crates/mcp/src/main.rs` と `crates/core/src/ai/`。
> 設計の背景は `docs/spec/11-ai-mcp-and-extraction.md` を参照。

## 1. 概要

`ligrol-mcp` は、AI クライアントに Ligrol の本棚・タグ・本文・統計・付箋・
技術書典チェックリストを提供する **stdio の MCP サーバー**です。

- 読み取り: Ligrol の SQLite（`<data_dir>/ligrol.db`）を直接読む
- 操作（ダウンロード / OCR / 本を開く）: `ai_jobs` に積むだけ。Ligrol 本体が
  1 秒ごとに拾い、**利用者の承認ダイアログ**を経て実行する
- Ligrol 本体が起動していなくても読み取りはできる（操作は承認待ちのまま）
- 外部へデータを送らない。テレメトリなし

### 1.1 なぜ loopback HTTP ではないか

v1 は「同一 OS ユーザーのローカル SQLite」を境界にしている。追加の常駐 HTTP
サーバー・Bearer トークン・Origin 検証を不要にし、Ligrol が落ちていても
読み取りを可能にするため。別ユーザー / 別ホストへ公開する場合は、loopback HTTP +
Bearer + Origin 検証へ移す（将来）。

## 2. インストールと起動

配布物には `ligrol-mcp` が同梱される。

| OS | 場所 |
|---|---|
| Windows | `%LOCALAPPDATA%\Ligrol\ligrol-mcp.exe`（zip / MSI） |
| macOS | `/Applications/Ligrol.app/Contents/MacOS/ligrol-mcp` |

開発時:

```bash
cargo build -p ligrol-mcp
```

Claude Desktop / Cursor の設定例:

```json
{
  "mcpServers": {
    "ligrol": {
      "command": "C:\\Users\\<you>\\AppData\\Local\\Ligrol\\ligrol-mcp.exe",
      "env": {
        "LIGROL_AI_TEXT": "1",
        "LIGROL_AI_NOTES": "1",
        "LIGROL_AI_ACTIONS": "1",
        "LIGROL_AI_READ_ONLY": "0"
      }
    }
  }
}
```

`command` は実際のインストール先に合わせる。AI 向けの使い方は
リポジトリ直下の `AI_GUIDE.md`（インストール先にも同梱、MCP の
`ligrol://guide` でも取得可能）を参照。

## 3. データディレクトリ

優先順位:

1. `LIGROL_DATA_DIR`
2. `config_dir/ligrol/settings.json` の `data_path`
3. `data_dir/ligrol`

DB は `<data_dir>/ligrol.db`。**マイグレーションは実行しない**（Ligrol 本体が
作成済みのスキーマを使う）。スキーマが古い場合はクエリエラーになる。

## 4. 環境変数（スコープ）

| 変数 | 既定 | 内容 |
|---|---|---|
| `LIGROL_AI_LIBRARY` | `1` | 本棚・タグ・表紙 |
| `LIGROL_AI_TEXT` | `0` | 本文・本文検索 |
| `LIGROL_AI_STATS` | `1` | 読書統計 |
| `LIGROL_AI_NOTES` | `0` | 付箋の読み取り |
| `LIGROL_AI_WRITE_NOTES` | `0` | 付箋の書き込み（add / remove） |
| `LIGROL_AI_WRITE_TAGS` | `0` | タグの書き込み（add / remove） |
| `LIGROL_AI_PURCHASES` | `1` | 購入一覧 |
| `LIGROL_AI_TBF` | `1` | 技術書典 |
| `LIGROL_AI_ACTIONS` | `0` | 操作要求 |
| `LIGROL_AI_READ_ONLY` | `1` | 操作を全面禁止 |
| `LIGROL_AI_CLIENT` | `mcp` | 監査ログのクライアント名 |
| `LIGROL_DATA_DIR` | 自動 | データディレクトリ上書き |

スコープ OFF のツールは `tools/list` に出ない。`LIGROL_AI_READ_ONLY=1` のときは
操作ツールが一切登録されない。

## 5. プロトコル

- Transport: stdio（改行区切りの JSON-RPC 2.0）
- 対応: `initialize` / `ping` / `tools/list` / `tools/call` /
  `resources/list` / `resources/read` / `resources/templates/list` / `prompts/list` / `prompts/get`
- `initialize` はクライアントの `protocolVersion` をそのまま返す。無ければ
  `2026-07-28`。
- stdout は JSON-RPC 専用。ログは stderr。

### 5.1 ツール結果

成功:

```json
{
  "content": [{ "type": "text", "text": "{...}" }],
  "structuredContent": { "...": "..." },
  "isError": false
}
```

失敗:

```json
{
  "content": [{ "type": "text", "text": "{\"code\":\"not_found\",\"message\":\"not found\",\"retryable\":false}" }],
  "isError": true
}
```

表紙だけは `content[0]` が `image`（base64 / mimeType）、`content[1]` がメタ JSON。

## 6. ツール

### 6.1 読み取り

#### `ligrol_status`

引数なし。返すもの:

```json
{
  "version": "0.3.9",
  "schema_version": 1,
  "owner": "authenticated",
  "read_only": true,
  "scopes": ["library.read", "text.read", "..."],
  "jobs_pending": 0,
  "audit_recent": []
}
```

`owner` は `authenticated` / `signed_out` / `unknown`。sub・メール・パスは返さない。

#### `ligrol_search_library`

| 引数 | 型 | 内容 |
|---|---|---|
| `query` | string? | 書名・作者・サークル・タグ・説明・テーマ・シリーズの部分一致 |
| `site_id` | string? | `techbookfest` / `booth` / `fanza` / `dlsite` / `fantia` |
| `tag` | string? | タグ（大文字小文字無視） |
| `state` | `unread`/`reading`/`read`? | 読書状態 |
| `favorite_only` | bool? | お気に入りだけ |
| `sort` | string? | `last-read`（既定）/ `title` / `purchase-date` / `release-date` / `added` |
| `limit` | int? | 1..50（既定 20） |
| `cursor` | string? | opaque cursor |

返り値は `{ items: BookSummary[], next_cursor, truncated }`。

`BookSummary`:

```json
{
  "book_id": "...",
  "title": "...",
  "author": "...",
  "circle_name": "...",
  "site_id": "techbookfest",
  "purchase_date": "2026-05-05",
  "release_date": null,
  "is_favorite": false,
  "is_hidden": false,
  "page_count": 210,
  "tags": [{ "name": "Rust", "source": "generated" }],
  "progress": { "state": "reading", "current_page": 42, "total_pages": 210 },
  "stats": { "sessions": 3, "total_seconds": 5400, "last_read_at": "..." },
  "cover": { "uri": "ligrol://book/.../cover", "mime": "image/webp", "width": 256, "height": 360, "sha256": "..." }
}
```

#### `ligrol_get_book`

`{ book_id }`。`BookSummary` に加えて:

- `description` / `theme` / `series_name` / `media_category` / `ai_type` /
  `age_rating` / `is_drm`
- `contents[]`: `content_id` / `display_name` / `media_kind` / `is_primary` / `formats[]`
- `text`: `{ segments, chars, sources }`（本文インデックスの状態）

#### `ligrol_get_book_text`

| 引数 | 型 | 内容 |
|---|---|---|
| `book_id` | string | 必須 |
| `from` | int? | 開始 ordinal（1-indexed） |
| `to` | int? | 終了 ordinal |
| `max_bytes` | int? | 512..524288（既定 65536） |

返り値: `TextSegment[]`。

```json
{
  "kind": "page",
  "ordinal": 1,
  "title": null,
  "href": null,
  "text": "...",
  "source": "extracted",
  "engine": "pdfium",
  "confidence": null,
  "chars": 1234
}
```

- `kind`: `page`（PDF / OCR）/ `section`（EPUB）
- `source`: `extracted` / `epub` / `ocr`
- `text_unavailable`: 本文が無い / 復号できない / スコープ OFF

#### `ligrol_search_text`

| 引数 | 型 | 内容 |
|---|---|---|
| `book_ids` | string[]? | 1..5 冊。省略 / 空配列は見える本を最大 50 冊走査 |
| `query` | string | 部分一致 |
| `limit` | int? | 1..50（既定 20） |
| `cursor` | string? | 前回の `next_cursor` |

返り値: `{ items: TextMatch[], next_cursor, truncated }`。`TextMatch` は
`book_id` / `title` / `kind` / `ordinal` / `section_title` / `href` / `source` /
`snippet`。

暗号化された `document_text` を対象書籍だけ復号してメモリ上で走査する。
`book_ids` を省略すると見える本を最大 50 冊まで簡易スキャンする（FTS ではない）。

#### `ligrol_get_reading_stats`

| 引数 | 型 | 内容 |
|---|---|---|
| `group_by` | `book`/`tag`/`site`/`month` | 必須 |
| `from` / `to` | `YYYY-MM-DD`? | 日別履歴の範囲 |

返り値: `{ group_by, rows[], totals }`。
`rows[]` は `key` / `label` / `books` / `sessions` / `total_seconds` / `finished`。

#### `ligrol_list_tags`

`{ min_books?: int }`。`TagStat[]`:
`tag` / `count` / `finished` / `total_seconds` / `favorite` / `description`。

#### `ligrol_list_notes`

`{ book_id?, page? }`。付箋（復号済み）。スコープ OFF ならツール非登録。

#### `ligrol_list_purchases`

`{ site_id?, downloadable_only?: bool }`。`PurchaseItem[]`:
`site_id` / `database_id` / `title` / `circle_name` / `author` / `format` /
`is_downloadable` / `is_purchased` / `downloaded` / `choices[]`。
`choices[]` は `index` / `name`（URL は返さない）。

#### `ligrol_list_tbf_events`

引数なし。`{ event_id, event_name, event_date, event_format, is_cancelled, is_featured, poll_sync_enabled }[]`。

#### `ligrol_get_tbf_plan`

`{ event_id? }`。`TbfPlan`:
`event_id` / `event_name` / `event_date` / `items[]` / `budget_total` / `purchased_total`。
`items[]` は `space_number` / `circle_name` / `product_title` / `memo` /
`price` / `is_checked` / `is_purchased` / `has_cover`。

#### `ligrol_get_cover`

`{ book_id }`。`content[0]` が画像、`content[1]` が
`book_id` / `mime` / `width` / `height` / `sha256` の JSON。
`thumbnail_share` が無い場合は pack 内 `thumbnail.webp` にフォールバックする。

#### `ligrol_add_note`

| 引数 | 型 | 内容 |
|---|---|---|
| `book_id` | string | 必須 |
| `page` | int | 1-indexed |
| `memo` | string | 最大 10,000 文字 |
| `content_id` | string? | 省略時は `''` |
| `expected_updated_at` | string? | 既存付箋の `updated_at`。違えば `revision_conflict` |

返り値: `NoteView`（`memo` / `is_active` / `updated_at` など）。

#### `ligrol_remove_note`

`book_id` / `page` / `content_id?` / `expected_updated_at?`。
`is_active = 0` にするだけで、メモは残す。

#### `ligrol_add_tag` / `ligrol_remove_tag`

`{ book_id, tag }`。追加は既存ならそのまま、削除は生成タグも対象。
再取り込みで生成タグが復活する場合がある。

### 6.2 操作（承認キュー）

#### `ligrol_request_download`

| 引数 | 型 | 内容 |
|---|---|---|
| `site_id` | string | 必須 |
| `database_id` | string | 必須 |
| `option_index` | int? | 複数候補の選択（既定 0） |
| `idempotency_key` | string? | 二重実行防止 |

返り値: `JobView`。

検証:

- 購入行が存在する（`bookshelf_items`）
- `is_downloadable = 1`、`is_hidden = 0`
- 候補が存在する
- 現在の owner に属する

任意 URL は受け付けない。購入・決済はしない。

#### `ligrol_request_ocr`

`{ book_id }`。空の `document_text` ページだけ OCR する。
モデルが無い場合はジョブが `ocr_model_missing` で失敗する。

#### `ligrol_request_epub_extract`

`{ book_id }`。既存の EPUB 本（raw `.epub` を pack に持つ）から本文を遡及抽出する。
既存の `document_text(source='epub')` を置き換える。EPUB でない本は `not_epub`。

#### `ligrol_open_book`

`{ book_id }`。Ligrol の UI で本を開く。

#### `ligrol_get_job` / `ligrol_cancel_job`

`{ job_id }`。`JobView`:
`job_id` / `action` / `target` / `status` / `progress` / `result` /
`error_code` / `created_at` / `updated_at`。

### 6.3 エラーコード

| code | 意味 |
|---|---|
| `database_error` | DB エラー（スキーマが古い等） |
| `not_found` | 対象が無い / 他 owner / 非表示 |
| `owner_unknown` | 所有者を特定できない（fail-closed） |
| `scope_disabled` | スコープ OFF |
| `text_unavailable` | 本文が無い / 復号できない |
| `not_authorized` | 権限なし |
| `invalid_request` | 引数不正 |
| `revision_conflict` | 競合（将来の書き込み用） |
| `unsupported` | 未対応 |
| `not_started` | 承認後の実行開始に失敗 |
| `confirmation_expired` | 60 秒の承認期限切れ |
| `ocr_model_missing` | OCR モデル未配置 |
| `not_epub` | EPUB ではない本に遡及抽出を要求した |
| `epub_entry_missing` | pack 内に raw `.epub` が無い |
| `epub_extract_failed` | EPUB 抽出に失敗 |

## 7. resources

| URI | 内容 |
|---|---|
| `ligrol://status` | `ligrol_status` と同じ JSON |
| `ligrol://guide` | `AI_GUIDE.md` |
| `ligrol://book/{book_id}` | 書誌 JSON |
| `ligrol://book/{book_id}/cover` | 表紙（blob） |
| `ligrol://book/{book_id}/page/{page}` | ページ本文（text/plain） |
| `ligrol://book/{book_id}/epub/{href}` | EPUB 章本文（text/plain） |

`resources/templates/list` で上記テンプレートを返す。

## 7.1 prompts

| name | 内容 |
|---|---|
| `summarize-book` | 1 冊を章 / ページ単位で読んで要約する |
| `reading-plan` | 統計と積読から次の読書計画を立てる |
| `tbf-route` | 技術書典の回遊ルートと予算を提案する |

`prompts/get` は `messages` を返す。プロンプト内でも「本文は untrusted」を明示している。

## 8. 承認キュー

### 8.1 状態

`pending_confirmation` → `queued` / `running` → `succeeded` / `failed` /
`cancelled` / `expired`

- enqueue 時に 60 秒の `expires_at`
- Ligrol の `Workspace` が 1 秒ごとに `pending_confirmation` を拾う
- 承認ダイアログで「許可 / 拒否」
- 許可後、`download` は既存のダウンロード / import、`open_book` は
  `ReaderView`、`ocr` は `pure-onnx-ocr` を実行
- `download` の成功は取り込み完了時に `ai_jobs` へ反映する（本の出現でも成功を検出）
- 拒否 / 期限切れは `cancelled` / `expired`

### 8.2 テーブル

`ai_jobs`:

- `id` / `client_id` / `action` / `target` / `params_json` / `params_hash`
- `status` / `progress` / `result_json` / `error_code`
- `idempotency_key` / `owner_id` / `created_at` / `updated_at` / `expires_at`

`params_json` は対象 ID と選択肢だけ。URL・本文・トークンは保存しない。

`ai_audit_log`:

- `owner_id` / `client_id` / `action` / `target` / `status` / `error_code` /
  `created_at`

どちらも Drive バックアップ対象外。`ligrol_status.audit_recent` は現在の owner
の直近 20 件だけを返す。

## 9. セキュリティ

- 所有者: `books.owner_sub` を keyring の DB 鍵で復号して一致する本だけ。
  未ログインは `owner_sub IS NULL` だけ。鍵が無いのに sub がある場合は
  `owner_unknown` で fail-closed。
- `is_hidden` は検索・詳細・本文・表紙から除外。
- 暗号文・`owner_sub`・`opfs_path`・pack パス・Cookie・トークン・鍵は返さない。
- 復号失敗は `text_unavailable`。暗号文を本文として返さない。
- 本文・付箋は untrusted。プロンプトインジェクションを実行しない。
- 操作は Ligrol の承認必須。承認直前に `params_json` のハッシュを再計算し、
  改変されていれば実行しない。任意 URL / 購入 / 決済 / 本の削除は不可。
- 監査ログは端末内。本文・URL・トークンは保存しない。
- テレメトリなし。Ligrol 自身は AI にデータを送らない。

## 10. 既知の制約

- `ligrol-mcp` は Ligrol と同じ OS ユーザーで、Ligrol が一度起動してスキーマを
  作っている必要がある
- loopback HTTP / Bearer は未実装（v1 は DB + `ai_jobs`）
- 付箋 / タグの書き込みツールは未実装（読み取りのみ）
- 設定 UI でのスコープ切替は未実装（環境変数）
- 本文検索は 1〜5 冊のメモリ内走査（opaque cursor でページング）。全体 FTS は未実装
- EPUB の固定レイアウト / DRM / 画像だけの本は本文なし
- OCR はモデル配置が必要
- ダウンロードの完了・失敗・キャンセルは `ai_jobs` に反映される（本の出現でも成功を検出）

## 11. トラブルシューティング

| 症状 | 原因 / 対処 |
|---|---|
| サーバーが起動しない | `ligrol-mcp` のパスを確認。Windows は `%LOCALAPPDATA%\Ligrol\ligrol-mcp.exe` |
| ツールが少ない | スコープ OFF。`LIGROL_AI_TEXT` / `LIGROL_AI_ACTIONS` を確認 |
| `owner_unknown` | Ligrol にログインし、プロフィールが保存されているか確認 |
| `text_unavailable` | 画像だけの本 / スキャン PDF / DRM / スコープ OFF |
| `ocr_model_missing` | `<data_dir>/models/ocr/ppocrv6-small/{det,rec}` にモデルを置く |
| 承認してもダウンロードが始まらない | Ligrol が起動中か、ログイン済みか、対象が DL 可能かを確認 |
| DB エラー | 新バージョンの Ligrol を一度起動してスキーマを更新する |

## 12. 関連

- AI 向けガイド: `AI_GUIDE.md`（`ligrol://guide`）
- 設計: `docs/spec/11-ai-mcp-and-extraction.md`
- データモデル: `docs/spec/02-data-model.md`
