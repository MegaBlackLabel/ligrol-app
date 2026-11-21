# Agent Instructions (ligrol-app)

このファイルは `ligrol-app`（デスクトップ版）固有の運用ルール。共通の人格・TDD・ツールルールは
`~/.omp/agent/AGENTS.md` と `~/.agents/skills/` に従う。

## このリポジトリ

- Rust / GPUI のデスクトップアプリ。DB は SQLite（`crates/core/src/db`）。
- 実装は TDD。テスト → Fail 確認 → 最小実装 → Pass の順を守る。
- 検証コマンド:
  - `mise run test`（`cargo test --workspace`）
  - `mise run lint`（clippy `-D warnings`）
  - `mise run build`（`cargo build --release`）

## Git / ブランチ / コミット / PR（リリース後・厳守）

- **リリース済みのため、コミット・プッシュ・PR 作成はユーザーの明示的な指示があるまで絶対に実行しない。**
  - 対象: `git commit` / `git push` / `gh pr create` / `gh pr merge`
  - 実装・修正・テスト・ビルドが終わったら、**差分と検証結果を報告して止める**。
    勝手にコミットを作らない。
  - 「PR 出しておいて」等の指示があったときだけ、その時点の変更をまとめて
    コミット → プッシュ → PR を作成する。
  - 指示の範囲を超えて別ブランチ・別リポジトリへコミットしない。
- コード変更時は feature ブランチを使う（`main` への直接コミット禁止）。
  - 例: `git switch -c fix/checklist-thumbnails`
- **マージはユーザーの「マージお願い」だけが十分条件。**
  CI 通過・Copilot 指摘の解消は必要条件だが、自動マージは禁止。
- PR 作成前にユーザーの動作確認を完了させる。PR を出したら一旦停止する。
- 修正後は必ずコードレビューを実施し、正確・完全であることを確認してから報告する。

## SQLite スキーマ変更時

以下のいずれかを変更した場合、同じ変更セット内で関連ドキュメントも更新する。

- `crates/core/migrations/0001_init.sql`
- `crates/core/src/db/schema.sql`
- `crates/core/src/db/mod.rs` のランタイムマイグレーション
- `crates/core/src/db/*.rs` のテーブル DDL / カラム追加 / PK・FK 変更

更新先の例:

- `docs/spec/02-data-model.md`
- `docs/account-switch.md`（所有者・`owner_id` / `owner_sub`）
- `docs/spec/06-sync-auth-drive.md`（バックアップ対象・除外列・PK）
- `docs/spec/10-pack-keys.md`（Drive バックアップ／表紙バンドル）

チェックリスト:

- [ ] テーブル定義の変更を仕様書へ反映
- [ ] ランタイムマイグレーションを追記
- [ ] PK / FK / UNIQUE の変更を追記
- [ ] バックアップの対象テーブル・除外列・競合キーを追記
- [ ] 既存データの移行・バックフィル条件を追記

## GitHub Copilot レビュー対応

PR 作成後、Copilot の指摘を確認する場合:

```bash
gh pr view <PR番号> --comments
gh api repos/MegaBlackLabel/ligrol-app/pulls/<PR番号>/comments \
  --jq '.[] | {path: .path, body: .body, line: .line}'
```

- セキュリティ指摘を最優先で修正する。
- デバッグログ・一時ファイルを残さない。
- 修正後は `mise run test` / `mise run lint` を実行する。
- 修正後のコミット・プッシュも、ユーザーの指示があるまで行わない。
  「直したから確認して」で停止する。

## 技術書典・Drive 仕様の変更

技術書典（チェックリスト／サークルマップ）や Drive 同期を変更するときは、
先に以下を確認する。

- `docs/spec/09-stores.md`
- `docs/spec/06-sync-auth-drive.md`
- `docs/spec/10-pack-keys.md`
- `docs/account-switch.md`

`ligrol-thumbs.json` / `ligrol-backup.json` の形式を変える場合は、Rust と
`docs/ligrol-thumbs.jsonc` などのサンプル・仕様書を同時に更新する。
