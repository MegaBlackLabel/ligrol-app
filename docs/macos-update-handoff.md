# macOS 自動アップデート対応の引き継ぎ

最終更新: 2026-11-10
対象: macOS（Apple Silicon / Intel）でのビルド検証と、自動アップデートの macOS 対応。

Windows 側の土台はこのリポジトリで検証済み。**macOS は未検証**（このマシンに Mac が無い）。
この文書は、Mac 環境で続きをやる人（または将来の自分）向けの手順と注意点。

---

## 1. いまの状態

### 済んでいること（Windows で検証済み）

- Rust ツールチェーンを **1.98.1** に更新（`rust-toolchain.toml` / `.mise.toml` / CI の pin）
- `gpui-updater-pre = "=0.4.4"` を追加（**まだ未使用**。更新 UI の実装はこれから）
  - MSRV が 1.98 のため、ツールチェーン更新とセットで固定している
- 1.98 の新 clippy lint を解消（`chunks_exact*` → `as_chunks*`、未使用 import の削除）
- `cargo clippy --workspace --all-targets -- -D warnings` が Windows で green

### 未検証・未対応

- macOS でのビルド・テスト（このリポジトリの CI は macos-14 を 2 アーキで回す）
- macOS の配布物（`.dmg`）と、gpui-updater の macOS インストール経路
- Homebrew cask との共存（アプリ側ゲート）の実装
- 自動アップデートの本体（チェック・UI・再起動）。配布物の dmg/MSI 化も未着手

## 2. 決定済みの方針（変更するなら要相談）

| 項目 | 決定 |
|---|---|
| 使うクレート | `gpui-updater-pre`（gpui-kit の `gpui-pre 0.3.9` と同一パッケージで型が一致することを Windows で確認済み） |
| Windows 配布 | MSI/インストーラ化は OK。Store へは **MSI/EXE 提出**（Store は既存ユーザーに更新を配らないため、アプリ内更新が前提） |
| Windows 署名 | 後で決める（gpui-updater の Authenticode 検証は「インストール済み exe が署名済みのときだけ」走る） |
| Homebrew | **アプリ側ゲート**。`Caskroom` を検出したら自己更新せず `brew upgrade --cask ligrol` を案内。cask に `auto_updates` は付けない |
| macOS 配布物 | gpui-updater の macOS インストーラは `.dmg` のみ。zip は残しつつ dmg を追加する |

## 3. Mac で最初にやる検証

```sh
# ツールチェーン（mise 経由で入る。rustup 直でも可）
rustup toolchain install 1.98.1 --component clippy --component rustfmt

# PDF 描画テスト用（初回だけ。crates/core/ に dylib を置く）
mise run pdfium

# 依存が 1 つに解決されているか（gpui-pre が 0.3.9 だけであること）
cargo tree -p ligrol -i gpui-pre
cargo tree -d | grep -i gpui

# 品質ゲート
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# リリースビルド
cargo build --release --locked -p ligrol
```

期待値:

- `gpui-pre` は 0.3.9 の 1 種類だけ
- clippy / test が green
- release ビルドが通る（`gpui-updater-pre` はまだ未使用なので、追加クレートのビルドだけが増える）

失敗したときの見どころ:

- `gpui-updater-pre 0.4.4` は `rust-version = "1.98"`。1.98.1 未満だと依存解決で落ちる
- 1.98 で新しい clippy lint が出たら、Windows 側と同じように機械的に直す

## 4. gpui-kit のバージョンについて

`gpui-kit` は `=0.8.0` 固定のまま。0.8.1 は `gpui-pre-macos 0.3.10` が当時の stable（1.97）で
未安定 API を使うため見送っていた（`docs/spec/01-architecture.md` 参照）。
**1.98.1 では 0.8.1 / 0.9.x がビルドできる可能性がある**ので、Mac で一度試す価値あり。

注意:

- 上げるなら `gpui-kit` と `gpui-webview` を同時に揃える（gpui-pre が二重に入ると型が食い違う）
- `gpui-updater-pre` は `gpui-pre ^0.3.4` なので、gpui-kit 側が 0.3.12 でも解決はできる
- 失敗したら 0.8.0 に戻して、この文書に結果を残す

## 5. 自動アップデートの macOS 対応（これから）

gpui-updater のインストール戦略はプラットフォーム固定:

- macOS: **`.dmg`** をマウントして `.app` を差し替え
- Windows: 素の `.exe` または `.msi`
- Linux: tar.gz / AppDir / AppImage（今回は対象外）

### 5.1 配布物

`.app` を署名・公証・staple した後（`release.yml` の既存手順）に dmg を作る:

```sh
STAGE=target/<target>/release/dist
hdiutil create -volname "Ligrol" -srcfolder "$STAGE" -ov -format UDZO \
  "ligrol-<version>-<arch>-macos.dmg"
```

- zip は残してよい（新規ユーザー向け）
- dmg も必要なら署名・公証する（`.app` 側が公証済みなら動くが、dmg 自体の検証は別）

### 5.2 検証設定（アプリ側）

`EngineConfig` に最低限これを入れる想定:

- `verification(Verification::Strict)`
- `minisign_public_key(...)`（CI の署名鍵と対）
- `BundleVersion::MustMatch`（`CFBundleShortVersionString` = リリース版）
- `CodeSigning::MatchInstalled`（Team ID 一致）または `CodeSigning::TeamId(...)`

macOS では「インストール済み `.app` が署名済みのとき、更新も同じ Team で署名されていること」を
`codesign --verify --deep --strict` で確認する。公証済みの `.app` を差し替えるので、
**更新側も署名・公証済みであること**が前提。

### 5.3 権限

- `/Applications` は管理者ユーザーなら書けるが、**標準ユーザーは書けない**
- gpui-updater は DL 前に `ErrorKind::Permission` を返すので、UI では
  「/Applications に書き込めないため自動更新できません」と案内する
- どうしても標準ユーザーも更新したいなら Sparkle ドライバ（`gpui-updater-sparkle`）が候補。
  ただし Sparkle.framework の同梱・appcast・Ed25519 鍵の管理が増える

### 5.4 Homebrew

- `ownership()` が `Ownership::Packager`（Caskroom の symlink）を返したら、自己更新 UI を出さない
- 代わりに `brew upgrade --cask ligrol` を案内する
- cask 側は `auto_updates` を付けない（アプリが brew 管理下で黙る前提）

## 6. リリースパスの検証（Mac 実機 / VM）

gpui-kit の Auto Update ガイドにある表をそのまま使う:

| シナリオ | 期待 |
|---|---|
| 最新・オフライン・遅い回線・タイムアウト | アプリは使えたまま。状態表示が正直で再試行できる |
| 対象違い・アセット欠け・切り詰め・ハッシュ不一致・署名不一致 | 何もインストールされない。失敗フェーズが分かる |
| 未保存の作業がある状態で新バージョン | 再起動を後回しにできる |
| 連打・同時チェック・チェック中に新リリース | 選んだ版だけが 1 回インストールされる |
| 書込不可（標準ユーザー）/ macOS の署名検証失敗 | 途中で壊さず失敗する |
| インストール中断・再起動失敗・ダウングレード | 起動できる版に戻る。ユーザーデータは無事 |

実機での最低ライン:

1. 旧版を `/Applications` に入れる
2. 新 dmg を公開（GitHub Releases）
3. アプリの「更新」から DL → 再起動
4. 再起動後に `CFBundleShortVersionString` が新しくなっていること
5. `codesign --verify --deep --strict Ligrol.app` が通ること

## 7. 注意

- リリースタグ `vX.Y.Z` と `Cargo.toml` の version が一致していること（現状 v0.2.13 と 0.3.0 がズレている）。リリース CI に一致チェックを入れる（未実装）
- `release.yml` の macOS 署名・公証の手順は触らない（証明書・公証の順序が繊細）
- 依存を足したら `mise run licenses` で `crates/app/assets/third-party/licenses.json` を再生成し、
  `licenses_json_matches_cargo_lock` を通す
- `mise run licenses` が「ソースが見つからない」で止まったときは、ホストで未ビルドの
  target 専用依存（`jni` / `rustls-platform-verifier-android` / `webpki-root-certs` など）が
  展開されていないだけ。`cargo fetch --target aarch64-linux-android` のあと、
  `~/.cargo/registry/cache/*/` の `.crate` を `~/.cargo/registry/src/*/` に展開してから再実行する
  （リポジトリには影響しないローカル操作）
- `gpui-updater-core` は Android 向けの `jni` などを lock に入れる。供給網レビューでは
  「追加される 10 クレート」をまとめて確認すること
- CI のツールチェーンは `dtolnay/rust-toolchain` の **1.98.1 の ref の SHA** に固定済み
  （`ci.yml` 2 箇所 / `release.yml` 1 箇所）
