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
- **設定画面の「アップデート」節**（状態表示・手動チェック・DL/中止・再起動・リリースノート）
- **自動チェック**（起動 30 秒後、以降 24 時間おき。最終確認は `app_settings` に保存）
- **所有権ゲート**（Homebrew / WindowsApps / winget などを検出したら自己更新せず案内）
- GitHub Releases フィード（`SHA256SUMS` + `.minisig`）。公開鍵が無い開発ビルドはチェックサム必須

### 未検証・未対応

- macOS でのビルド・テスト（このリポジトリの CI は macos-14 を 2 アーキで回す）
- macOS の配布物（`.dmg`）と、dmg 経由のインストール検証
- Windows の MSI 化と Store（MSI/EXE）提出
- リリース CI の `SHA256SUMS` / minisign 署名 / タグと Cargo version の一致チェック
- Homebrew cask の実機確認（`brew upgrade ligrol` の案内が正しいこと）

## 2. 決定済みの方針（変更するなら要相談）

| 項目 | 決定 |
|---|---|
| 使うクレート | `gpui-updater-pre`（gpui-kit の `gpui-pre 0.3.9` と同一パッケージで型が一致することを Windows で確認済み） |
| Windows 配布 | **Store は MSIX（メイン。Store が再署名するので証明書不要）**。Store を使わない人向けに **署名なし MSI**（per-user、`%LOCALAPPDATA%\Ligrol`）を直販 |
| Windows 署名 | **いまは署名しない**（MSIX は Store 再署名 / 直販 MSI は SmartScreen 警告を README で案内）。将来 SmartScreen を消したくなったら IV 証明書（約 $129/年）or Certum OSS クラウド（€49） |
| Store 版の更新 | 自己更新は不可。起動時に Store の更新を確認し、`ms-windows-store://downloadsandupdates`（将来は `StoreContext`）で促す。**GitHub フィードの自動チェックは Store 版では止める** |
| 直販 Windows の更新 | `gpui-updater` の **`.msi`** 経路。`updates.rs` の Windows フィルタは `.msi` のまま |
| Homebrew | **アプリ側ゲート**。`Caskroom` を検出したら自己更新せず `brew upgrade ligrol` を案内。cask に `auto_updates` は付けない |
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
- 代わりに `brew upgrade ligrol` を案内する
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

- リリースタグ `vX.Y.Z` と `Cargo.toml` の version が一致していること。リリース CI の
  `Compute version` が一致を検証して落とす（実装済み）
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
- リリースビルドで埋め込む環境変数:
  - `LIGROL_UPDATE_MINISIGN_KEY`: 更新フィードの minisign 公開鍵。設定すると
    `Verification::Strict`（署名 + SHA-256）になる
  - `LIGROL_UPDATE_EXPLANATION`（任意）: パッケージ管理者が「このビルドはこう更新する」と
    示す文言。設定すると `Ownership::Packager` として自己更新を止める
- リリース CI の secret / variable:
  - `MINISIGN_SECRET_KEY`（secret）: minisign 秘密鍵ファイルを **base64** にしたもの。
    `minisign -G -W -p update.pub -s update.key` で作る（`-W` = パスワードなし。CI で
    プロンプトが出ないようにする）。`base64 -w0 update.key` を登録する。
    公開鍵は `LIGROL_UPDATE_MINISIGN_KEY`（variable）に、公開鍵ファイルの中身
    （`minisign -G` が表示する文字列）を入れる
  - `MSIX_PUBLISHER`（variable）: Partner Center の Publisher（例 `CN=...`）。
    未設定だと `CN=MegaBlackLabel` で MSIX を作る（Store 提出時は必ず設定する）
  - `MSIX_IDENTITY_NAME`（variable）: Partner Center で予約したパッケージ名
    （例 `MegaBlackLabel.Ligrol`）。未設定だとその既定値になる
- 直販 Windows は **署名なし MSI**。SmartScreen の警告が出るので README / 配布ページに
  「詳細情報 → 実行」の手順を書く。更新の検証は minisign + SHA256SUMS で行う

## 8. 次の作業（優先順）

優先度の高い順。各項目は独立して PR にできる。

1. **リリース CI に配布物を足す**（`.github/workflows/release.yml`）— **実装済み（要 CI 実行確認）**
   - macOS: 公証・staple 後に `.dmg` を生成（§5.1）。ファイル名は `ligrol-<ver>-<arch>-macos.dmg`
   - Windows Store: **MSIX**（`packaging/windows/AppxManifest.xml.in` + `make-msix.ps1`）。
     Store が再署名するので証明書は不要。**ローカルにサイドロードするときだけ**
     LocalMachine\TrustedPeople への証明書登録が要る
   - Windows 直販: **MSI**（`packaging/windows/Ligrol.wxs`、per-user で `%LOCALAPPDATA%\Ligrol`）。署名なし
   - 配布物の名前は `crates/app/src/updates.rs` の `asset_patterns()` に合わせる
     （Windows 直販 = `windows` + `.msi` / macOS = `macos` + `.dmg` + アーキテクチャ）
2. **更新フィードの署名と版一致** — **実装済み（鍵の登録が必要）**
   - release に `SHA256SUMS` と、MSI / dmg の `.minisig` を追加（`version:<semver>` 付き）
   - minisign 秘密鍵は CI secret（`MINISIGN_SECRET_KEY`）、公開鍵は
     `LIGROL_UPDATE_MINISIGN_KEY` としてビルドに埋め込む
   - タグ `vX.Y.Z` と `Cargo.toml` の version の一致チェックを `release.yml` の
     `Compute version` に追加済み
3. **Windows の実機テスト**
   - MSI をインストール → アプリ内更新 → 再起動後 `ligrol.exe` が新しい版になっていること
   - Store 版（MSIX）: 起動時に Store の更新誘導が出て、`ms-windows-store://downloadsandupdates` が開くこと。
     Store 版では GitHub フィードの自動チェックをしないこと
   - `remove_leftovers()` は **素の `.exe` 更新のときだけ**必要。MSI 運用なら不要
   - Homebrew / winget / Scoop 相当のパスに置いたときに自己更新ボタンが出ないこと
4. **macOS の実機テスト**（§6 の表）
   - dmg 経由の更新、`/Applications` の権限、標準ユーザー、brew cask のゲート
   - `Ligrol.app` を Homebrew cask で入れた状態で `brew upgrade ligrol` の案内が出ること
5. **ストア / cask への登録**
   - Microsoft Store: **MSIX 提出**（Store 再署名なので証明書不要。`pdfium.dll` も個別署名不要）。
     自己更新は不可なので、Store の更新誘導を入れる
   - Homebrew: cask を追加（dmg の URL と sha256、`auto_updates` は付けない）
6. **任意（後回し）**
   - メニューに「アップデートを確認」を足す（今は設定画面のみ）
   - Sparkle ドライバ（標準ユーザーも `/Applications` を更新できる）
   - delta 更新（`gpui-updater-pre` の `delta` feature）

## 9. Windows 署名の選択肢（2026-11 時点）

決定: **いまは署名しない**。Windows は **Store = MSIX（Store 再署名）**、
直販は **署名なし MSI**。将来 SmartScreen 警告を消したくなったら IV 証明書（約 $129/年）
or Certum OSS クラウド（€49）を検討する。発行元に個人の実名が出ることは許容。

| 選択肢 | 費用 | 日本の個人 | Store MSI/EXE | メモ |
|---|---|---|---|---|
| Store MSIX（Store 再署名） | 無料 | ○ | MSIX のみ | MSIX は自己更新不可（アプリはゲート済み） |
| Azure Artifact Signing（旧 Trusted Signing） | 約 $9.99/月 | **✗** | ✗ | 個人は米・加のみ、法人も米/加/EU/英のみ |
| OV 証明書 | $150〜300/年 | 法人向け | △ | 法人名が出る |
| **IV 証明書**（SSL.com 等） | **$129/年** | **○** | △ | 実名が出る。eSigner（クラウド署名）か YubiKey（$379） |
| EV 証明書 | $400+/年 | 法人のみ | △ | 2024 年以降 SmartScreen の即時回避なし |
| SignPath Foundation（OSS 無料） | 無料 | ○（審査） | △ | 発行元は SignPath Foundation。手動承認・ポリシー・役割・MFA が条件 |
| 署名なし | 無料 | ○ | ✗ | SmartScreen の強い警告 |

注意:

- Store の MSI/EXE 要件は「インストーラと**含まれるすべての PE ファイル**の署名」。
  うちは `ligrol.exe` + `pdfium.dll` の 2 つなので、**pdfium の静的リンク**（exe 1 本）か
  DLL も署名するかの判断が要る。SignPath 規約では上流バイナリを自分の証明書で署名できないため、
  静的リンクが前提になる
- IV 証明書は政府 ID + 自撮り + 電話確認。eSigner は別途サブスクで CI から署名できる
  （YubiKey をランナーに挿さない）
- 証明書を切り替えると Authenticode の検証（`CodeSigning::MatchInstalled`）が効くため、
  署名を始めたら**同じ CN で継続**するか、`CodeSigning::Publisher` を固定する
