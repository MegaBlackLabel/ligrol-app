# Web 版: チェックリストの owner 分離（実装手順）

デスクトップ版が `checked_items` を `(id, owner_id)` の複合主キーへ移行した。
Web 版も同じモデルにしないと、同じ技術書典の項目 ID を複数アカウントで持てず、
片方のアカウントの項目が欠ける（実機: `tbf21` が 30 件中 20 件しか出ない）。

対象: `D:\develop\ligrol\ligrol`（Web モノレポ）

## 1. スキーマ

`packages/ligrol-web/src/types/schema.sql` と
`packages/ligrol-web/src/lib/db/migrations.ts` を変更する。

### `checked_items`

- `owner_id TEXT NOT NULL DEFAULT ''` を追加
- 主キーを `(id, owner_id)` に変更
- `owner_sub` は既存のまま（バックアップ所有者フィルタ用）

```sql
CREATE TABLE checked_items (
  id TEXT NOT NULL,
  owner_id TEXT NOT NULL DEFAULT '',
  event_id TEXT NOT NULL,
  -- 既存カラムはそのまま
  owner_sub TEXT,
  PRIMARY KEY (id, owner_id)
);
```

SQLite は PK を ALTER できないため、既存 DB はテーブルを作り直して
`INSERT ... SELECT id, '', ... FROM checked_items_old` で移行する。
`owner_id` はデスクトップの `owner_id`（`derive_owner_id(sub)`）と同じ値にする。

### `product_sample_pages`

- `owner_id TEXT NOT NULL DEFAULT ''` を追加
- 複合 FK `(checklist_item_id, owner_id) REFERENCES checked_items(id, owner_id)`

```sql
CREATE TABLE product_sample_pages (
  id TEXT PRIMARY KEY,
  checklist_item_id TEXT NOT NULL,
  owner_id TEXT NOT NULL DEFAULT '',
  -- 既存カラムはそのまま
  FOREIGN KEY (checklist_item_id, owner_id)
    REFERENCES checked_items(id, owner_id)
);
```

既存行の `owner_id` は親 `checked_items` の `owner_id` から埋める。
移行中は `PRAGMA foreign_keys = OFF` で作り替え、最後に ON に戻す。

### インデックス

```sql
CREATE INDEX checked_items_owner_event_idx
  ON checked_items(owner_id, event_id);
```

## 2. クエリ

`packages/ligrol-web/src/lib/db/repositories/checklist.repository.ts`
（無ければ `checked_items` を触る repository）で、**すべての読み書きに owner を足す**。

- 一覧: `WHERE event_id = ? AND owner_id = ?`
- 取得: `WHERE id = ? AND owner_id = ?`
- チェック: `UPDATE ... WHERE id = ? AND owner_id = ?`
- サムネイル: `WHERE id = ? AND owner_id = ?`
- 削除: `WHERE id = ? AND owner_id = ?`

`id` 単独で更新・削除しない（他アカウントの行を壊す）。

## 3. バックアップ取り込み

`packages/ligrol-web/src/lib/drive/backup-tables.ts`:

- `checked_items` の PK を `['id', 'owner_id']`
- `product_sample_pages` は `id` のまま（行に `owner_id` が入る）

`packages/ligrol-web/src/lib/db/repositories/drive-backup.repository.ts`:

- `pruneDeletedChecklistItems`: `(id, owner_id)` 単位で削除する
- `product_sample_pages` の削除も `(checklist_item_id, owner_id)` で行う
- `reconcileChecklistEvent`（`lib/drive/checklist-reconcile.ts`）も
  `id + owner_id` で比較し、`INSERT` 時の `owner_id` を保持する
- `remapEventReferences` で `product_sample_pages` の親を付け替える場合も
  `owner_id` を壊さない

## 4. UI

React の key や選択状態は `id` 単独ではなく `owner_id + id` の複合キーにする。

```ts
const itemKey = `${item.owner_id ?? ''}\u0000${item.id}`
```

`selectedItemId` も同じ複合キーで保持する。
`?checklist-debug` の件数表示も owner ごとに分ける。

## 5. 表紙バンドル

`ligrol-thumbs.json` の封筒は owner ごとに分かれているため、entry の
`item_id` はそのまま使える。ただし Web の `thumbnail_data` 参照は
`(id, owner_id)` で引くこと。

再同期の手順:

1. デスクトップを最新版にする
2. 対象アカウントでチェックリストを同期する
3. デスクトップの Drive 同期を実行する
4. Web で `?checklist-debug` を開き、`base` 件数がアカウントの件数と一致するか確認
5. 一度チェックリストを再読込する（`drive_backup_base` との突き合わせが走る）

## 6. 受け入れ確認

- [ ] 同じ TBF 項目 ID を 2 アカウントが持てる
- [ ] Web の一覧が現在アカウントの行だけを出す
- [ ] チェック状態がアカウント間で混ざらない
- [ ] 試し読みページが `(checklist_item_id, owner_id)` で分かれる
- [ ] バックアップの復元で `owner_id` が保持される
- [ ] `?checklist-debug` の `base` 件数がデスクトップの同期件数と一致する
