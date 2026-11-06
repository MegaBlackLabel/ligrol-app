# Ligrol / nartik リネーム状況

このファイルは **マシンとエージェントをまたぐ引き継ぎ用の生きているドキュメント**。
Windows / macOS / Linux のどこから作業しても、ここを読めば現在地が分かるように保つ。
作業のたびに「進捗」と「触ったファイル」を更新すること。

- 最終更新: 2026-11-05
- ブランチ: `rename/ligrol-nartik`
- 状態: **Phase 1（アプリ版）完了・検証済み。次は GCP → Web**

## 1. 決定事項

| 項目 | 決定 |
|---|---|
| アプリ名 | **Ligrol**（カタカナ表記: リグロル） |
| アプリ名の由来 | ヘブライ語 לגלול（liglol＝巻物をほどく）の l→r 変異 |
| 形式名 | **nartik** |
| 形式名の由来 | ヘブライ語 נרתיק（＝ケース／鞘）をそのまま使用。**変異なし** |
| nartik の公式な読み | **公開しない**（ドキュメント・ストア・README にカタカナを書かない） |
| 由来の記載場所 | 仕様書の冒頭1箇所のみ。カタカナ禁止（`ナルティーク` は医薬品ブランドと衝突する） |
| 既存ユーザー | ゼロ。**データ移行・互換読みは不要**。両側を同一リリース窓で切り替える |
| git 履歴 | 残す（旧名は履歴に残る） |
| CHANGELOG | 過去エントリは書き換えない。改名エントリを1本追加する |
| `docs/naming.md` | 当分残す（**後日削除予定**） |
| GCP | 新プロジェクト `ligrol-app` を作り、旧 `thundoku-shelf` は削除 |
| OAuth | デスクトップ用 / Web用の **2クライアント**（同一プロジェクト内なら `drive.file` は共有される） |
| アイコン生成 | ImageMagick（librsvg 同梱）+ ICNS 生成スクリプト |

### 触ってはいけないもの

| 対象 | 理由 |
|---|---|
| DB 列 `opfs_path` | 据え置きと決定。意味は「pack ファイルのローカルパス」 |
| `FORMAT_VERSION = 3` | 据え置き。新旧の識別はマジックで行う |
| `MegaBlackLabel`（作者名・org 名） | 変更しない |
| `crates/app/assets/third-party/licenses.json` の中身 | 生成物。`mise run licenses` で再生成 |
| CI のジョブ名 / matrix 名 | ブランチ保護が名前で参照している |
| `CHANGELOG.md` の過去エントリ | 履歴。追記のみ |

## 2. 識別子マップ

| 旧 | 新 |
|---|---|
| crate / bin `thundoku-shelf` | `ligrol` |
| crate `thundoku-core` / `thundoku_core` | `ligrol-core` / `ligrol_core` |
| lib target `thundoku_shelf` | `ligrol` |
| 表示名 `Thundoku Shelf` | `Ligrol` |
| バンドル `ThundokuShelf.app` | `Ligrol.app` |
| `com.megablacklabel.thundoku-shelf` | `com.megablacklabel.ligrol` |
| データディレクトリ `thundoku-shelf` | `ligrol` |
| DB `thundoku-shelf.db` | `ligrol.db` |
| ログ `thundoku.log` | `ligrol.log` |
| keyring service | `com.megablacklabel.ligrol` |
| keyring slots | `ligrol.db-key` / `ligrol.session-key` / `ligrol.pack-root-key:<owner>` |
| AAD ラベル `thundoku-*` | `ligrol-*` |
| AAD `thundoku-shelf/column/v1` | `ligrol/column/v1` |
| AAD `thundoku-shelf/session/v2/*` | `ligrol/session/v2/*` |
| Drive フォルダ `thundoku-shelf` | `ligrol` |
| Drive ファイル `thundoku-{keys,backup,thumbs}.json` | `ligrol-{keys,backup,thumbs}.json` |
| `appProperties.app` = `thundoku-shelf` | `ligrol` |
| UA `ThundokuShelf/<ver>` | `Ligrol/<ver>` |
| UA `thundoku-shelf/<ver>` | `ligrol/<ver>` |
| 環境変数 `THUNDOKU_*` | `LIGROL_*` |
| crate `opfspack` | `nartik` |
| 拡張子 `.opfspack` | `.nartik` |
| マジック `OPFS` | `NRTK` |
| `opfspack-v1-identity-salt-2024` | `nartik-v1-identity-salt-2026` |
| `opfspack-entry-key` | `nartik-entry-key` |
| `opfspack:v1:<sub>` | `nartik:v1:<sub>` |
| notary プロファイル `thundoku-notary` | `ligrol-notary` |
| GitHub repo `thundoku-shelf-app` | `ligrol-app` |
| Cloudflare Worker `thundoku-shelf-staging` / `thundoku-shelf` | `ligrol-staging` / `ligrol` |
| `docs/thundoku-thumbs.jsonc` | `docs/ligrol-thumbs.jsonc` |
| アクション名前空間 `thundoku` | `ligrol` |

## 3. 進捗

| 段 | 内容 | 状態 |
|---|---|---|
| 0a | ブランチ作成・ベースライン記録 | 完了（2026-11-05） |
| 0b | このドキュメントの雛形 | 完了（2026-11-05） |
| 1 | 新契約の失敗テスト（`crates/core/tests/rename_contract.rs`） | 完了 |
| 2 | クレート・ビルド同一性 | 完了 |
| 3 | `opfspack`→`nartik`（crate/拡張子/マジック/定数） | 完了 |
| 4 | 暗号ラベル・keyring・データパス・ログ | 完了 |
| 5 | Drive 契約 | 完了 |
| 6 | UA・環境変数 | 完了 |
| 7 | パッケージング | 完了 |
| 8 | アイコン生成 | 完了 |
| 9 | UI 文言 | 完了 |
| 10 | ドキュメント | 完了 |
| 11 | 検証 | 完了（下記） |
| — | GCP 再作成（`ligrol-app`） | 完了（2026-11-05。クライアント2本・Picker キー・実機ログインまで確認） |
| — | Web 版リネーム | 完了（2026-11-06。staging デプロイ・旧 Worker 削除まで） |
| — | GitHub リポジトリ改名 | 完了（`ligrol` / `ligrol-app`） |

### Phase 1 の実施ログ（2026-11-05）

- 一括置換: `thundoku-shelf`→`ligrol`、`thundoku_core`→`ligrol_core`、`thundoku`→`ligrol`、
  `ThundokuShelf`/`Thundoku`→`Ligrol`、`THUNDOKU`→`LIGROL`、`opfspack`→`nartik`、`OPFS`→`NRTK`
  （`CHANGELOG.md` / `docs/naming.md` / このファイルは除外）
- ディレクトリ: `crates/opfspack` → `crates/nartik`
- マジック: `[0x4e, 0x52, 0x54, 0x4b]`（`NRTK`）
- ゴールデンベクタを再生成（仕様書 §10・§11 と `docs/ligrol-thumbs.jsonc`、
  `tests/{backup_envelope,keys_v3,thumbs_envelope,key_bundle}.rs` の期待値）
- パックのテストフィクスチャ: `plain.nartik` / `encrypted.nartik` をビルダで再生成
  （ページ本文の `LIGROL_PAGE_*` に伴いサイズも更新）
- アイコン: `scripts/make-icons.ps1`（Windows）と `scripts/make-icons.sh`（macOS/Linux）、
  `scripts/make-icns.py` を追加。`icon.icns` / `icon.ico` / `icon_{1024,512,256}.png` を再生成
- `APP_LOGO_RADIUS` を 5.12 → 12.38 に更新（新アイコンの白枠を実測）。
  テストは「白地に濃紺枠」前提から「濃紺地に白枠」前提へ書き換え
- 検証結果: `cargo build --workspace --locked` 成功、`cargo test --workspace` 全緑
  （525 + 362 + その他、失敗 0）、`mise run licenses` 差分なし、
  `git grep -Iin "thundoku|opfspack"` = 0（除外3ファイルを除く）

### 実機検証（2026-11-06 未明）

| 項目 | 結果 |
|---|---|
| アプリ起動 | `cargo run -p ligrol` で起動。タイトルバー・サイドバーとも `Ligrol` |
| データディレクトリ | `%APPDATA%\ligrol` を新規作成（`ligrol.db` / `logs/ligrol.log` / `packs`） |
| Google ログイン | **成功**（新プロジェクト `ligrol-app` のデスクトップ用クライアント。同意画面のアプリ名は Ligrol） |
| Drive 同期 | **成功**。My Drive 直下に `ligrol` フォルダ（id `1jeqk3gMSq2FepSFdVcjrkhPFcT_y3saj`）が作られ、`ligrol-backup.json` をアップロード（394 bytes） |
| 確認方法 | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/drive-audit.ps1`（読み取り専用） |
| 未検証 | GUI からの本の取り込み → `.nartik` 生成 → 閲覧（テストでは担保済み。実ファイルでの通し確認はしていない） |

### Web 版リネーム（2026-11-06）

- リポジトリ改名: `MegaBlackLabel/thundoku-shelf` → `ligrol`（private）、
  `thundoku-shelf-app` → `ligrol-app`（public）。ローカルの remote も更新済み
- コミット: `844fce7`（`feature/rename-ligrol`、602ファイル）
- パッケージ: `thundoku-{api,web,import,viewer}` → `ligrol-*`、`@megablacklabel/opfspack` → `@megablacklabel/nartik`
- 形式: マジック `NRTK`、拡張子 `.nartik`、AAD/鍵導出を `ligrol-*`。テストベクタとフィクスチャを再生成
- Worker: `ligrol-staging`（新規）を作成。旧 `thundoku-shelf-staging` は削除
- DO migrations を `v1 = SessionDurableObject` の1本に集約
- Cloudflare KV: 孤児8個を削除（`SESSIONS*` / `*-event-masters-cache*`）、残り10個を `ligrol-*` に改名
- secret 7個を投入（`SESSION_SECRET` / Google 系 / redirect URI 2本）
- 検証: `nartik` 345 / `ligrol-api` 340 / `ligrol-web` 2157 passed、typecheck、`build:staging` 成功
- staging 実機: `https://ligrol-staging.megablacklabel.workers.dev` が 200、`/health` が ok、
  `/api/auth/google/start` が新クライアントで 302

### 残タスク

- 本番 Worker `thundoku-shelf` は **workers.dev 無効・custom domain なし**で元から到達不能。
  本番を出すなら prod 用 KV（`DRIVE_TOKENS` が存在しない）と公開ホストの設定が必要
- Web リポジトリの push / PR（未実施）
- 旧 GCP プロジェクト `thundoku-shelf` の削除、Drive の旧 `thundoku-shelf/` フォルダ削除
- macOS / Linux の旧データ・旧資格情報の掃除

### ベースライン（2026-11-05 実測 / `target/` 除外）

| トークン | 出現数 |
|---|---|
| `thundoku-shelf` | 250 |
| `thundoku_core` | 633 |
| `thundoku_shelf` | 19 |
| `thundoku-core` | 15 |
| `ThundokuShelf` | 21 |
| `Thundoku` | 47 |
| `THUNDOKU` | 55 |
| `opfspack` | 563 |
| `opfs_path` | 109（**残す**） |

## 4. 外部リソース（値は書かない）

| 種別 | 場所 |
|---|---|
| GitHub | `MegaBlackLabel/thundoku-shelf-app` → `ligrol-app` に改名予定 |
| GCP（現行） | プロジェクト `thundoku-shelf`（番号 `1054619943130`）→ 削除予定 |
| GCP（新） | **作成済み**: `ligrol-app`（番号 `786418602386`、表示名 Ligrol、2026-11-05） |
| 有効 API | `drive.googleapis.com` / `picker.googleapis.com` / `apikeys.googleapis.com` |
| Picker API キー | 作成済み（表示名 `Ligrol Picker`、ID `56ba5131-…`）。参照元は ligrol.pages.dev / ligrol-staging…workers.dev / ligrol…workers.dev / localhost:5173 に制限済み。**値はここに書かない**（Web の `VITE_GOOGLE_API_KEY`） |
| OAuth | **作成済み**: デスクトップ アプリ `Ligrol Desktop`（アプリの既定値に埋め込み済み）と ウェブ アプリケーション `Ligrol Web`（redirect 8本 / JS 生成元5本）。どちらもプロジェクト `786418602386` |
| OAuth 同意画面 | 設定済み（External / テスト / スコープ `openid` `email` `drive.file` / テストユーザー2人） |
| OAuth secret の置き場所 | デスクトップ用は `crates/app/src/app_state.rs` に埋め込み（公開情報扱い）。Web 用は Web リポジトリの `.dev.vars` / `wrangler secret`。**このファイルには書かない** |
| Cloudflare | アカウント `megablacklabel@gmail.com`、Worker `thundoku-shelf-staging` / `thundoku-shelf` |
| Drive | 旧 `thundoku-shelf/` フォルダは削除対象 |

**秘密（OAuth secret、`.dev.vars`、`.p12` のパスワード）はこのファイルに書かない。**
置き場所だけ書く。

## 5. マシン別セットアップ

| 項目 | Windows | macOS | Linux |
|---|---|---|---|
| ツール管理 | mise | mise | mise |
| Rust | `mise exec -- cargo` | 同左 | 同左 |
| SVG→PNG | ImageMagick 7（librsvg 同梱） | `brew install imagemagick librsvg` | `apt install imagemagick librsvg2-bin` |
| ICNS | 同梱スクリプト | `iconutil` でも可 | 同梱スクリプト |
| Cloudflare | `wrangler login` | 同左 | 同左 |
| GCP | Cloud SDK（winget `Google.CloudSDK`） | `brew install --cask google-cloud-sdk` | `apt install google-cloud-cli` |
| 旧データ | `%APPDATA%\thundoku-shelf` | `~/Library/Application Support/thundoku-shelf` | `~/.local/share/thundoku-shelf` |
| 旧資格情報 | 資格情報マネージャー | Keychain | Secret Service |

### ローカルビルドの OAuth 設定

OAuth クライアントの **secret はリポジトリに置かない**（GitHub の push protection がブロックする）。
`option_env!` でビルド時に埋め込むので、Google ログインを試すビルドでは環境変数で渡す:

```sh
export LIGROL_GOOGLE_CLIENT_SECRET=...   # ligrol-app のデスクトップ用クライアント
mise exec -- cargo run -p ligrol
```

`scripts/drive-audit.ps1` も同じ環境変数を読む。配布ビルド（GitHub Actions）は
`secrets.LIGROL_GOOGLE_CLIENT_SECRET` / `vars.LIGROL_GOOGLE_CLIENT_ID` から注入する。

## 6. 検証コマンド

```sh
# 旧アプリ名・旧形式名がゼロ（CHANGELOG と naming.md は除外）
git grep -Iin thundoku -- . ':!CHANGELOG.md' ':!docs/naming.md'
git grep -Iin opfspack -- . ':!CHANGELOG.md' ':!docs/naming.md'
git grep -In '0x4f, 0x50, 0x46, 0x53' -- .

# 意図的に残すもの
git grep -In opfs_path -- . | wc -l    # 109 前後

# ビルド・テスト
mise exec -- cargo build --workspace --locked
mise exec -- cargo test --workspace
mise run licenses
```

実機確認（アプリ版の完了条件）:

1. Windows / macOS の release ビルドが通る
2. `.nartik` の取り込み → 閲覧 → エクスポート往復
3. Drive 同期（アップロード / 復元）
4. DLsite / FANZA / BOOTH / Fantia / TechBookFest のログイン（UA 変更後の再確認）
5. `Ligrol.app` の起動と Gatekeeper 手順

## 7. 残タスク・未決

| 項目 | 状態 |
|---|---|
| `crates/app/src/app_state.rs` の新 OAuth client id / secret 差し替え | GCP 作成後 |
| `scripts/drive-audit.ps1` の secret ローテーション | 任意 |
| アイコンの `<text>` パス化 | 方針未決（`magick` の librsvg 描画で足りるか検証中） |
| Web 版リネーム | アプリ版の検証完了後 |
| GitHub repo 改名 | 最後（README / UA が新名になってから） |

## 8. 既知の注意点

- `ナルティーク` はファイザー／エーザイの医薬品ブランド。カタカナ表記を公開しない
- `nartick` は実在の製品（NarTick E-Ink Calendar）なので使わない
- `opfspack` の封筒ラベル・owner_id 接頭辞は **Web 側（`packages/nartik`）と同一文字列**にする
- Worker 名を変えると Durable Object の namespace が新規作成され、Web のセッションは全滅する（ユーザーゼロなので許容）
- `drive.file` の付与は GCP プロジェクト単位。プロジェクトを変えると旧プロジェクトで作った Drive ファイルは見えなくなる
