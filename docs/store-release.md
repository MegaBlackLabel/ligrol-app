# Microsoft Store リリース自動化

`release.yml` はタグ `v*` で Windows の MSIX を作り、GitHub Release に添付する。
その後に `store-submit` ジョブが MSIX を Microsoft Store へ提出する。

## 初回だけ手動でやること

1. Partner Center でアプリを公開済みにする（初回提出は手動）
2. Partner Center と Microsoft Entra ID を関連付ける
3. Entra ID でアプリ登録を作成する
4. Partner Center → アカウント設定 → ユーザー管理 → Microsoft Entra アプリケーション に
   登録し、**Manager** ロールを付与する
5. 以下を控える
   - Tenant ID
   - Client ID
   - Client Secret
   - Seller ID
   - Store の Product ID（`9N...`）

## GitHub の設定

Repository Settings → Secrets and variables → Actions。

Secrets:

| 名前 | 値 |
|---|---|
| `AZURE_AD_TENANT_ID` | Entra の Tenant ID |
| `AZURE_AD_APPLICATION_CLIENT_ID` | Entra アプリの Client ID |
| `AZURE_AD_APPLICATION_SECRET` | Entra アプリの Client Secret |
| `SELLER_ID` | Partner Center の Seller ID |

Variables:

| 名前 | 値 |
|---|---|
| `STORE_PRODUCT_ID` | Store の Product ID |
| `MSIX_PUBLISHER` | Partner Center の Publisher（`CN=...`） |
| `MSIX_IDENTITY_NAME` | Partner Center の Package identity name |

`MSIX_PUBLISHER` / `MSIX_IDENTITY_NAME` が未設定なら `make-msix.ps1` の既定値を使う。
Store に出すときは必ず Partner Center の値と一致させる。

## リリース

1. `Cargo.toml` の version を上げる
2. 同じ番号で注釈タグを打つ（例 `v0.3.2`）
3. push する

`release.yml` が以下を実行する:

- テスト
- Windows / macOS ビルド
- MSIX / MSI / zip / dmg 作成
- GitHub Release 作成
- `store-submit`: MSIX を Store に提出

Secrets / Variables が不足している場合、`store-submit` は警告を出してスキップする。
通常の GitHub Release は止まらない。

## 注意

- Store の審査は非同期。提出後の公開状態は Partner Center で確認する。
- MSIX は Store が再署名するので証明書は不要。
- MSIX の version は 4 桁で、4 桁目は `0`（`0.3.2.0`）。
- Client Secret の有効期限が切れると `store-submit` が失敗する。
- 公式 GitHub Action は無料アプリのみ対応。有料アプリは Microsoft Store submission API を直接使う。
