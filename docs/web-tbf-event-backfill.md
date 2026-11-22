# Web 版: 技術書典の購入にイベント名を付ける

対象: `D:\develop\ligrol\ligrol`（Web モノレポ）

## 背景

技術書典の古い購入は API の `BookShelfQuery` が
`marketHandshake.event` を返さないため、`bookshelf_items` の
`event_name` / `event_slug` / `event_id` がすべて NULL になる。

実機 DB（2026-11-22）:

| 状態 | 件数 |
|---|---:|
| 技術書典の本 | 614 |
| 3 列とも NULL | 358 |
| いずれかが入っている | 256 |

デスクトップ版は今回の修正以降、TBF 同期時に `causedAt` からイベントを補完して
`bookshelf_items` に保存する。これにより Drive バックアップにも
`event_name` / `event_slug` / `event_id` が載る。

## 1. まずやること: バックアップの再取り込み

デスクトップ版で **技術書典の本棚を同期** してから、Web で Drive バックアップを
再取り込みする。スキーマ変更は不要。

確認:

```sql
SELECT count(*)
FROM bookshelf_items
WHERE site_id = 'techbookfest'
  AND event_name IS NULL
  AND event_slug IS NULL
  AND event_id IS NULL;
```

同期後は 358 件から減る。残るのは次の 2 行だけ（同じ本を 2 アカウントが購入）:

- `database_id = 5633017015959552`
- `title = PWA を Google Play Storeに公開するハンズオン`
- `causedAt = 2024-01-05`
- どのイベント開催期間にも入らないため NULL のまま

## 2. 古いバックアップを読む場合のフォールバック

未同期の旧バックアップや、すでに Web の IndexedDB に入っている NULL 行も
表示できるようにする。**保存値は書き換えず、表示時だけ補完**してよい。

`packages/ligrol-web/src/lib/db.ts` の `loadBookshelfItems()` は現在、

```sql
LEFT JOIN tbf_events e  ON b.event_id = e.id
LEFT JOIN tbf_events e2 ON b.event_slug = e2.slug
```

でイベント名を引いている。`event_id` / `event_slug` が両方 NULL の行は
ここで拾えないので、`causedAt` からの日付フォールバックを足す。

既存の `packages/ligrol-web/src/lib/db/repositories/checklist-event.repository.ts` の
`findEventInfoByDate()` がそのまま使える（現在はテスト以外から未使用）。

アルゴリズム:

1. `site_id = 'techbookfest'`
2. `event_name` / `event_slug` / `event_id` がすべて空
3. `causedAt` の先頭 10 文字（`YYYY-MM-DD`）を日付にする
4. `tbf_events` の `event_start_date <= 日付 <= event_end_date` を満たす行を探す
5. 複数ある場合は `event_start_date` が最も新しい行を選ぶ
6. 表示用に `event_name = t.event_name`、`event_slug = t.slug`、`event_id = t.id` を埋める

`findEventInfoByDate()` は `new Date()` で日付を解釈するため、`causedAt` が
壊れている行は `null` を返す。その場合は「イベント不明」のままでよい。

`Bookshelf.tsx` の `shelfMetaText()` は既に `event_slug` から
`技術書典N` を作るので、`event_slug` だけ補完できれば表示は直る。
ただし `event_name` も入れておくこと（バックアップとの比較やフィルタで使う）。

## 3. テスト

最低限、次の 2 ケースを追加する。

1. `causedAt = '2026-04-26T...'` の `bookshelf_items` 行が
   表示上 `技術書典20` になる（`tbf_events` に `tbf20` がある状態）
2. `causedAt = '2024-01-05'` のようにどの期間にも入らない行は
   `イベント不明` のまま

## 受け入れ条件

- [ ] デスクトップ同期後のバックアップ取り込みで、358 件のうち 356 件に
      `event_name` / `event_slug` / `event_id` が入る
- [ ] `PWA を Google Play Storeに公開するハンズオン` の 2 行だけは NULL のまま
- [ ] Web の本棚カードで、上記 356 件が「イベント不明」ではなく
      `技術書典20` などの名前になる
- [ ] 既存のイベント名が入っている 256 件を上書きしない
