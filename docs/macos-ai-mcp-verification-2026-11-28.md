# macOS AI/MCP 実機確認（2026-11-28）

対象: `main` (`9c35297`) / macOS 27.2.1 (arm64) / Rust 1.98.1。
CI（macos-latest）では拾いにくい実機依存を確認した記録。検証の一部は隔離データディレクトリで行い、実データはユーザー確認のうえ復元した。

## 結果サマリ

| 領域 | 結果 |
|---|---|
| ビルド前提 | ✅ テスト・clippy・release build とも通過。PDF テストも skip なし |
| MCP stdio / scopes | ✅ default 9 / checklist env 18 / read-only 12 / write 含む全 22。全ツールには `WRITE_NOTES` / `WRITE_TAGS` が必要 |
| Keychain | ⚠️ 署名済みリリースは無言でプロフィール復元。ad-hoc ビルドは項目ごとにプロンプト→拒否で `signed_out` |
| 承認キュー | ✅ 1秒表示 / 拒否 / 60秒 expire / 改ざん検知 / 冪等性を実機確認 |
| EPUB / OCR | ✅ EPUB と OCR モデル無し経路はテストで確認。モデル有りは未実施 |
| 配布物 | ⚠️ v0.3.9 は AI MCP 前で `ligrol-mcp` / `AI_GUIDE.md` なし。ローカル `.app` は同梱確認 |
| UI | ⚠️ About 画面・タイトルバー・信号機は目視。他の操作は未実施 |
| 注意 | ⚠️ Drive 復元プロンプトで実DBの冊数が変化する事象を確認。再確認は隔離データで行う |

## 0. ビルド前提

- macOS 27.2.1 (arm64) / Rust 1.98.1
- `mise run pdfium` → SHA256 検証 OK、`crates/core/libpdfium.dylib` を配置
- `cargo test --workspace --locked` → pass。PDF テストも実行
  （`pdf_fixture_imports_pages_text_and_db_rows`、`pdf_rendering_*`、
  `render_pdf_pages_into_*` など）
- `cargo clippy --workspace --locked --all-targets -- -D warnings` → pass
- `cargo build --release --locked -p ligrol -p ligrol-mcp` → pass、両バイナリ生成
- `Info.plist` の `LSMinimumSystemVersion` は 13.0（`release.yml` / `make-app-bundle.sh`）

## 1. MCP サーバー

- `initialize` + `tools/list` の stdout は JSON-RPC のみ（2行）、stderr 0行。
  すべての出力行が JSON としてパースできる。
- `initialize` の `protocolVersion` は `2026-07-28`。
- ツール数:

| 環境 | tools |
|---|---|
| 既定 | 9 |
| `LIGROL_AI_TEXT=1 LIGROL_AI_NOTES=1 LIGROL_AI_ACTIONS=1 LIGROL_AI_READ_ONLY=0` | 18（write 4つは出ない） |
| 上記 + `LIGROL_AI_WRITE_NOTES=1 LIGROL_AI_WRITE_TAGS=1` | 22 |
| `LIGROL_AI_READ_ONLY=1`（text/notes/actions=1） | 12（write/actions なし） |

- `resources/list` は `ligrol://status` と `ligrol://guide` を返す。
  `ligrol://guide` は `AI_GUIDE.md` と byte 一致。
- 範囲外ツールは `isError: true` + `{code, message, retryable}`。
- 不正 URI は JSON-RPC error `-32602`（`isError` 形式ではない）。
- Claude Desktop の設定ファイルはこの環境には無く、未設定。

## 2. Keychain と所有者境界

- Keychain には `ligrol.db-key` / `ligrol.session-key` / `google` / `google-profile` /
  `ligrol.pack-root-key:<owner>` が存在。
- 署名済み v0.3.9 リリースアプリ: Keychain プロンプトなしで
  `google profile: 起動時復元 = あり`。`google` / `google-profile` /
  `pack-root-key` / `techbookfest` を読めた。
- ローカル ad-hoc ビルド: Keychain 項目ごとにログインキーチェーンのパスワードを聞かれる。
  拒否すると `session vault` を初期化できず、アプリ/MCP は `signed_out` になる
  （無言で成功はしない）。
- 画面ロック中（dark wake）: `Platform failure: In dark wake, no UI possible` で
  Keychain が読めず、プロンプトも出ずに `signed_out`。
- `signed_out` の MCP は所有本を返さず fail-closed（`owner_sub IS NULL` のみ）。
  ログイン中の MCP 所有者境界は ad-hoc MCP の Keychain プロンプトのため未確認。
- `connect_for_ai` は WAL + busy_timeout 5秒。アプリの1秒ポーリング中でも
  MCP の status / search は成功した。
- `owner_unknown` / `is_hidden` の境界はコードとテストで確認。実データでの
  シミュレーションは未実施（検証データに hidden は無し）。

## 3. 承認キュー

隔離データベースと実アプリで確認。

- 1秒ポーリング: アプリの watcher が `pending_confirmation` を 1秒以内に取得。
- ダイアログ: 「AI からの操作要求」「60秒で期限切れ」「拒否 / 許可」を実機で確認。
- 拒否: job は `cancelled` / `error_code=denied`、audit も `cancelled/denied`。
- 改ざん検知: 正しい `params_hash` で enqueue 後に `params_json` を書き換え、
  「許可」→ `failed` / `invalid_request`。本の取り込みは実行されなかった。
- 60秒放置: `expired` / `confirmation_expired`。アプリの `expire_stale` が
  `rows_affected=1` で更新。
- 冪等性: 同じ `idempotency_key` の二重送信は同じ job_id、DB は1行。
- `ai_audit_log` は `owner_id/client_id/action/target/status/error_code` のみで、
  本文・URL・トークンの列を持たない。実データも `download/pending_confirmation/...`
  のような action/status/target だけ。
- 未実施: ログイン中の承認、`ligrol_open_book` / OCR / EPUB 遡及抽出の承認成功。

## 4. EPUB / OCR

- EPUB: spine 順・章タイトル・ルビ、`document_text` の `source=epub` /
  `engine=epub-xhtml`、本文検索と章取得のテストが pass。
- OCR: モデル無しで `ocr_model_missing` になり、本を壊さないテストが pass。
- 実データの `document_text` は `extracted` のみ。EPUB / OCR の実データは未確認。
- OCR モデル（`models/ocr/ppocrv6-small/{det,rec}`）は未配置。モデル有りの経路は未実施。
- 縦書きは PP-OCR の対象外（仕様どおり）。

## 5. 配布物 / Gatekeeper

- ローカル `.app`（修正後の `scripts/make-app-bundle.sh`）:
  - `Contents/MacOS/ligrol-mcp`（strip 後 25MB）と `Contents/Resources/AI_GUIDE.md` を同梱
  - `codesign --verify --deep --strict` は valid
  - sidecar の `LC_SYMTAB.stroff % 8 == 0`
- v0.3.9 のリリース zip:
  - `Ligrol.app/Contents/MacOS/ligrol-mcp` と `AI_GUIDE.md` は **無い**（AI MCP マージ前）
  - `codesign --verify` valid / `spctl` は Notarized Developer ID で accepted /
    `xcrun stapler validate` OK
  - `Contents/Frameworks/libpdfium.dylib` は同梱
- AI MCP を含む新リリースが必要（v0.3.9 は対象外）。
- quarantine / `xattr -dr` の実機確認は未実施。

## 6. macOS UI

- About 画面、タイトルバー、信号機（赤/黄/緑）、サイドバー、アプリメニューを目視確認。
- ライセンスデータは `quick-xml 0.41.0` を含み、0.38.4 は無い。
- 未実施: 信号機のクリック、タイトルバードラッグ、アップロード中の終了無効、
  ナチュラルスクロール、ライセンス画面のスクロール。

## 7. 注意: Drive 復元プロンプト

署名済み v0.3.9 リリースアプリを実データで起動したところ、Google プロフィールの
復元後に「Drive バックアップが見つかりました。復元しますか？ 既存のデータは Drive 側の
内容で上書きされます」というプロンプトが表示された。

- アクセシビリティ（System Events）ではダイアログのボタン名が取得できず、
  「キャンセル」を確実に押せなかった。
- その結果、実DBの冊数が変化する事象が発生した。アプリを停止し、
  ユーザー確認のうえ実DBを復元した。
- **Drive 復元の確認は隔離データディレクトリで行う**こと。実DBで試す場合は、
  復元される側のバックアップを事前に取得する。
- ダイアログのアクセシビリティ（ボタン名）と、キャンセル時のデフォルト動作は
  別途調査・改善の候補。

## 8. 未実施 / 次の確認

- 署名済み `ligrol-mcp` でのログイン中所有者境界（ad-hoc MCP は Keychain プロンプトになる）
- 承認成功（`succeeded`）と `ligrol_open_book` / OCR / EPUB 遡及抽出の承認
- OCR モデル配置後の空ページ OCR
- 信号機・タイトルバードラッグ・アップロード中終了無効・ナチュラルスクロールの実操作
- AI MCP を含む新リリース（`ligrol-mcp` / `AI_GUIDE.md` 同梱）

## 9. 今回の修正

- `docs/mcp.md`: 付箋/タグ書き込みは実装済みであることを明記。全ツールには
  `LIGROL_AI_WRITE_NOTES` / `LIGROL_AI_WRITE_TAGS` も必要と追記。
- `docs/spec/11-ai-mcp-and-extraction.md`: OCR モデルの既定パスを
  macOS / Linux / Windows 併記に修正。
- `scripts/make-app-bundle.sh`: ローカル `.app` に `ligrol-mcp` と
  `AI_GUIDE.md` を同梱し、sidecar も `strip -x` するように修正。
