//! チェックリストビュー: イベント一覧 → チェック項目、同期、試し読み、
//! お気に入り取り込み。

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read as _;
use std::sync::Arc;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::{ScrollableElement as _, Scrollbar};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName};
use gpui_kit::{
    Context, FontWeight, IntoElement, ParentElement, Render, RenderImage, ScrollHandle,
    SharedString, StyledImage, Window, div, img, px,
};
use gpui_kit::{
    InteractiveElement as _, ReadGlobal as _, StatefulInteractiveElement as _, Styled as _,
};
use ligrol_core::db;
use ligrol_core::db::checklist::{CheckedItem, TbfEvent};
use ligrol_core::tbf;

use crate::app_state::AppState;
use crate::icons::AppIcon;
use crate::views::bookshelf::{progress_ring_image, rasterize_svg_scaled};
use image::GenericImageView as _;

/// デコード済みサムネイルの保持上限（1 ページ = 50 件 + 余裕を持たせた値）。
/// これを超えたら古い順に追い出してメモリに残さない。
const MAX_THUMB_CACHE: usize = 64;

/// 並び替えフィールド（Web の SortField 相当）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortField {
    SortOrder,
    CircleName,
    SpaceNumber,
}

/// 試し読みの進捗（0.0..=1.0）。`page_index` は 0 始まりの取得中ページ。
///
/// リングはページ単位で進める（`total_bytes` が 0 = `Content-Length` 無しのときは
/// そのページの先頭に留める。割合を偽らない）。
fn sample_progress_fraction(
    page_index: usize,
    page_count: usize,
    downloaded: u64,
    total_bytes: u64,
) -> f32 {
    if page_count == 0 {
        return 0.0;
    }
    let inner = if total_bytes > 0 {
        (downloaded as f32 / total_bytes as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((page_index as f32 + inner) / page_count as f32).clamp(0.0, 1.0)
}

pub struct ChecklistView {
    events: Vec<TbfEvent>,
    selected_slug: Option<String>,
    items: Vec<CheckedItem>,
    /// お気に入り（フォロー中）のサークル名。地図の青色表示に使う。
    favorite_circles: HashSet<String>,
    /// 試し読みの取得進捗（チェック項目 id → 0.0..=1.0）。render は DB を読まない。
    sample_progress: HashMap<String, f32>,
    busy: bool,
    error: Option<String>,
    toast: Option<String>,
    last_sync: Option<String>,
    /// 並び替え（Web の sort 相当）
    sort_field: SortField,
    /// true = 昇順（Web の sort.direction === "asc"）
    sort_ascending: bool,
    /// 初出が選択中イベント以外の項目も表示するか（既定 OFF）
    show_other_first_appearance: bool,
    /// ページネーション（Web の PAGE_SIZE = 50）
    page: usize,
    /// 一覧の「これより以前のイベントを表示」で表示済みの件数
    visible_older_count: usize,
    /// デコード済みサムネイルのキャッシュ（render で毎回デコードしない）。
    /// 保持数は MAX_THUMB_CACHE まで（超過分は古い順に追い出す）
    thumbnail_images: HashMap<String, Arc<RenderImage>>,
    /// thumbnail_images の挿入順（先頭が最古）。上限超過時の追い出しに使う
    thumbnail_order: VecDeque<String>,
    /// 追い出し済みサムネイルの GPU テクスチャ解放待ち（`release_evicted_thumbs` が
    /// 描画時に drop_image する）。キャッシュ操作は window を持たない
    /// バックグラウンドタスクから呼ばれるため、いったんここに預けてから解放する
    evicted_thumb_images: Vec<Arc<RenderImage>>,
    /// 取得中のサムネイル（重複ダウンロード防止）
    thumbnail_fetching: HashSet<String>,
    /// ダウンロード失敗したサムネイル（reload 連鎖の無限ループ防止。
    /// 失敗はアプリ再起動でリセットされ、次回起動時に再試行される）
    fetch_failed: HashSet<String>,
    /// 空のイベントを開いたときに自動同期を試した slug（同じイベントを開き直しても
    /// 繰り返し叩かない。ビューの寿命 = アプリを閉じるまで）
    auto_sync_attempted: HashSet<String>,
    /// 取得済みのサークルマップ（イベント slug → 原寸画像）。ネットワークへは
    /// 「サークルマップ」を開いたときにだけ出る。
    circle_map_images: HashMap<String, Arc<RenderImage>>,
    /// サークルマップ取得中のイベント slug（重複取得の防止）
    circle_map_fetching: HashSet<String>,
    /// サークルマップの取得失敗（イベント slug → 表示するエラー）
    circle_map_error: HashMap<String, String>,
    /// 選択中イベントのサークルマップを表示中か
    show_circle_map: bool,
    /// 地図の倍率（1.0 = 原寸）
    circle_map_zoom: f32,
    /// 「全体表示」中か（倍率は viewport にフィット）
    circle_map_fit: bool,
    /// 地図の平行移動（画像中心からのずれ）
    circle_map_pan: gpui_kit::Point<f32>,
    /// ドラッグ開始位置
    circle_map_drag_start: Option<gpui_kit::Point<f32>>,
    /// ドラッグの慣性速度（px/秒）
    circle_map_pan_velocity: gpui_kit::Point<f32>,
    /// 慣性アニメーションの世代（新しい操作で古いループを止める）
    circle_map_inertia_generation: u64,
    /// ダブルクリック判定
    circle_map_last_click: Option<std::time::Instant>,
    /// クリック/ドラッグ判定（ドラッグ中に動いたらクリック扱いにしない）
    circle_map_click_moved: bool,
    /// 表示中の地図画像の画面上の bounds（クリック位置→セル判定に使う）
    circle_map_bounds: std::rc::Rc<std::cell::Cell<Option<gpui_kit::Bounds<gpui_kit::Pixels>>>>,
    /// 表示中の地図ビューポートの画面上の bounds（ホイール位置→拡大中心に使う）
    circle_map_viewport_bounds:
        std::rc::Rc<std::cell::Cell<Option<gpui_kit::Bounds<gpui_kit::Pixels>>>>,
    /// ホイール 1 ジェスチャー中に中央へ寄せる画像座標と、そのときの画面位置
    circle_map_wheel_anchor: Option<(gpui_kit::Point<f32>, gpui_kit::Point<f32>)>,
    /// ホイールズームの目標倍率（アニメーション中のみ）
    circle_map_zoom_target: Option<f32>,
    /// ホイールズームの目標 pan（アニメーション中のみ）
    circle_map_pan_target: Option<gpui_kit::Point<f32>>,
    /// ズームアニメーションの世代（新しい操作で古いループを止める）
    circle_map_zoom_animation_generation: u64,
    /// イベント slug → 色付きセルの矩形（画像ピクセル座標）
    circle_map_cells: HashMap<String, HashMap<(String, u32), CellRect>>,
    /// 地図クリックで選択中のチェックリスト項目
    selected_item_id: Option<String>,
    /// チェックリストのスクロール位置（地図クリック時の自動スクロールに使う）
    checklist_scroll_handle: ScrollHandle,
}

/// 地図セルの矩形（画像ピクセル座標）。
#[derive(Debug, Clone, Copy)]
struct CellRect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

/// 地図の表示領域（960px のパネル - `p_3` の左右 24px）。
const CIRCLE_MAP_VIEWPORT_WIDTH: f32 = 936.0;
const CIRCLE_MAP_VIEWPORT_HEIGHT: f32 = 520.0;
/// 地図モードの倍率（1.0 = 原寸）。全体表示まで縮小できる。
const CIRCLE_MAP_MIN_ZOOM: f32 = 0.2;
const CIRCLE_MAP_MAX_ZOOM: f32 = 6.0;
/// マップ画像のラスタライズ倍率（ヒット判定の座標も同じ倍率に揃える）。
const CIRCLE_MAP_RASTER_SCALE: f32 = 2.0;

impl ChecklistView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            events: Vec::new(),
            selected_slug: None,
            items: Vec::new(),
            favorite_circles: HashSet::new(),
            sample_progress: HashMap::new(),
            busy: false,
            error: None,
            toast: None,
            last_sync: None,
            sort_field: SortField::SpaceNumber,
            sort_ascending: true,
            show_other_first_appearance: false,
            page: 0,
            visible_older_count: 0,
            thumbnail_images: HashMap::new(),
            thumbnail_order: VecDeque::new(),
            evicted_thumb_images: Vec::new(),
            thumbnail_fetching: HashSet::new(),
            fetch_failed: HashSet::new(),
            auto_sync_attempted: HashSet::new(),
            circle_map_images: HashMap::new(),
            circle_map_fetching: HashSet::new(),
            circle_map_error: HashMap::new(),
            show_circle_map: false,
            circle_map_zoom: 1.0,
            circle_map_fit: false,
            circle_map_pan: gpui_kit::Point::new(0.0, 0.0),
            circle_map_drag_start: None,
            circle_map_pan_velocity: gpui_kit::Point::new(0.0, 0.0),
            circle_map_inertia_generation: 0,
            circle_map_last_click: None,
            circle_map_click_moved: false,
            circle_map_bounds: std::rc::Rc::new(std::cell::Cell::new(None)),
            circle_map_viewport_bounds: std::rc::Rc::new(std::cell::Cell::new(None)),
            circle_map_wheel_anchor: None,
            circle_map_zoom_target: None,
            circle_map_pan_target: None,
            circle_map_zoom_animation_generation: 0,
            circle_map_cells: HashMap::new(),
            selected_item_id: None,
            checklist_scroll_handle: ScrollHandle::new(),
        };
        view.reload(cx);
        view
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        // 設定の「ローカルデータをすべて削除」のあとは、自動同期の試行履歴を捨てる
        // （同じイベントを開き直したときに、もう一度取りに行けるように）
        if AppState::global(cx)
            .local_data_deleted
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.auto_sync_attempted.clear();
        }
        let (events, items, last_sync, selected, favorite_circles) = {
            let state = AppState::global(cx);
            let db = &state.db_pool;
            // 同期前でも canonical イベント（技術書典21 など）を一覧に出す。
            // 既存行は書き換えない（利用者のポーリング設定を保持する）。
            if let Err(error) = tbf::sync::ensure_canonical_events(db) {
                log::warn!("canonical tbf events の種まきに失敗: {error}");
            }
            let events = db::checklist::list_events(db).unwrap_or_default();
            let selected = self.selected_slug.clone();
            let items = match &selected {
                Some(slug) => db::checklist::list_items(db, slug).unwrap_or_default(),
                None => Vec::new(),
            };
            let last_sync = db::settings::get(db, "api.last_sync_at").ok().flatten();
            let favorite_circles: HashSet<String> =
                db::favorites::list_favorites(db, db::favorites::EntityKind::Circle)
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
            (events, items, last_sync, selected, favorite_circles)
        };
        self.selected_slug = selected;
        self.events = events;
        self.items = items;
        self.favorite_circles = favorite_circles;
        self.last_sync = last_sync;
        // 本が変わったら自作マップの色付けも作り直す（表示中だけ）。
        if self.show_circle_map
            && let Some(slug) = self.selected_slug.clone()
        {
            self.circle_map_images.remove(&slug);
            self.build_circle_map(cx, slug);
        }
        // 表紙の取得・デコードはイベント詳細を開いたときだけ行う。
        // 起動時（イベント未選択）に何十枚もデコードして UI を重くしない。
        if self.selected_slug.is_some() {
            self.fetch_missing_thumbnails(cx);
            self.decode_cached_thumbnails(cx);
        }
        cx.notify();
    }

    /// 現在の並び替え・ページ設定で表示中の項目（Web の PAGE_SIZE = 50 件）。
    /// サムネイルのデコード対象と、追い出しから守る範囲を揃えるために使う。
    fn page_items(&self) -> Vec<CheckedItem> {
        let mut sorted = self.items.clone();
        sorted.sort_by(|a, b| {
            let ordering = match self.sort_field {
                SortField::CircleName => a.circle_name.cmp(&b.circle_name),
                SortField::SpaceNumber => a.space_number.cmp(&b.space_number),
                SortField::SortOrder => a.sort_order.cmp(&b.sort_order),
            };
            if self.sort_ascending {
                ordering
            } else {
                ordering.reverse()
            }
        });
        const PAGE_SIZE: usize = 50;
        let start = self.page * PAGE_SIZE;
        sorted.into_iter().skip(start).take(PAGE_SIZE).collect()
    }

    /// 選択中イベントが初出の項目（「初出以外も表示」が ON なら全項目）。
    fn visible_items(&self, event_name: &str) -> Vec<CheckedItem> {
        if self.show_other_first_appearance {
            self.items.clone()
        } else {
            filter_first_appearance(&self.items, event_name)
        }
    }

    /// 選択中イベントの項目を、現在の並び順で返す。
    fn sorted_display_items(&self, event_name: &str) -> Vec<CheckedItem> {
        let mut items = self.visible_items(event_name);
        items.sort_by(|a, b| {
            let ordering = match self.sort_field {
                SortField::CircleName => a.circle_name.cmp(&b.circle_name),
                SortField::SpaceNumber => compare_space_numbers(&a.space_number, &b.space_number),
                SortField::SortOrder => a.sort_order.cmp(&b.sort_order),
            };
            if self.sort_ascending {
                ordering
            } else {
                ordering.reverse()
            }
        });
        items
    }

    /// DB に保存済みの thumbnail_data（base64）をバックグラウンドで
    /// 縮小デコードしてキャッシュする（render で毎回デコードしない）。
    /// デコード対象（thumbnail_data あり・未キャッシュ）を現在表示中の
    /// ページ（50 件）に限定して返す。全件を一括デコードしない。
    fn decode_pending(&self) -> Vec<(String, String)> {
        self.page_items()
            .into_iter()
            .filter(|item| {
                item.thumbnail_data.is_some()
                    && !self.thumbnail_images.contains_key(&item.id)
                    && !self.thumbnail_fetching.contains(&item.id)
                    && !self.fetch_failed.contains(&item.id)
            })
            .filter_map(|item| {
                item.thumbnail_data
                    .clone()
                    .map(|data| (item.id.clone(), data))
            })
            .take(20)
            .collect()
    }

    /// デコード済みサムネイルをキャッシュへ追加する（上限つき LRU）。
    /// 上限を超えた分は古い順に追い出し、`evicted_thumb_images` に積む。
    /// ここでは window を持たない（バックグラウンドタスクから呼ばれる）ため、
    /// GPU テクスチャの解放は `release_evicted_thumbs` が描画時に行う。
    fn cache_thumb(&mut self, key: String, image: Arc<RenderImage>) {
        if let Some(previous) = self.thumbnail_images.insert(key.clone(), image) {
            // 同じキーの再挿入（取得とデコードの競合）は古い画像を解放待ちにして、
            // 挿入位置を最新へ移す
            self.evicted_thumb_images.push(previous);
            if let Some(index) = self.thumbnail_order.iter().position(|k| k == &key) {
                self.thumbnail_order.remove(index);
            }
        }
        self.thumbnail_order.push_back(key);
        self.trim_thumb_cache();
    }

    /// キャッシュの上限を超えたサムネイルを古い順に追い出す。
    /// 現在表示中のページの項目は、追い出すと次のデコードまで表紙が消えるため残す。
    fn trim_thumb_cache(&mut self) {
        if self.thumbnail_images.len() <= MAX_THUMB_CACHE {
            return;
        }
        let visible: Vec<String> = self.page_items().into_iter().map(|item| item.id).collect();
        let mut index = 0;
        while self.thumbnail_images.len() > MAX_THUMB_CACHE && index < self.thumbnail_order.len() {
            if visible.contains(&self.thumbnail_order[index]) {
                // 表示中の行は飛ばして、次に古いものを追い出す
                index += 1;
                continue;
            }
            let key = self.thumbnail_order.remove(index).expect("index < len");
            self.evict_thumb(&key);
        }
        // 表示中だけで上限を超えた場合は、古い順に落として上限を守る
        while self.thumbnail_images.len() > MAX_THUMB_CACHE {
            let Some(key) = self.thumbnail_order.pop_front() else {
                break;
            };
            self.evict_thumb(&key);
        }
    }

    /// キャッシュから外し、GPU テクスチャ解放待ちに積む。
    fn evict_thumb(&mut self, key: &str) {
        if let Some(image) = self.thumbnail_images.remove(key) {
            self.evicted_thumb_images.push(image);
        }
    }

    /// 追い出し済みサムネイルの GPU テクスチャを解放する（render から呼ぶ）。
    fn release_evicted_thumbs(&mut self, window: &mut Window) {
        for image in std::mem::take(&mut self.evicted_thumb_images) {
            // GPU 側のテクスチャも解放する（atlas は明示的に消すまで残る）
            let _ = window.drop_image(image);
        }
    }

    fn decode_cached_thumbnails(&mut self, cx: &mut Context<Self>) {
        let pending = self.decode_pending();
        if pending.is_empty() {
            return;
        }
        log::info!("decode_cached_thumbnails: {} pending", pending.len());
        for (id, _) in &pending {
            self.thumbnail_fetching.insert(id.clone());
        }
        let handle = cx.entity();
        let task = cx.background_executor().spawn(async move {
            let mut images: Vec<(String, Arc<RenderImage>)> = Vec::new();
            let mut failed: Vec<String> = Vec::new();
            for (id, data) in pending {
                let bytes = data
                    .strip_prefix("data:image/")
                    .and_then(|rest| rest.split_once(','))
                    .map(|(_, b64)| b64)
                    .unwrap_or(&data);
                let ok = (|| {
                    let decoded =
                        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, bytes)
                            .map_err(|e| e.to_string())?;
                    let image = decode_thumbnail(&decoded).ok_or("decode failed")?;
                    images.push((id.clone(), image));
                    Ok::<(), String>(())
                })();
                if ok.is_err() {
                    failed.push(id);
                }
            }
            (images, failed)
        });
        cx.spawn(async move |_window, cx| {
            let (images, failed) = task.await;
            let count = images.len();
            handle.update(cx, |this, cx| {
                for (id, image) in images {
                    this.thumbnail_fetching.remove(&id);
                    this.cache_thumb(id, image);
                }
                // デコード失敗も記録して毎回の再試行を防ぐ（連鎖の停止）
                for id in failed {
                    this.thumbnail_fetching.remove(&id);
                    this.fetch_failed.insert(id);
                }
                log::info!("decode_cached_thumbnails done: {count} images");
                // 残りは直接続行（reload() は全体再ロードを伴い連鎖を生むので
                // 呼ばない。pending が空なら即 return して終了する）
                this.decode_cached_thumbnails(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// サムネイルが未取得の項目について、thumbnail_url から画像を
    /// ダウンロードして thumbnail_data（base64）に保存する。
    /// メモリ上に持つのは保存用の縮小 JPEG のみで、表示用のデコードは
    /// 描画中のページに限り `decode_cached_thumbnails` が行う。
    fn fetch_missing_thumbnails(&mut self, cx: &mut Context<Self>) {
        // 取得済み/取得中の項目はスキップし、1 度の実行では 20 件まで
        let pending: Vec<(String, String)> = self
            .items
            .iter()
            .filter(|item| {
                item.thumbnail_data.is_none()
                    && item.thumbnail_url.is_some()
                    && !self.thumbnail_fetching.contains(&item.id)
                    && !self.fetch_failed.contains(&item.id)
            })
            .filter_map(|item| item.thumbnail_url.clone().map(|url| (item.id.clone(), url)))
            .take(20)
            .collect();
        if pending.is_empty() {
            return;
        }
        for (id, _) in &pending {
            self.thumbnail_fetching.insert(id.clone());
        }
        let handle = cx.entity();
        let state = AppState::global(cx);
        let db = state.db_pool.clone();
        let task = cx.background_executor().spawn(async move {
            let agent = ureq::Agent::new_with_config(
                ureq::Agent::config_builder()
                    .timeout_connect(Some(std::time::Duration::from_secs(15)))
                    .timeout_per_read(Some(std::time::Duration::from_secs(30)))
                    .build(),
            );
            // ダウンロード → 縮小 JPEG にして DB へ保存する。
            // 表示用のデコードは描画中のページだけ `decode_cached_thumbnails` が行う
            // （全件分の RenderImage をメモリに作らない）
            let mut stored: Vec<(String, String)> = Vec::new();
            let mut failed: Vec<String> = Vec::new();
            for (id, url) in pending {
                let ok = (|| {
                    let resp = agent.get(&url).call().map_err(|e| e.to_string())?;
                    let mut body = Vec::new();
                    resp.into_body()
                        .into_reader()
                        .read_to_end(&mut body)
                        .map_err(|e| e.to_string())?;
                    // 永続化は縮小 JPEG のみ（元画像は保存しない）
                    let jpeg_b64 = thumbnail_jpeg_base64(&body).ok_or("not an image")?;
                    stored.push((id.clone(), jpeg_b64));
                    Ok::<(), String>(())
                })();
                if ok.is_err() {
                    failed.push(id);
                }
            }
            for (id, data) in &stored {
                let _ = db::checklist::update_thumbnail_data(&db, id, data);
                // Drive の表紙バンドル用の共有キャッシュにも入れる（再エンコードしない）
                let _ = ligrol_core::thumbs::cache_checklist_thumbnail(&db, id, data);
            }
            (failed, stored)
        });
        cx.spawn(async move |_window, cx| {
            let (failed, stored) = task.await;
            handle.update(cx, |this, cx| {
                this.apply_fetched_thumbnails(cx, stored, failed);
                // 残りは直接続行（reload() は全体再ロードを伴い連鎖を生むので
                // 呼ばない。pending が空なら即 return して終了する）
                this.fetch_missing_thumbnails(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 取得結果（保存済み base64 / 失敗した項目 ID）を items へ反映し、
    /// 描画中のページのサムネイルだけをデコードし直す。
    /// 非表示ページ分の RenderImage は作らない（メモリに残さない）。
    fn apply_fetched_thumbnails(
        &mut self,
        cx: &mut Context<Self>,
        stored: Vec<(String, String)>,
        failed: Vec<String>,
    ) {
        // 保存済みデータをメモリ上の items にも反映して、
        // 次の reload まで再取得対象にならないようにする
        for (id, data) in stored {
            self.thumbnail_fetching.remove(&id);
            if let Some(item) = self.items.iter_mut().find(|item| item.id == *id) {
                item.thumbnail_data = Some(data);
            }
        }
        // 失敗した項目は記録して再試行しない（無限連鎖の防止）
        for id in failed {
            self.thumbnail_fetching.remove(&id);
            self.fetch_failed.insert(id);
        }
        // 表示中のページのサムネイルをデコードしてキャッシュする
        self.decode_cached_thumbnails(cx);
    }

    fn select_event(&mut self, cx: &mut Context<Self>, slug: &str) {
        self.selected_slug = Some(slug.to_string());
        self.selected_item_id = None;
        self.page = 0;
        // イベント詳細ではサークルマップをデフォルト表示する
        // （取得済み画像は slug ごとに残す）。
        self.show_circle_map = true;
        self.reset_circle_map_view();
        self.reload(cx);
        self.maybe_auto_sync(cx);
    }

    /// 開いたイベントにアイテムが 1 件も無ければ、1 回だけ自動で同期する
    /// （全データ削除や初回インストールのあと、チェックリストが空のままにならないように）。
    /// 未ログインのときは何もしない（`sync()` はログイン要求を出すため、
    /// 画面を開いただけの利用者を邪魔しない）。
    fn maybe_auto_sync(&mut self, cx: &mut Context<Self>) {
        let Some(slug) = self.selected_slug.clone() else {
            return;
        };
        if self.busy || !self.items.is_empty() {
            return;
        }
        if !*AppState::global(cx).tbf_logged_in.lock() {
            return;
        }
        if !self.auto_sync_attempted.insert(slug) {
            return;
        }
        self.sync(cx);
    }

    /// イベント一覧に戻る（Web の「← イベント一覧に戻る」相当）。
    fn back_to_list(&mut self, cx: &mut Context<Self>) {
        self.selected_slug = None;
        self.selected_item_id = None;
        self.show_circle_map = false;
        self.reset_circle_map_view();
        self.reload(cx);
    }

    /// 並び替え（Web の handleSortChange 相当: 同じフィールドなら向きを反転）。
    fn toggle_sort(&mut self, cx: &mut Context<Self>, field: SortField) {
        if self.sort_field == field {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_field = field;
            self.sort_ascending = true;
        }
        self.page = 0;
        cx.notify();
    }

    /// 「初出以外も表示」の ON/OFF。
    fn set_show_other_first_appearance(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.show_other_first_appearance = enabled;
        self.page = 0;
        // 表示中ならマップの色付けも新しい表示集合で作り直す。
        self.reload(cx);
    }

    /// サークルマップの表示を切り替える。未取得なら取得を開始する。
    fn toggle_circle_map(&mut self, cx: &mut Context<Self>) {
        if self.show_circle_map {
            self.show_circle_map = false;
            cx.notify();
            return;
        }
        let Some(slug) = self.selected_slug.clone() else {
            return;
        };
        self.show_circle_map = true;
        self.reset_circle_map_view();
        if !self.circle_map_images.contains_key(&slug) && !self.circle_map_fetching.contains(&slug)
        {
            // 再取得ボタンは置かない。失敗していたら、もう一度開いたときに再試行する。
            self.circle_map_error.remove(&slug);
            self.build_circle_map(cx, slug);
        }
        cx.notify();
    }

    /// サークルマップ（自作 SVG）を生成してキャッシュする。
    ///
    /// 技術書典サイトの画像は使わない。`tbf21` は固定レイアウト
    /// （`assets/tbf/tbf21-layout.svg`）をそのまま使い、それ以外のイベントは
    /// チェックリストのスペース番号から簡易マップを組み立てる。
    fn build_circle_map(&mut self, cx: &mut Context<Self>, slug: String) {
        if self.circle_map_fetching.contains(&slug) {
            return;
        }
        self.circle_map_fetching.insert(slug.clone());
        self.circle_map_error.remove(&slug);
        let event_name = self
            .events
            .iter()
            .find(|event| event.slug.as_deref() == Some(slug.as_str()))
            .map(|event| event.event_name.clone())
            .unwrap_or_else(|| slug.clone());
        // 一覧に出しているチェックリスト項目だけをマップへ反映する。
        let displayed_items = self.visible_items(&event_name);
        let spaces: Vec<(String, u32)> = displayed_items
            .iter()
            .filter_map(|item| parse_space(&item.space_number))
            .collect();
        // チェックリストに入っている本があるサークルは、チェック済みとして塗る。
        let checked_spaces: HashSet<(String, u32)> = displayed_items
            .iter()
            .filter_map(|item| parse_space(&item.space_number))
            .collect();
        let favorite_spaces: HashSet<(String, u32)> = displayed_items
            .iter()
            .filter(|item| self.favorite_circles.contains(&item.circle_name))
            .filter_map(|item| parse_space(&item.space_number))
            .collect();
        let selected_space = self
            .selected_item_id
            .as_deref()
            .and_then(|id| displayed_items.iter().find(|item| item.id == id))
            .and_then(|item| parse_space(&item.space_number));
        let svg = build_circle_map_svg_with_highlights(
            &slug,
            &event_name,
            &spaces,
            &checked_spaces,
            &favorite_spaces,
            selected_space.as_ref(),
        );
        let clickable: HashSet<(String, u32)> = checked_spaces
            .union(&favorite_spaces)
            .cloned()
            .collect();
        let rects = if slug == "tbf21" {
            tbf21_cell_rects(&svg, &clickable, CIRCLE_MAP_RASTER_SCALE)
        } else {
            HashMap::new()
        };
        self.circle_map_cells.insert(slug.clone(), rects);
        let handle = cx.entity();
        let task: gpui_kit::Task<Result<Arc<RenderImage>, String>> =
            cx.background_executor().spawn(async move {
                rasterize_svg_scaled(&svg, CIRCLE_MAP_RASTER_SCALE)
                    .ok_or_else(|| "サークルマップを描画できませんでした".into())
            });
        cx.spawn(async move |_window, cx| {
            let result = task.await;
            handle.update(cx, |this, cx| {
                this.circle_map_fetching.remove(&slug);
                match result {
                    Ok(image) => {
                        this.circle_map_images.insert(slug, image);
                        // 地図を開いた直後は全体表示にして、色付きセルが
                        // 初期クロップの外に出ないようにする。
                        if this.circle_map_fit {
                            this.fit_circle_map(cx);
                        }
                    }
                    Err(message) => {
                        this.circle_map_error.insert(slug, message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 取得済みのサークルマップをキャッシュへ入れる（テスト用）。
    #[cfg(test)]
    fn apply_circle_map(&mut self, slug: &str, image: Arc<RenderImage>) {
        self.circle_map_fetching.remove(slug);
        self.circle_map_error.remove(slug);
        self.circle_map_images.insert(slug.to_string(), image);
    }

    /// 地図モードの倍率・位置を初期状態へ戻す。
    fn reset_circle_map_view(&mut self) {
        self.circle_map_zoom = 1.0;
        self.circle_map_fit = true;
        self.circle_map_pan = gpui_kit::Point::new(0.0, 0.0);
        self.circle_map_drag_start = None;
        self.circle_map_pan_velocity = gpui_kit::Point::new(0.0, 0.0);
        self.circle_map_inertia_generation = self.circle_map_inertia_generation.wrapping_add(1);
        self.circle_map_last_click = None;
        self.circle_map_wheel_anchor = None;
        self.stop_circle_map_zoom_animation();
    }

    /// 選択中イベントの配置図の原寸（px）。
    fn circle_map_image_size(&self) -> Option<(f32, f32)> {
        let slug = self.selected_slug.as_ref()?;
        let image = self.circle_map_images.get(slug)?;
        let size = image.size(0);
        Some((size.width.0 as f32, size.height.0 as f32))
    }

    /// 地図モードの平行移動を clamp する。
    ///
    /// 画像の端で止めると端のブロック（あ・ち など）が中央に来ないので、
    /// 地図のどの位置でもビューポート中央へ寄せられる範囲まで許す
    /// （画像の外側に余白が出る）。
    fn clamp_circle_map_pan(&mut self) {
        let Some((width, height)) = self.circle_map_image_size() else {
            self.circle_map_pan = gpui_kit::Point::new(0.0, 0.0);
            return;
        };
        let max_x = width * self.circle_map_zoom / 2.0;
        let max_y = height * self.circle_map_zoom / 2.0;
        self.circle_map_pan.x = self.circle_map_pan.x.clamp(-max_x, max_x);
        self.circle_map_pan.y = self.circle_map_pan.y.clamp(-max_y, max_y);
    }

    /// チェックリスト項目を選択し、表示中ならマップの選択色も更新する。
    fn select_checklist_item(&mut self, item_id: &str, cx: &mut Context<Self>) {
        // 地図画像は作り直さず、選択オーバーレイだけを更新する（チラつき防止）。
        self.selected_item_id = Some(item_id.to_string());
        self.center_selected_circle_map_cell(cx);
        cx.notify();
    }

    /// 選択中の項目のサークルセルが地図の中央に来るよう平行移動する。
    fn center_selected_circle_map_cell(&mut self, cx: &mut Context<Self>) {
        if !self.show_circle_map {
            return;
        }
        let Some(slug) = self.selected_slug.clone() else {
            return;
        };
        let Some(item_id) = self.selected_item_id.clone() else {
            return;
        };
        let Some((width, height)) = self.circle_map_image_size() else {
            return;
        };
        let Some(space) = self
            .items
            .iter()
            .find(|item| item.id == item_id)
            .and_then(|item| parse_space(&item.space_number))
        else {
            return;
        };
        let Some(rect) = self
            .circle_map_cells
            .get(&slug)
            .and_then(|cells| cells.get(&space))
            .copied()
        else {
            return;
        };
        let zoom = self.circle_map_zoom;
        let center_x = (rect.x0 + rect.x1) / 2.0 * zoom;
        let center_y = (rect.y0 + rect.y1) / 2.0 * zoom;
        // 画面座標 = (viewport - display)/2 + pan + rect*zoom なので、
        // セル中心をビューポート中央へ持ってくる pan を逆算する。
        self.circle_map_pan = gpui_kit::Point::new(
            width * zoom / 2.0 - center_x,
            height * zoom / 2.0 - center_y,
        );
        self.circle_map_fit = false;
        self.circle_map_pan_velocity = gpui_kit::Point::new(0.0, 0.0);
        self.circle_map_inertia_generation = self.circle_map_inertia_generation.wrapping_add(1);
        self.clamp_circle_map_pan();
        cx.notify();
    }

    /// 地図の色付きセルをクリックしたら、対応するチェックリスト行を選択する。
    fn select_circle_map_cell(
        &mut self,
        position: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(slug) = self.selected_slug.clone() else {
            return;
        };
        let Some(bounds) = self.circle_map_bounds.get() else {
            return;
        };
        if bounds.size.width.as_f32() <= 0.0 || bounds.size.height.as_f32() <= 0.0 {
            return;
        }
        let local_x = (position.x - bounds.origin.x).as_f32();
        let local_y = (position.y - bounds.origin.y).as_f32();
        if local_x < 0.0
            || local_y < 0.0
            || local_x > bounds.size.width.as_f32()
            || local_y > bounds.size.height.as_f32()
        {
            return;
        }
        let Some((image_width, image_height)) = self.circle_map_image_size() else {
            return;
        };
        let image_x = local_x / bounds.size.width.as_f32() * image_width;
        let image_y = local_y / bounds.size.height.as_f32() * image_height;
        let Some(cells) = self.circle_map_cells.get(&slug) else {
            return;
        };
        let Some(space) = cells
            .iter()
            .find(|(_, rect)| {
                image_x >= rect.x0
                    && image_x <= rect.x1
                    && image_y >= rect.y0
                    && image_y <= rect.y1
            })
            .map(|(space, _)| space.clone())
        else {
            return;
        };
        let event_name = self
            .events
            .iter()
            .find(|event| event.slug.as_deref() == Some(slug.as_str()))
            .map(|event| event.event_name.as_str())
            .unwrap_or_default();
        let displayed = self.sorted_display_items(event_name);
        let Some((index, item)) = displayed.iter().enumerate().find(|(_, item)| {
            parse_space(&item.space_number).is_some_and(|space_number| space_number == space)
        }) else {
            return;
        };
        self.page = index / 50;
        // 子 0 はテーブルのヘッダー。選択行が表示範囲外なら最小限スクロールする。
        self.checklist_scroll_handle
            .scroll_to_item(index % 50 + 1);
        self.select_checklist_item(&item.id, cx);
    }

    /// 地図モードの倍率を掛け算で変更する（ホイール・ボタン共用）。
    fn zoom_circle_map(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.zoom_circle_map_at(
            factor,
            gpui_kit::Point::new(
                CIRCLE_MAP_VIEWPORT_WIDTH / 2.0,
                CIRCLE_MAP_VIEWPORT_HEIGHT / 2.0,
            ),
            cx,
        );
    }

    /// ビューポート内の `anchor` を固定したまま倍率を変更する。
    ///
    /// 画面座標 = (viewport - display)/2 + pan + 画像座標*zoom なので、
    /// anchor 直下の画像座標が変わらない pan を逆算する。
    fn zoom_circle_map_at(
        &mut self,
        factor: f32,
        anchor: gpui_kit::Point<f32>,
        cx: &mut Context<Self>,
    ) {
        self.circle_map_wheel_anchor = None;
        self.stop_circle_map_zoom_animation();
        let old_zoom = self.circle_map_zoom;
        let new_zoom = (old_zoom * factor).clamp(CIRCLE_MAP_MIN_ZOOM, CIRCLE_MAP_MAX_ZOOM);
        if (new_zoom - old_zoom).abs() < f32::EPSILON {
            return;
        }
        let ratio = new_zoom / old_zoom;
        let offset_x = anchor.x - CIRCLE_MAP_VIEWPORT_WIDTH / 2.0;
        let offset_y = anchor.y - CIRCLE_MAP_VIEWPORT_HEIGHT / 2.0;
        self.circle_map_pan.x = self.circle_map_pan.x * ratio + offset_x * (1.0 - ratio);
        self.circle_map_pan.y = self.circle_map_pan.y * ratio + offset_y * (1.0 - ratio);
        self.circle_map_zoom = new_zoom;
        self.circle_map_fit = false;
        self.circle_map_pan_velocity = gpui_kit::Point::new(0.0, 0.0);
        self.circle_map_inertia_generation = self.circle_map_inertia_generation.wrapping_add(1);
        self.clamp_circle_map_pan();
        cx.notify();
    }

    /// 実行中のホイールズームアニメーションを止める。
    fn stop_circle_map_zoom_animation(&mut self) {
        self.circle_map_zoom_target = None;
        self.circle_map_pan_target = None;
        self.circle_map_zoom_animation_generation = self
            .circle_map_zoom_animation_generation
            .wrapping_add(1);
    }

    /// ホイール 1 ジェスチャー中に固定する画像座標を返す。
    ///
    /// 同じ画面位置なら、最初に掴んだ地点を再利用する。毎回その時のカーソル下を
    /// 拾うと、中央へ寄せた次のイベントで別の地点を掴んでしまい、ズームのたびに
    /// 地図が流れる。マウスを動かした時点で新しい地点を掴み直す。
    fn wheel_zoom_anchor(
        &mut self,
        local_mouse: gpui_kit::Point<f32>,
        width: f32,
        height: f32,
    ) -> gpui_kit::Point<f32> {
        let reuse = self.circle_map_wheel_anchor.filter(|(screen, _)| {
            (screen.x - local_mouse.x).abs() <= 4.0
                && (screen.y - local_mouse.y).abs() <= 4.0
        });
        if let Some((_, anchor)) = reuse {
            return anchor;
        }
        let zoom = self.circle_map_zoom;
        let anchor = gpui_kit::Point::new(
            (local_mouse.x
                - (CIRCLE_MAP_VIEWPORT_WIDTH - width * zoom) / 2.0
                - self.circle_map_pan.x)
                / zoom,
            (local_mouse.y
                - (CIRCLE_MAP_VIEWPORT_HEIGHT - height * zoom) / 2.0
                - self.circle_map_pan.y)
                / zoom,
        );
        self.circle_map_wheel_anchor = Some((local_mouse, anchor));
        anchor
    }

    /// ホイールの拡大縮小。マウス直下の地点をビューポート中央へ寄せる。
    ///
    /// 目標値だけを更新し、16ms ごとに現在値を目標へ寄せる。1 ノッチで
    /// 大きく pan が飛ぶとカクつくため、pan と倍率を同時に補間する。
    fn zoom_circle_map_to_wheel_anchor(
        &mut self,
        local_mouse: gpui_kit::Point<f32>,
        viewport_center: gpui_kit::Point<f32>,
        factor: f32,
        cx: &mut Context<Self>,
    ) {
        let Some((width, height)) = self.circle_map_image_size() else {
            return;
        };
        let base_zoom = self.circle_map_zoom_target.unwrap_or(self.circle_map_zoom);
        let target_zoom = (base_zoom * factor).clamp(CIRCLE_MAP_MIN_ZOOM, CIRCLE_MAP_MAX_ZOOM);
        if (target_zoom - base_zoom).abs() < f32::EPSILON {
            return;
        }
        let anchor = self.wheel_zoom_anchor(local_mouse, width, height);
        let max_x = width * target_zoom / 2.0;
        let max_y = height * target_zoom / 2.0;
        let target_pan = gpui_kit::Point::new(
            (viewport_center.x
                - (CIRCLE_MAP_VIEWPORT_WIDTH - width * target_zoom) / 2.0
                - anchor.x * target_zoom)
                .clamp(-max_x, max_x),
            (viewport_center.y
                - (CIRCLE_MAP_VIEWPORT_HEIGHT - height * target_zoom) / 2.0
                - anchor.y * target_zoom)
                .clamp(-max_y, max_y),
        );
        self.circle_map_zoom_target = Some(target_zoom);
        self.circle_map_pan_target = Some(target_pan);
        self.circle_map_fit = false;
        self.circle_map_pan_velocity = gpui_kit::Point::new(0.0, 0.0);
        self.circle_map_inertia_generation = self.circle_map_inertia_generation.wrapping_add(1);
        let generation = self
            .circle_map_zoom_animation_generation
            .wrapping_add(1);
        self.circle_map_zoom_animation_generation = generation;
        let handle = cx.entity();
        cx.spawn(async move |_window, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
                let done = handle.update(cx, |this, cx| {
                    if this.circle_map_zoom_animation_generation != generation {
                        return true;
                    }
                    let (Some(target_zoom), Some(target_pan)) =
                        (this.circle_map_zoom_target, this.circle_map_pan_target)
                    else {
                        return true;
                    };
                    const SMOOTHING: f32 = 0.35;
                    this.circle_map_zoom += (target_zoom - this.circle_map_zoom) * SMOOTHING;
                    this.circle_map_pan.x += (target_pan.x - this.circle_map_pan.x) * SMOOTHING;
                    this.circle_map_pan.y += (target_pan.y - this.circle_map_pan.y) * SMOOTHING;
                    let close = (target_zoom - this.circle_map_zoom).abs() < 0.0005
                        && (target_pan.x - this.circle_map_pan.x).abs() < 0.3
                        && (target_pan.y - this.circle_map_pan.y).abs() < 0.3;
                    if close {
                        this.circle_map_zoom = target_zoom;
                        this.circle_map_pan = target_pan;
                        this.circle_map_zoom_target = None;
                        this.circle_map_pan_target = None;
                    }
                    cx.notify();
                    close
                });
                if done {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// ドラッグを離したときに慣性で流す（Google マップ風）。
    fn end_circle_map_pan(&mut self, cx: &mut Context<Self>) {
        self.circle_map_drag_start = None;
        let velocity = self.circle_map_pan_velocity;
        if velocity.x.abs() < 40.0 && velocity.y.abs() < 40.0 {
            self.circle_map_pan_velocity = gpui_kit::Point::new(0.0, 0.0);
            cx.notify();
            return;
        }
        self.circle_map_inertia_generation = self.circle_map_inertia_generation.wrapping_add(1);
        let generation = self.circle_map_inertia_generation;
        let handle = cx.entity();
        cx.spawn(async move |_window, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
                let stop = handle.update(cx, |this, cx| {
                    if this.circle_map_inertia_generation != generation {
                        return true;
                    }
                    let velocity = this.circle_map_pan_velocity;
                    if velocity.x.abs() < 15.0 && velocity.y.abs() < 15.0 {
                        this.circle_map_pan_velocity = gpui_kit::Point::new(0.0, 0.0);
                        return true;
                    }
                    this.circle_map_pan.x += velocity.x * 0.016;
                    this.circle_map_pan.y += velocity.y * 0.016;
                    this.clamp_circle_map_pan();
                    // 減速をゆるめにして、少し長く滑る（Google マップ寄り）。
                    this.circle_map_pan_velocity.x *= 0.92;
                    this.circle_map_pan_velocity.y *= 0.92;
                    cx.notify();
                    false
                });
                if stop {
                    break;
                }
            }
        })
        .detach();
    }

    /// 配置図全体が表示領域に収まる倍率へ戻す。
    fn fit_circle_map(&mut self, cx: &mut Context<Self>) {
        let Some((width, height)) = self.circle_map_image_size() else {
            return;
        };
        let fit = (CIRCLE_MAP_VIEWPORT_WIDTH / width)
            .min(CIRCLE_MAP_VIEWPORT_HEIGHT / height)
            .clamp(CIRCLE_MAP_MIN_ZOOM, CIRCLE_MAP_MAX_ZOOM);
        self.circle_map_zoom = fit;
        self.circle_map_fit = true;
        self.circle_map_pan = gpui_kit::Point::new(0.0, 0.0);
        self.circle_map_pan_velocity = gpui_kit::Point::new(0.0, 0.0);
        self.circle_map_inertia_generation = self.circle_map_inertia_generation.wrapping_add(1);
        self.circle_map_wheel_anchor = None;
        self.stop_circle_map_zoom_animation();
        cx.notify();
    }

    /// 選択中イベントのチェックリストを同期する。
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        // サンプル（デモ）モードはネットワークへ出ない（ダミーのセッションで
        // 本物の技術書典を叩かない。叩いても「セッション切れ」の通知が出るだけ）。
        if AppState::global(cx).demo {
            return;
        }
        let logged_in = *AppState::global(cx).tbf_logged_in.lock();
        if !logged_in {
            self.toast = Some("ログインしてから同期してください".into());
            cx.defer(move |cx| cx.dispatch_action(&crate::actions::OpenAuth));
            cx.notify();
            return;
        }
        let Some(slug) = self.selected_slug.clone() else {
            return;
        };
        self.busy = true;
        self.error = None;
        let handle = cx.entity();
        let state = AppState::global(cx);
        let tbf_client = state.tbf.clone();
        let db = state.db_pool.clone();
        let slug_for_task = slug.clone();
        let owner = crate::app_state::owner_token(state);
        let task: gpui_kit::Task<Result<usize, String>> =
            cx.background_executor().spawn(async move {
                let mut client = tbf_client.lock();
                let outcome = tbf::sync::refresh_checklist(
                    &db,
                    &mut client,
                    &slug_for_task,
                    owner.as_deref(),
                )
                .map_err(|e| e.to_string())?;
                Ok(outcome.count)
            });
        cx.spawn(async move |_window, cx| {
            let result = task.await;
            handle.update(cx, |this, cx| this.finish_sync(cx, result));
        })
        .detach();
    }

    /// 同期タスクの完了処理（結果の反映と、選択中イベントでの自動同期の再判定）。
    ///
    /// 同期中に別のイベントを開くと `maybe_auto_sync` は `busy` で見送られるため、
    /// 完了時にもう一度判定する（実行中の同期が終わった時点で取得が始まる）。
    fn finish_sync(&mut self, cx: &mut Context<Self>, result: Result<usize, String>) {
        self.busy = false;
        match result {
            Ok(count) => {
                log::info!("sync done: {count} entries");
                self.toast = Some(format!("チェックリストを同期しました（{count} 件）"));
                self.reload(cx);
            }
            Err(message) => {
                self.error = Some(message.clone());
                if message.contains("session expired") {
                    cx.defer(move |cx| cx.dispatch_action(&crate::actions::OpenAuth));
                }
            }
        }
        self.maybe_auto_sync(cx);
        cx.notify();
    }

    /// お気に入り（checkedProductInfos）をチェックリストへ取り込む。
    pub fn import_favorites(&mut self, cx: &mut Context<Self>) {
        // サンプル（デモ）モードはネットワークへ出ない（`sync` と同じ理由）。
        if AppState::global(cx).demo {
            return;
        }
        let logged_in = *AppState::global(cx).tbf_logged_in.lock();
        if !logged_in {
            self.toast = Some("ログインしてから実行してください".into());
            cx.defer(move |cx| cx.dispatch_action(&crate::actions::OpenAuth));
            cx.notify();
            return;
        }
        let Some(slug) = self.selected_slug.clone() else {
            return;
        };
        self.busy = true;
        self.error = None;
        let handle = cx.entity();
        let state = AppState::global(cx);
        let tbf_client = state.tbf.clone();
        let db = state.db_pool.clone();
        let slug_for_task = slug.clone();
        let owner = crate::app_state::owner_token(state);
        let task: gpui_kit::Task<Result<usize, String>> =
            cx.background_executor().spawn(async move {
                let mut client = tbf_client.lock();
                let entries = client
                    .favorites(&slug_for_task)
                    .map_err(|e| e.to_string())?;
                tbf::sync::save_checklist(&db, &slug_for_task, &entries, owner.as_deref())
                    .map_err(|e| e.to_string())?;
                Ok(entries.len())
            });
        cx.spawn(async move |_window, cx| {
            let result = task.await;
            handle.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(count) => {
                        this.toast = Some(format!("お気に入りを {count} 件取り込みました"));
                        this.reload(cx);
                    }
                    Err(message) => {
                        this.error = Some(message.clone());
                        if message.contains("session expired") {
                            cx.defer(move |cx| cx.dispatch_action(&crate::actions::OpenAuth));
                        }
                    }
                }
                // 取り込み中に別のイベントを開いていた場合に備えて再判定する
                this.maybe_auto_sync(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// チェック状態をトグルする。
    pub fn toggle_item(&mut self, cx: &mut Context<Self>, item_id: &str) {
        let (next, slug) = {
            let state = AppState::global(cx);
            let db = &state.db_pool;
            let current = db::checklist::get_item(db, item_id)
                .ok()
                .flatten()
                .map(|item| item.is_checked != 0)
                .unwrap_or(false);
            let slug = self.selected_slug.clone().unwrap_or_default();
            let _ = db::checklist::set_checked(db, item_id, !current);
            (!current, slug)
        };
        let _ = slug;
        let _ = next;
        self.reload(cx);
    }

    /// 試し読みページを取得してビューアーで開く。
    pub fn fetch_sample(&mut self, cx: &mut Context<Self>, item: CheckedItem) {
        log::info!("fetch_sample: {}", item.id);
        let Some(product_id) = item.product_id.clone() else {
            return;
        };
        self.busy = true;
        self.error = None;
        let handle = cx.entity();
        let state = AppState::global(cx);
        let tbf_client = state.tbf.clone();
        let db = state.db_pool.clone();
        let item_id = item.id.clone();
        let open_item_id = item_id.clone();
        // 取得中はその行の表紙に進捗リングを出す（0.0 から始めて完了時に消す）
        self.sample_progress.insert(item_id.clone(), 0.0);
        let (progress_tx, progress_rx) = std::sync::mpsc::channel::<(String, f32)>();
        type SamplePages = Vec<(String, u32, u32)>;
        let task: gpui_kit::Task<Result<SamplePages, String>> =
            cx.background_executor().spawn(async move {
                let mut client = tbf_client.lock();
                let pages = client
                    .product_sample_pages(&product_id)
                    .map_err(|e| e.to_string())?;
                let page_count = pages.len();
                let mut stored: Vec<(String, u32, u32)> = Vec::new();
                // 旧ページを先に 1 回だけ削除してから全ページを保存する
                {
                    let _ = db::samples::delete_for_item(&db, &item_id);
                }
                for (index, page) in pages.into_iter().enumerate() {
                    // 試し読み画像は本体（`/api/product-dlc/`）とは別の許可リストを通す
                    // （自サイトの画像 `/api/image/`。本体用のリストでは全部ブロックされる）
                    let bytes = {
                        let report_tx = progress_tx.clone();
                        let report_id = item_id.clone();
                        let mut on_progress = move |downloaded: u64, total: u64| {
                            report_tx
                                .send((
                                    report_id.clone(),
                                    sample_progress_fraction(index, page_count, downloaded, total),
                                ))
                                .is_ok()
                        };
                        client
                            .download_site_image_with_progress(&page.url, &mut on_progress)
                            .map_err(|e| e.to_string())?
                    };
                    use base64::Engine;
                    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    let (width, height) = match (page.width, page.height) {
                        (Some(w), Some(h)) => (w, h),
                        _ => decode_size(&bytes).unwrap_or((0, 0)),
                    };
                    {
                        let _ = db::samples::insert_sample_page(
                            &db,
                            &db::samples::SamplePageRow {
                                id: uuid::Uuid::new_v4().to_string(),
                                checklist_item_id: item_id.clone(),
                                product_id: Some(product_id.clone()),
                                page_number: page.page_number,
                                image_url: Some(page.url.clone()),
                                image_data: Some(data.clone()),
                                mime_type: "image/jpeg".to_string(),
                                width: Some(width),
                                height: Some(height),
                                file_size: Some(bytes.len() as i64),
                                fetched_at: chrono::Utc::now().to_rfc3339(),
                            },
                        );
                    }
                    // ページを読み切ったぶんを確定させる（`Content-Length` が無い画像でも進む）
                    let _ = progress_tx.send((
                        item_id.clone(),
                        sample_progress_fraction(index + 1, page_count, 0, 1),
                    ));
                    stored.push((data, width as u32, height as u32));
                }
                Ok(stored)
            });
        cx.spawn(async move |_window, cx| {
            // worker の進捗を UI へ流す（チャネルが閉じるまで）。値が変わったときだけ描き直す。
            let mut disconnected = false;
            while !disconnected {
                let mut latest: Option<(String, f32)> = None;
                loop {
                    match progress_rx.try_recv() {
                        Ok(progress) => latest = Some(progress),
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
                if let Some((id, fraction)) = latest {
                    handle.update(cx, |this, cx| {
                        if this.sample_progress.get(&id).copied() != Some(fraction) {
                            this.sample_progress.insert(id, fraction);
                            cx.notify();
                        }
                    });
                }
                if !disconnected {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(50))
                        .await;
                }
            }
            let result = task.await;
            handle.update(cx, |this, cx| {
                this.busy = false;
                this.sample_progress.remove(&open_item_id);
                match result {
                    Ok(pages) if !pages.is_empty() => {
                        // 同じウィンドウのリーダーで開く（workspace が処理）
                        let action = crate::actions::OpenSampleReader {
                            item_id: open_item_id.clone().into(),
                        };
                        cx.defer(move |cx| cx.dispatch_action(&action));
                    }
                    Ok(_) => this.error = Some("試し読み画像がありません".into()),
                    Err(message) => {
                        this.error = Some(message.clone());
                        if message.contains("session expired") {
                            cx.defer(move |cx| cx.dispatch_action(&crate::actions::OpenAuth));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn decode_size(bytes: &[u8]) -> Option<(i64, i64)> {
    let image = image::load_from_memory(bytes).ok()?;
    Some((image.width() as i64, image.height() as i64))
}

/// 試し読み画像を同じウィンドウのリーダーで開く（workspace が処理）。
impl Render for ChecklistView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 追い出し済みサムネイルの GPU テクスチャを解放する（window を持つここで行う）
        self.release_evicted_thumbs(window);
        // Web と同じ 2 画面構成: 選択中イベントがなければ一覧、あれば詳細
        if self.selected_slug.is_none() {
            self.render_event_list(window, cx).into_any_element()
        } else {
            self.render_event_detail(window, cx).into_any_element()
        }
    }
}

impl ChecklistView {
    /// イベントの日付を「YYYY/MM/DD」形式に整形（Web の toLocaleDateString("ja-JP") 相当）。
    fn format_date_ja(&self, date: &str) -> String {
        let digits: Vec<&str> = date.split('-').collect();
        if digits.len() == 3 {
            format!("{}/{}/{}", digits[0], digits[1], digits[2])
        } else {
            date.to_string()
        }
    }

    /// Web の formatEventDate 相当。
    fn format_event_date(&self, event: &TbfEvent) -> String {
        if event.is_cancelled != 0 {
            return event
                .event_date
                .as_deref()
                .map(|d| format!("{} (中止)", self.format_date_ja(d)))
                .unwrap_or_else(|| "日付未設定".to_string());
        }
        if event.event_format == "online"
            && let (Some(start), Some(end)) = (
                event.event_start_date.as_deref(),
                event.event_end_date.as_deref(),
            )
        {
            return format!(
                "{} 〜 {}",
                self.format_date_ja(start),
                self.format_date_ja(end)
            );
        }
        event
            .event_date
            .as_deref()
            .map(|d| self.format_date_ja(d))
            .unwrap_or_else(|| "日付未設定".to_string())
    }

    /// イベントの日付値（ソート用。日付なしは負の無限大相当）。
    fn event_date_value(&self, event: &TbfEvent) -> (i32, i32, i32) {
        event
            .event_date
            .as_deref()
            .and_then(|d| {
                let parts: Vec<&str> = d.split('-').collect();
                if parts.len() == 3 {
                    Some((
                        parts[0].parse().unwrap_or(0),
                        parts[1].parse().unwrap_or(0),
                        parts[2].parse().unwrap_or(0),
                    ))
                } else {
                    None
                }
            })
            .unwrap_or((0, 0, 0))
    }

    /// イベント一覧（Web の ChecklistEventList 相当）。
    fn render_event_list(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let handle = cx.entity();
        let events = self.events.clone();
        let visible_older_count = self.visible_older_count;
        let border = cx.theme().border;
        let muted = cx.theme().muted;
        let muted_fg = cx.theme().muted_foreground;
        let primary = cx.theme().primary;
        let danger = cx.theme().danger;

        // ソート: featured 先頭、非 featured は日付降順（Web と同じ）
        let mut featured: Vec<TbfEvent> = Vec::new();
        let mut non_featured: Vec<TbfEvent> = Vec::new();
        for event in &events {
            if event.is_featured != 0 {
                featured.push(event.clone());
            } else {
                non_featured.push(event.clone());
            }
        }
        non_featured.sort_by_key(|e| std::cmp::Reverse(self.event_date_value(e)));
        // tbf17 より前は「これより以前のイベントを表示」で段階表示
        let boundary = non_featured
            .iter()
            .position(|e| e.slug.as_deref() == Some("tbf17"))
            .unwrap_or(2.min(non_featured.len()));
        let recent = non_featured[..boundary].to_vec();
        let older = non_featured[boundary..].to_vec();
        let visible_older = older[..visible_older_count.min(older.len())].to_vec();
        let has_more_older = older.len() > visible_older_count;

        let card = |event: &TbfEvent| {
            let handle = handle.clone();
            let event = event.clone();
            let name = event.event_name.clone();
            let date_text = self.format_event_date(&event);
            let icon = if event.is_cancelled != 0 {
                Icon::new(IconName::TriangleAlert)
                    .size(px(24.0))
                    .text_color(danger)
                    .into_any_element()
            } else if event.event_format == "online" {
                Icon::new(IconName::Globe)
                    .size(px(24.0))
                    .text_color(primary)
                    .into_any_element()
            } else if event.event_format == "hybrid" {
                Icon::new(IconName::Building2)
                    .size(px(24.0))
                    .text_color(primary)
                    .into_any_element()
            } else {
                Icon::new(IconName::Calendar)
                    .size(px(24.0))
                    .text_color(primary)
                    .into_any_element()
            };
            let slug = event.slug.clone().unwrap_or_else(|| event.id.clone());
            div()
                .id(SharedString::from(format!("event-card-{slug}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_4()
                .rounded_lg()
                .border_1()
                .border_color(border)
                .bg(cx.theme().background)
                .p_4()
                .hover(|style| style.bg(muted.opacity(0.3)))
                .cursor_pointer()
                .on_click(move |_, _window, cx| {
                    handle.update(cx, |this, cx| this.select_event(cx, &slug));
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(48.0))
                        .h(px(48.0))
                        .rounded_full()
                        .bg(primary.opacity(0.1))
                        .child(icon),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(name),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .text_sm()
                                .text_color(muted_fg)
                                .child(
                                    Icon::new(IconName::Calendar)
                                        .size(px(14.0))
                                        .text_color(muted_fg),
                                )
                                .child(date_text),
                        ),
                )
                .child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(20.0))
                        .text_color(muted_fg),
                )
        };

        div()
            .size_full()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .child(
                div()
                    .mx_auto()
                    .w(px(720.0))
                    .py_8()
                    .px_4()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("チェックリスト(イベント一覧)"),
                    )
                    .child(if events.is_empty() {
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .border_1()
                            .border_color(border)
                            .bg(muted.opacity(0.3))
                            .px_4()
                            .py_12()
                            .text_center()
                            .child(
                                div().mb_4().rounded_full().bg(muted).p_4().child(
                                    Icon::new(IconName::Calendar)
                                        .size(px(32.0))
                                        .text_color(muted_fg),
                                ),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("イベントがありません"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(muted_fg)
                                    .child("まだチェックリスト用のイベントがありません。"),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .children(
                                featured
                                    .iter()
                                    .chain(recent.iter())
                                    .chain(visible_older.iter())
                                    .map(|event| card(event).into_any_element()),
                            )
                            .child(if has_more_older {
                                div()
                                    .id("load-more-older-events")
                                    .w_full()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(border)
                                    .px_4()
                                    .py_3()
                                    .text_sm()
                                    .text_color(muted_fg)
                                    .text_center()
                                    .hover(|style| style.bg(muted.opacity(0.3)))
                                    .cursor_pointer()
                                    .on_click({
                                        let handle = handle.clone();
                                        move |_, _window, cx| {
                                            handle.update(cx, |this, cx| {
                                                this.visible_older_count += 5;
                                                cx.notify();
                                            });
                                        }
                                    })
                                    .child("これより以前のイベントを表示")
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .into_any_element()
                    }),
            )
    }

    /// 地図の表示領域（ドラッグ移動・ホイール拡大・慣性スクロール）。
    fn render_circle_map_viewport(&self, handle: gpui_kit::Entity<Self>) -> gpui_kit::AnyElement {
        let Some(slug) = self.selected_slug.as_ref() else {
            return div().into_any_element();
        };
        let Some(image) = self.circle_map_images.get(slug).cloned() else {
            return div().into_any_element();
        };
        let size = image.size(0);
        let (width, height) = (size.width.0 as f32, size.height.0 as f32);
        let zoom = self.circle_map_zoom;
        let pan = self.circle_map_pan;
        let dragging = self.circle_map_drag_start.is_some();
        let down_handle = handle.clone();
        let move_handle = handle.clone();
        let up_handle = handle.clone();
        let out_handle = handle.clone();
        let wheel_handle = handle;
        let display_width = width * zoom;
        let display_height = height * zoom;
        let display_left = (CIRCLE_MAP_VIEWPORT_WIDTH - display_width) / 2.0 + pan.x;
        let display_top = (CIRCLE_MAP_VIEWPORT_HEIGHT - display_height) / 2.0 + pan.y;
        let selected_rect = self
            .selected_item_id
            .as_deref()
            .and_then(|id| {
                let space = self
                    .items
                    .iter()
                    .find(|item| item.id.as_str() == id)
                    .and_then(|item| parse_space(&item.space_number))?;
                self.circle_map_cells
                    .get(slug)
                    .and_then(|cells| cells.get(&space))
                    .copied()
            });
        let selected_overlay = selected_rect.map(|rect| {
            let left = display_left + rect.x0 * zoom;
            let top = display_top + rect.y0 * zoom;
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px((rect.x1 - rect.x0) * zoom))
                .h(px((rect.y1 - rect.y0) * zoom))
                .bg(gpui_kit::rgb(0xef4444).opacity(0.45))
                .border_1()
                .border_color(gpui_kit::rgb(0xdc2626))
                .rounded(px(2.0))
                .into_any_element()
        });
        let viewport_bounds = self.circle_map_viewport_bounds.clone();
        div()
            .w_full()
            .on_children_prepainted(move |children, _window, _cx| {
                if let Some(child) = children.first() {
                    viewport_bounds.set(Some(*child));
                }
            })
            .child(
                div()
            .on_children_prepainted({
                let bounds = self.circle_map_bounds.clone();
                move |children, _window, _cx| {
                    if let Some(child) = children.first() {
                        bounds.set(Some(*child));
                    }
                }
            })
            .id("circle-map-viewport")
            .debug_selector(|| "circle-map-image".into())
            .w_full()
            .h(px(CIRCLE_MAP_VIEWPORT_HEIGHT))
            .relative()
            .overflow_hidden()
            .bg(gpui_kit::white())
            .cursor(if dragging {
                gpui_kit::CursorStyle::ClosedHand
            } else {
                gpui_kit::CursorStyle::OpenHand
            })
            .on_mouse_down(gpui_kit::MouseButton::Left, move |event, _window, cx| {
                let position =
                    gpui_kit::Point::new(event.position.x.as_f32(), event.position.y.as_f32());
                let now = std::time::Instant::now();
                down_handle.update(cx, |this, cx| {
                    let double = this
                        .circle_map_last_click
                        .map(|last| now.duration_since(last).as_millis() < 350)
                        .unwrap_or(false);
                    this.circle_map_last_click = Some(now);
                    if double {
                        this.circle_map_last_click = None;
                        this.circle_map_drag_start = None;
                        this.zoom_circle_map(2.0, cx);
                    } else {
                        this.circle_map_drag_start = Some(position);
                        this.circle_map_wheel_anchor = None;
                        this.stop_circle_map_zoom_animation();
                        this.circle_map_click_moved = false;
                        this.circle_map_pan_velocity = gpui_kit::Point::new(0.0, 0.0);
                        this.circle_map_inertia_generation =
                            this.circle_map_inertia_generation.wrapping_add(1);
                        cx.notify();
                    }
                });
            })
            .on_mouse_move(move |event, _window, cx| {
                let position =
                    gpui_kit::Point::new(event.position.x.as_f32(), event.position.y.as_f32());
                move_handle.update(cx, |this, cx| {
                    if let Some(start) = this.circle_map_drag_start {
                        let dx = position.x - start.x;
                        let dy = position.y - start.y;
                        // ドラッグしたらダブルクリック判定を消す（素早く続けて
                        // ドラッグしたときに 2 回目がズーム扱いにならないように）。
                        if dx.abs() > 2.0 || dy.abs() > 2.0 {
                            this.circle_map_last_click = None;
                            this.circle_map_click_moved = true;
                        }
                        this.circle_map_pan.x += dx;
                        this.circle_map_pan.y += dy;
                        this.circle_map_drag_start = Some(position);
                        // 16ms 換算の速度（px/秒）。離した後に慣性で流す。
                        this.circle_map_pan_velocity = gpui_kit::Point::new(
                            (dx / 0.016).clamp(-4000.0, 4000.0),
                            (dy / 0.016).clamp(-4000.0, 4000.0),
                        );
                        this.clamp_circle_map_pan();
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(gpui_kit::MouseButton::Left, move |event, _window, cx| {
                up_handle.update(cx, |this, cx| {
                    if !this.circle_map_click_moved && this.circle_map_drag_start.is_some() {
                        this.select_circle_map_cell(event.position, cx);
                    }
                    this.end_circle_map_pan(cx);
                });
            })
            .on_mouse_up_out(gpui_kit::MouseButton::Left, move |_, _window, cx| {
                out_handle.update(cx, |this, cx| this.end_circle_map_pan(cx));
            })
            .on_scroll_wheel(move |event, _window, cx| {
                let lines = match event.delta {
                    gpui_kit::ScrollDelta::Pixels(point) => f32::from(point.y) / 40.0,
                    gpui_kit::ScrollDelta::Lines(point) => point.y,
                };
                // 1 ノッチ（3 行）で約 6%。以前は 1 ノッチで 23% 拡大していた。
                let factor = 1.02_f32.powf(lines.clamp(-6.0, 6.0));
                let mouse = gpui_kit::Point::new(
                    event.position.x.as_f32(),
                    event.position.y.as_f32(),
                );
                wheel_handle.update(cx, |this, cx| {
                    let (local_mouse, viewport_center) = this
                        .circle_map_viewport_bounds
                        .get()
                        .map(|bounds| {
                            (
                                gpui_kit::Point::new(
                                    mouse.x - bounds.origin.x.as_f32(),
                                    mouse.y - bounds.origin.y.as_f32(),
                                ),
                                gpui_kit::Point::new(
                                    bounds.size.width.as_f32() / 2.0,
                                    bounds.size.height.as_f32() / 2.0,
                                ),
                            )
                        })
                        .unwrap_or_else(|| {
                            let center = gpui_kit::Point::new(
                                CIRCLE_MAP_VIEWPORT_WIDTH / 2.0,
                                CIRCLE_MAP_VIEWPORT_HEIGHT / 2.0,
                            );
                            (center, center)
                        });
                    this.zoom_circle_map_to_wheel_anchor(
                        local_mouse,
                        viewport_center,
                        factor,
                        cx,
                    );
                });
                cx.stop_propagation();
            })
            .child({
                let display_width = width * zoom;
                let display_height = height * zoom;
                // flex の中央寄せ + margin だと移動量が半分になる（中央寄せが
                // margin 込みで再計算するため）。絶対座標で置いて 1:1 で動かす。
                let left = (CIRCLE_MAP_VIEWPORT_WIDTH - display_width) / 2.0 + pan.x;
                let top = (CIRCLE_MAP_VIEWPORT_HEIGHT - display_height) / 2.0 + pan.y;
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(display_width))
                    .h(px(display_height))
                    .child(
                        img(image)
                            .w_full()
                            .h_full()
                            .object_fit(gpui_kit::ObjectFit::Fill),
                    )
            })
            .child(if let Some(overlay) = selected_overlay {
                overlay
            } else {
                div().into_any_element()
            })
            )
            .into_any_element()
    }

    /// イベント詳細（Web の Checklist 相当: テーブル + 並び替え + ページネーション）。
    fn render_event_detail(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let busy = self.busy;
        let error = self.error.clone();
        let toast = self.toast.clone();
        let last_sync = self.last_sync.clone();
        let selected_slug = self.selected_slug.clone().unwrap_or_default();
        // サンプル（デモ）モードはネットワークへ出ないので、同期のボタンも無効にする
        // （押しても何も起きないより、押せないほうが分かりやすい）。
        let demo = AppState::global(cx).demo;
        let handle = cx.entity();
        let checklist_scroll_handle = self.checklist_scroll_handle.clone();
        let events = self.events.clone();
        let sort_field = self.sort_field;
        let sort_ascending = self.sort_ascending;
        let show_other_first_appearance = self.show_other_first_appearance;
        let page = self.page;
        let border = cx.theme().border;
        let muted = cx.theme().muted;
        let muted_fg = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let primary = cx.theme().primary;
        let selected_item_id = self.selected_item_id.clone();
        let show_circle_map = self.show_circle_map;
        let circle_map = self.circle_map_images.get(&selected_slug).cloned();
        let circle_map_loading = self.circle_map_fetching.contains(&selected_slug);
        let circle_map_error = self.circle_map_error.get(&selected_slug).cloned();

        let event = events
            .iter()
            .find(|e| e.slug.as_deref() == Some(selected_slug.as_str()))
            .cloned();

        // 並び替え（Web の sortedItems 相当）。初出フィルタは並び替え前に適用する。
        let selected_event_name = event
            .as_ref()
            .map(|event| event.event_name.as_str())
            .unwrap_or_default();
        let sorted = self.sorted_display_items(selected_event_name);

        // ページネーション（Web の PAGE_SIZE = 50）
        const PAGE_SIZE: usize = 50;
        let total_pages = sorted.len().div_ceil(PAGE_SIZE).max(1);
        let start = page * PAGE_SIZE;
        let paginated: Vec<CheckedItem> = sorted[start.min(sorted.len())..]
            .iter()
            .take(PAGE_SIZE)
            .cloned()
            .collect();

        // テーブル行（Web の ChecklistItemRow 相当）
        let item_row = |item: &CheckedItem| {
            let handle = handle.clone();
            let circle = item.circle_name.clone();
            let space = item.space_number.clone();
            let title = item.product_title.clone();
            let price = item
                .price
                .map(|p| format!("¥{p}"))
                .unwrap_or_else(|| "¥-".to_string());
            let first_appearance = item.first_appearance_event_name.clone();
            let purchased = item.is_purchased != 0;
            let has_product = item.product_id.is_some();
            let sample_fetched = item.sample_fetch_attempted_at.is_some();
            // 取得中の行だけ表紙に進捗リングを出す
            let sample_fraction = self.sample_progress.get(&item.id).copied();
            let item_for_sample = item.clone();
            // render では毎回デコードせず、バックグラウンドで用意した
            // キャッシュ（thumbnail_images）だけを参照する
            let thumbnail = self.thumbnail_images.get(&item.id).cloned();
            let selected = selected_item_id.as_deref() == Some(item.id.as_str());

            div()
                .id(SharedString::from(format!("checklist-item-{}", item.id)))
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .p_3()
                .border_b_1()
                .border_color(border)
                .bg(if selected {
                    primary.opacity(0.14)
                } else {
                    gpui_kit::transparent_black()
                })
                .hover(|style| style.bg(muted.opacity(0.5)))
                .cursor_pointer()
                .on_click({
                    let handle = handle.clone();
                    let item_id = item.id.clone();
                    move |_, _, cx| {
                        handle.update(cx, |this, cx| {
                            this.select_checklist_item(&item_id, cx);
                        });
                    }
                })
                // 表紙（Web の aspect-[3/4] w-40 相当）
                .child(
                    div()
                        .relative()
                        .w(px(80.0))
                        .h(px(107.0))
                        .rounded_md()
                        .bg(muted)
                        .overflow_hidden()
                        .child(if let Some(image) = thumbnail {
                            img(image)
                                .w_full()
                                .h_full()
                                .object_fit(gpui_kit::ObjectFit::Cover)
                                .into_any_element()
                        } else {
                            div()
                                .w_full()
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(muted_fg)
                                .child(truncate_text(&circle, 8))
                                .into_any_element()
                        })
                        // 購入済/未購入バッジ
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .text_center()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(gpui_kit::white())
                                .py_0p5()
                                .bg(if purchased {
                                    gpui_kit::rgb(0x10b981).opacity(0.8)
                                } else {
                                    gpui_kit::rgb(0xec4899).opacity(0.8)
                                })
                                .child(if purchased { "購入済" } else { "未購入" }),
                        )
                        // サンプルバッジ
                        .child(if has_product {
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .text_center()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(gpui_kit::white())
                                .py_0p5()
                                .bg(gpui_kit::rgba(0x00000099))
                                .child(if sample_fetched {
                                    "サンプルを読む"
                                } else {
                                    "サンプル未取得"
                                })
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        })
                        // 試し読みの取得中: 進捗リング（Web の CircularProgress 相当）
                        .child(if let Some(fraction) = sample_fraction {
                            div()
                                .debug_selector({
                                    let id = item.id.clone();
                                    move || format!("sample-progress-{id}")
                                })
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(gpui_kit::rgba(0x00000080))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            img(progress_ring_image(fraction))
                                                .w(px(36.0))
                                                .h(px(36.0)),
                                        )
                                        .child(
                                            div()
                                                .text_color(gpui_kit::white())
                                                .text_size(px(10.0))
                                                .child(format!(
                                                    "{}%",
                                                    (fraction * 100.0).round() as u32
                                                )),
                                        ),
                                )
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        }),
                )
                // スペース名/サークル名
                .child(
                    div()
                        .w(px(140.0))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_sm().child(space))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .truncate()
                                .child(circle),
                        ),
                )
                // 書籍名/価格
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_sm().truncate().child(if title.is_empty() {
                            "-".to_string()
                        } else {
                            title
                        }))
                        .child(div().text_xs().text_color(muted_fg).child(price))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .child(first_appearance.map_or_else(
                                    || "初出: 不明".to_string(),
                                    |name| format!("初出: {name}"),
                                )),
                        ),
                )
                // 試し読み
                .child(
                    Button::new(format!("sample-{}", item.id))
                        .cursor_pointer()
                        .label(if sample_fetched {
                            "開く"
                        } else {
                            "試し読み"
                        })
                        .disabled(!has_product || busy)
                        .cursor_pointer()
                        .on_click({
                            let handle = handle.clone();
                            move |_, _window, cx| {
                                handle.update(cx, |this, cx| {
                                    this.fetch_sample(cx, item_for_sample.clone());
                                });
                            }
                        }),
                )
        };

        div()
            .size_full()
            .flex_1()
            .min_h_0()
            .relative()
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .child(
                div()
                    .mx_auto()
                    .w(px(960.0))
                    .py_8()
                    .px_4()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .flex_1()
                    .min_h_0()
                    // 一覧に戻る（最上部・左寄せ）
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .child(
                                Button::new("back-to-list").cursor_pointer()
                                    .label("← イベント一覧に戻る")
                                    .ghost()
                                    .small().cursor_pointer().on_click({
                                let handle = handle.clone();
                                move |_, _window, cx| {
                                    handle.update(cx, |this, cx| {
                                        this.back_to_list(cx);
                                    });
                                }
                                                                }),
                            ),
                    )
                    // ヘッダー: イベント名 + 日付
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(event.as_ref().map(|e| e.event_name.clone()).unwrap_or_default()),
                            )
                            .child(if let Some(event) = &event {
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .text_sm()
                                    .text_color(muted_fg)
                                    .child(
                                        Icon::new(IconName::Calendar)
                                            .size(px(14.0))
                                            .text_color(muted_fg),
                                    )
                                    .child(self.format_event_date(event))
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            // ポーリング ON/OFF
                            .child(if let Some(event) = &event {
                                let poll_slug = event.slug.clone().unwrap_or_default();
                                let poll_enabled = event.poll_sync_enabled != 0;
                                let poll_handle = handle.clone();
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Switch::new("event-poll-sync-toggle")
                                            .checked(poll_enabled)
                                            .cursor_pointer()
                                            .on_click(move |checked, _window, cx| {
                                                let slug = poll_slug.clone();
                                                poll_handle.update(cx, |this, cx| {
                                                    let state = AppState::global(cx);
                                                    let db = &state.db_pool;
                                                    let _ = db::checklist::set_poll_enabled(db, &slug, *checked);
                                                    this.reload(cx);
                                                });
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(muted_fg)
                                            .child("このイベントについて、技術書典から最新状況を同期する"),
                                    )
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            }),
                    )
                    // ツールバー
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .flex_wrap()
                            .child(if !sorted.is_empty() {
                                div()
                                    .text_sm()
                                    .text_color(muted_fg)
                                    .child(format!(
                                        "{}件{}",
                                        sorted.len(),
                                        last_sync
                                            .map(|t| format!(" ({t})"))
                                            .unwrap_or_default()
                                    ))
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .child(
                                Button::new("checklist-sync").cursor_pointer()
                                    .icon(AppIcon::RefreshCw)
                                    .label("同期")
                                    .disabled(busy || demo).cursor_pointer().on_click({
                                let handle = handle.clone();
                                move |_, _window, cx| {
                                    handle.update(cx, |this, cx| this.sync(cx));
                                }
                                                                }),
                            )
                            .child(
                                Button::new("checklist-circle-map").cursor_pointer()
                                    .label(if show_circle_map {
                                        "サークルマップを隠す"
                                    } else {
                                        "サークルマップ"
                                    })
                                    .small()
                                    .disabled(demo)
                                    .cursor_pointer()
                                    .on_click({
                                        let handle = handle.clone();
                                        move |_, _window, cx| {
                                            handle.update(cx, |this, cx| {
                                                this.toggle_circle_map(cx);
                                            });
                                        }
                                    }),
                            )
                            .child(div().text_sm().text_color(muted_fg).child("並び替え:"))
                            .children(
                                [
                                    (SortField::SortOrder, "順序"),
                                    (SortField::CircleName, "サークル名"),
                                    (SortField::SpaceNumber, "スペース"),
                                ]
                                .into_iter()
                                .map(|(field, label)| {
                                    let active = sort_field == field;
                                    let handle = handle.clone();
                                    let mut button = Button::new(format!("sort-{label}")).cursor_pointer()
                                        .label(format!(
                                            "{}{}",
                                            label,
                                            if active {
                                                if sort_ascending { " ↑" } else { " ↓" }
                                            } else {
                                                ""
                                            }
                                        ))
                                        .small();
                                    if active {
                                        button = button.secondary();
                                    }
                                    button.cursor_pointer().on_click(move |_, _window, cx| {
                                        handle.update(cx, |this, cx| {
                                            this.toggle_sort(cx, field);
                                        });
                                    })
                                }),
                            )
                            .child({
                                let toggle_handle = handle.clone();
                                let label = if selected_event_name.is_empty() {
                                    "初出以外も表示".to_string()
                                } else {
                                    format!("初出が{selected_event_name}以外でも表示")
                                };
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .debug_selector(|| {
                                                "checklist-show-other-first-appearance".into()
                                            })
                                            .child(
                                                Switch::new("checklist-show-other-first-appearance")
                                                    .checked(show_other_first_appearance)
                                                    .cursor_pointer()
                                                    .on_click(move |checked, _window, cx| {
                                                        toggle_handle.update(cx, |this, cx| {
                                                            this.set_show_other_first_appearance(
                                                                *checked, cx,
                                                            );
                                                        });
                                                    }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(muted_fg)
                                            .child(label),
                                    )
                            }),
                    )
                    // サークルマップ（会場配置図）。取得はボタンを押したときだけ。
                    .child(if show_circle_map {
                        let expand_handle = handle.clone();
                        div()
                            .id("circle-map-panel")
                            .flex()
                            .flex_col()
                            .gap_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(border)
                            .p_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child("サークルマップ"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                Button::new("circle-map-fit-toggle")
                                                    .label(if self.circle_map_fit {
                                                        "等倍表示"
                                                    } else {
                                                        "全体表示"
                                                    })
                                                    .small()
                                                    .disabled(circle_map.is_none())
                                                    .cursor_pointer()
                                                    .on_click(move |_, _window, cx| {
                                                        expand_handle.update(cx, |this, cx| {
                                                            if this.circle_map_fit {
                                                                this.circle_map_zoom = 1.0;
                                                                this.circle_map_fit = false;
                                                                this.circle_map_pan = gpui_kit::Point::new(0.0, 0.0);
                                                                this.circle_map_pan_velocity = gpui_kit::Point::new(0.0, 0.0);
                                                                this.circle_map_inertia_generation = this
                                                                    .circle_map_inertia_generation
                                                                    .wrapping_add(1);
                                                                cx.notify();
                                                            } else {
                                                                this.fit_circle_map(cx);
                                                            }
                                                        });
                                                    }),
                                            ),
                                    ),
                            )
                            .child(if let Some(error) = circle_map_error {
                                div()
                                    .text_sm()
                                    .text_color(danger)
                                    .child(format!(
                                        "{error}（もう一度「サークルマップ」を押すと再取得します）"
                                    ))
                                    .into_any_element()
                            } else if circle_map_loading {
                                div()
                                    .text_sm()
                                    .text_color(muted_fg)
                                    .child("サークルマップを作成しています…")
                                    .into_any_element()
                            } else if circle_map.is_some() {
                                self.render_circle_map_viewport(handle.clone())
                            } else {
                                div()
                                    .text_sm()
                                    .text_color(muted_fg)
                                    .child("「サークルマップ」を押すと配置図を取得します")
                                    .into_any_element()
                            })
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    // テーブル
                    .child(if sorted.is_empty() {
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .border_1()
                            .border_color(border)
                            .bg(muted.opacity(0.3))
                            .px_4()
                            .py_12()
                            .text_center()
                            .child(
                                div()
                                    .mb_4()
                                    .rounded_full()
                                    .bg(muted)
                                    .p_4()
                                    .child(
                                        Icon::new(IconName::CircleCheck)
                                            .size(px(32.0))
                                            .text_color(muted_fg),
                                    ),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("チェックリストは空です"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(muted_fg)
                                    .child(
                                        "まだアイテムが登録されていません。上の同期ボタンからチェックリストを同期できます。",
                                    ),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .flex_1()
                            .min_h_0()
                            .relative()
                            .child(
                                div()
                                    .id("checklist-list")
                                    .size_full()
                                    .track_scroll(&checklist_scroll_handle)
                                    .overflow_y_scroll()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(border)
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_3()
                                            .p_3()
                                            .bg(muted.opacity(0.5))
                                            .child(div().w(px(80.0)).text_sm().font_weight(FontWeight::MEDIUM).child("表紙"))
                                            .child(div().w(px(140.0)).text_sm().font_weight(FontWeight::MEDIUM).child("スペース名/サークル名"))
                                            .child(div().flex_1().text_sm().font_weight(FontWeight::MEDIUM).child("書籍名/価格"))
                                            .child(div().w(px(80.0))),
                                    )
                                    .children(paginated.iter().map(|item| item_row(item).into_any_element())),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .child(Scrollbar::vertical(&checklist_scroll_handle)),
                            )
                            .into_any_element()
                    })
                    // ページネーション
                    .child(if total_pages > 1 {
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_center()
                            .gap_4()
                            .py_2()
                            .child(
                                Button::new("page-prev").cursor_pointer()
                                    .label("前へ")
                                    .disabled(page == 0).cursor_pointer().on_click({
                                let handle = handle.clone();
                                move |_, _window, cx| {
                                    handle.update(cx, |this, cx| {
                                        this.page = this.page.saturating_sub(1);
                                        cx.notify();
                                    });
                                }
                                                                }),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(muted_fg)
                                    .child(format!("{} / {}", page + 1, total_pages)),
                            )
                            .child(
                                Button::new("page-next").cursor_pointer()
                                    .label("次へ")
                                    .disabled(page + 1 >= total_pages).cursor_pointer().on_click({
                                let handle = handle.clone();
                                move |_, _window, cx| {
                                    handle.update(cx, |this, cx| {
                                        this.page = (this.page + 1).min(total_pages - 1);
                                        cx.notify();
                                    });
                                }
                                                                }),
                            )
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    // ステータス（トースト・エラー）
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(if let Some(toast) = toast {
                                div()
                                    .text_sm()
                                    .text_color(muted_fg)
                                    .child(toast)
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .child(if let Some(message) = error {
                                div()
                                    .text_sm()
                                    .text_color(danger)
                                    .child(message)
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            }),
                    ),
                ),
            )
    }
}

/// チェックリストのサムネイル（base64 データ URI）を RenderImage に変換する。
/// 画像バイト列をデコードし、サムネイル表示用に縮小した RenderImage を返す。
/// 表紙は最大 144x192 の表示なので、幅 256px に縮小してメモリ使用量を抑える。
fn decode_thumbnail(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let rgba = shrink_rgba(bytes)?;
    let (width, height) = rgba.dimensions();
    let mut flat = rgba.into_raw();
    // BGRA に変換（gpui の RenderImage は BGRA 前提）
    for pixel in flat.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(
        image::RgbaImage::from_raw(width, height, flat)?,
    )])))
}

/// スペース番号をブロック→番号の順で比較する（`あ10` は `あ2` より後）。
fn compare_space_numbers(a: &str, b: &str) -> std::cmp::Ordering {
    match (parse_space(a), parse_space(b)) {
        (Some((a_block, a_number)), Some((b_block, b_number))) => {
            a_block.cmp(&b_block).then(a_number.cmp(&b_number))
        }
        _ => a.cmp(b),
    }
}

/// スペース名（例: `き05`, `あ-01`, `協01`）を（列, 番号）に分解する。
fn parse_space(space: &str) -> Option<(String, u32)> {
    let text = space.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("TBD") {
        return None;
    }
    let digit_index = text.char_indices().find(|(_, c)| c.is_ascii_digit())?.0;
    let letter = text[..digit_index]
        .trim()
        .trim_end_matches(['-', 'ー', '−', ' '])
        .to_string();
    if letter.is_empty() {
        return None;
    }
    let digits: String = text[digit_index..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    Some((letter, digits.parse().ok()?))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 自作サークルマップの SVG。
///
/// 技術書典21 は会場の配置（列・通路・設備）を残した自前レイアウト
/// （`assets/tbf/tbf21-layout.svg`）をそのまま使う。ブロック名は 1 回だけ、
/// 配置番号はセル中央に置いてある。それ以外のイベントはチェックリストの
/// スペース番号から簡易マップを作る。
#[cfg(test)]
fn build_circle_map_svg(event_slug: &str, event_name: &str, spaces: &[(String, u32)]) -> String {
    build_circle_map_svg_with_highlights(
        event_slug,
        event_name,
        spaces,
        &HashSet::new(),
        &HashSet::new(),
        None,
    )
}

/// 地図のセル色: チェックした本があるサークル（オレンジ）。
const MAP_CHECKED_FILL: &str = "#fdba74";
/// 地図のセル色: フォロー中のサークル（青）。
const MAP_FAVORITE_FILL: &str = "#93c5fd";
/// セル色の不透明度（白背景に馴染ませる）。
const MAP_FILL_OPACITY: &str = "0.4";
/// 選択中のセル色（赤）。通常の配色より少し濃くして目立たせる。
const MAP_SELECTED_FILL: &str = "#ef4444";
const MAP_SELECTED_OPACITY: &str = "0.65";

fn build_circle_map_svg_with_highlights(
    event_slug: &str,
    event_name: &str,
    spaces: &[(String, u32)],
    checked_spaces: &HashSet<(String, u32)>,
    favorite_spaces: &HashSet<(String, u32)>,
    selected_space: Option<&(String, u32)>,
) -> String {
    if event_slug == "tbf21" {
        return apply_tbf21_highlights(
            include_str!("../../assets/tbf/tbf21-layout.svg"),
            checked_spaces,
            favorite_spaces,
            selected_space,
        );
    }

    use std::collections::{BTreeMap, BTreeSet};

    // イベントごとの会場データが無いときは、チェックリストの
    // スペース番号から 10 個ずつのブロックを組み立てる。
    let mut by_letter: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for (letter, number) in spaces {
        by_letter.entry(letter.clone()).or_default().insert(*number);
    }
    let left: Vec<(String, Vec<(u32, u32)>)> = by_letter
        .into_iter()
        .map(|(letter, numbers)| {
            let mut groups: Vec<(u32, u32)> = Vec::new();
            for number in numbers {
                let start = ((number.saturating_sub(1)) / 10) * 10 + 1;
                let end = start + 9;
                if groups.last().map(|(_, e)| *e) != Some(end) {
                    groups.push((start, end));
                }
            }
            (letter, groups)
        })
        .collect();
    let right: Vec<(String, Vec<(u32, u32)>)> = Vec::new();

    let is_highlighted = |letter: &str, start: u32, end: u32| {
        spaces.iter().any(|(space_letter, number)| {
            space_letter == letter && *number >= start && *number <= end
        })
    };

    let mut body = String::new();
    let title = xml_escape(event_name);
    body.push_str(&format!(
        "<text x=\"40\" y=\"36\" font-family=\"Noto Sans JP, sans-serif\" font-size=\"22\" font-weight=\"700\" fill=\"#111827\">{title} サークルマップ</text>\
         <text x=\"40\" y=\"58\" font-family=\"Noto Sans JP, sans-serif\" font-size=\"11\" fill=\"#4b5563\">ブロックは 10 個ずつまとめて表示。色付きはチェックリストに本があるブロック。</text>"
    ));

    let card_width = 108.0;
    let card_height = 34.0;
    let gap = 6.0;
    let row_height = 44.0;
    let left_x = 40.0;
    let right_x = 560.0;
    let rows_y = 86.0;

    let mut draw_section = |rows: &[(String, Vec<(u32, u32)>)], x: f32, label: &str| {
        if rows.is_empty() {
            return;
        }
        body.push_str(&format!(
            "<text x=\"{x}\" y=\"{}\" font-family=\"Noto Sans JP, sans-serif\" font-size=\"13\" font-weight=\"700\" fill=\"#374151\">{label}</text>",
            rows_y - 10.0
        ));
        for (row_index, (letter, groups)) in rows.iter().enumerate() {
            let y = rows_y + row_index as f32 * row_height;
            for (group_index, (start, end)) in groups.iter().enumerate() {
                let card_x = x + group_index as f32 * (card_width + gap);
                let highlighted = is_highlighted(letter, *start, *end);
                let (fill, stroke) = if highlighted {
                    ("#bbf7d0", "#15803d")
                } else {
                    ("#f8fafc", "#334155")
                };
                body.push_str(&format!(
                    "<rect x=\"{card_x}\" y=\"{y}\" width=\"{card_width}\" height=\"{card_height}\" rx=\"6\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.2\"/>\
                     <text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-family=\"Noto Sans JP, sans-serif\"><tspan font-size=\"18\" font-weight=\"700\" fill=\"#111827\">{}</tspan><tspan font-size=\"8\" fill=\"#4b5563\">({}-{})</tspan></text>",
                    card_x + card_width / 2.0,
                    y + 22.0,
                    xml_escape(letter),
                    start,
                    end
                ));
            }
        }
    };

    draw_section(&left, left_x, "左側（西）");
    draw_section(&right, right_x, "右側（東）");

    // 会場の設備（簡略図）。
    for (x, y, w, h, label) in [
        (right_x, 430.0, 190.0, 70.0, "企画展示"),
        (right_x + 210.0, 430.0, 110.0, 70.0, "運営"),
        (right_x + 340.0, 430.0, 140.0, 70.0, "荷捌所"),
    ] {
        body.push_str(&format!(
            "<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" rx=\"4\" fill=\"#e5e7eb\" stroke=\"#6b7280\"/>\
             <text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-family=\"Noto Sans JP, sans-serif\" font-size=\"14\" fill=\"#374151\">{label}</text>",
            x + w / 2.0,
            y + h / 2.0 + 5.0
        ));
    }
    body.push_str(
        "<text x=\"40\" y=\"670\" font-family=\"Noto Sans JP, sans-serif\" font-size=\"11\" fill=\"#6b7280\">左側（西） / 右側（東）は会場の配置を簡略化したものです。数字はスペース番号の範囲。</text>\
         <text x=\"20\" y=\"620\" font-family=\"Noto Sans JP, sans-serif\" font-size=\"12\" fill=\"#9ca3af\">出口</text>\
         <text x=\"1160\" y=\"74\" font-family=\"Noto Sans JP, sans-serif\" font-size=\"12\" fill=\"#9ca3af\">入口</text>\
         <text x=\"500\" y=\"300\" transform=\"rotate(90 500 300)\" font-family=\"Noto Sans JP, sans-serif\" font-size=\"12\" fill=\"#9ca3af\">通路</text>"
    );

    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1200\" height=\"700\" viewBox=\"0 0 1200 700\">\
         <rect width=\"1200\" height=\"700\" fill=\"#ffffff\"/>{body}</svg>"
    )
}

/// tbf21 の固定 SVG のセルを、チェック済み（橙）・フォロー中（青）で塗る。
///
/// セルの id は Figma 由来の `\u304201`（= あ01）形式。スペース番号から
/// 同じ id を組み立てて `fill="#fff"` だけを差し替える。青が優先。
fn apply_tbf21_highlights(
    svg: &str,
    checked_spaces: &HashSet<(String, u32)>,
    favorite_spaces: &HashSet<(String, u32)>,
    selected_space: Option<&(String, u32)>,
) -> String {
    let mut out = svg.to_string();
    let mut paint = |letter: &str, number: u32, color: &str, opacity: &str| {
        let Some(ch) = letter.chars().next() else {
            return;
        };
        let id = format!("\\u{:04X}{:02}", ch as u32, number);
        let needle = format!("id=\"{id}\" fill=\"#fff\"");
        if out.contains(&needle) {
            let replacement =
                format!("id=\"{id}\" fill=\"{color}\" fill-opacity=\"{opacity}\"");
            out = out.replace(&needle, &replacement);
        }
    };

    // 選択中のセルを先に塗る。後続の橙/青は白セルだけを対象にするため、
    // 選択色が上書きされない。
    if let Some((letter, number)) = selected_space {
        paint(letter, *number, MAP_SELECTED_FILL, MAP_SELECTED_OPACITY);
    }
    for (letter, number) in favorite_spaces {
        paint(letter, *number, MAP_FAVORITE_FILL, MAP_FILL_OPACITY);
    }
    for (letter, number) in checked_spaces {
        paint(letter, *number, MAP_CHECKED_FILL, MAP_FILL_OPACITY);
    }
    out
}

/// 画像バイト列を幅 256px 上限まで縮小した RGBA 画像にする。
fn shrink_rgba(bytes: &[u8]) -> Option<image::RgbaImage> {
    let format = image::guess_format(bytes).ok()?;
    let img = image::load_from_memory_with_format(bytes, format).ok()?;
    let (w, h) = img.dimensions();
    let img = if w > 256 {
        let nh = ((h as f32) * (256.0 / w as f32)).round().max(1.0) as u32;
        img.resize(256, nh, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    Some(img.to_rgba8())
}

/// ダウンロードした表紙画像を縮小 JPEG の base64 に変換する（DB 保存用）。
/// 元画像（最大 2MB 級）をそのまま保存せず、表示に十分なサイズで永続化する。
fn thumbnail_jpeg_base64(bytes: &[u8]) -> Option<String> {
    let rgba = shrink_rgba(bytes)?;
    let (width, height) = rgba.dimensions();
    // JPEG は RGB のみ（アルファなし）なので変換してからエンコードする
    let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
    let mut jpeg = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80);
    encoder
        .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .ok()?;
    drop(encoder);
    Some(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &jpeg,
    ))
}

/// 固定 SVG の色付きセルから、クリック判定用の矩形（画像ピクセル座標）を作る。
fn tbf21_cell_rects(
    svg: &str,
    spaces: &HashSet<(String, u32)>,
    scale: f32,
) -> HashMap<(String, u32), CellRect> {
    let mut rects = HashMap::new();
    for (letter, number) in spaces {
        let Some(ch) = letter.chars().next() else {
            continue;
        };
        let id = format!("\\u{:04X}{:02}", ch as u32, number);
        let Some(id_pos) = svg.find(&format!("id=\"{id}\"")) else {
            continue;
        };
        let path_start = svg[..id_pos].rfind("<path").unwrap_or(0);
        let path_end = svg[id_pos..]
            .find("</path>")
            .map_or(svg.len(), |end| id_pos + end);
        let path = &svg[path_start..path_end];
        let Some(d_start) = path.find("d=\"M") else {
            continue;
        };
        let d_start = d_start + 3;
        let Some(d_end) = path[d_start..].find('"') else {
            continue;
        };
        let d = &path[d_start..d_start + d_end];
        let Some(rest) = d.strip_prefix('M') else {
            continue;
        };
        let Some(pos_end) = rest.find(['h', 'v']) else {
            continue;
        };
        let mut pos = rest[..pos_end].split_whitespace();
        let (Some(x), Some(y)) = (
            pos.next().and_then(|v| v.parse::<f32>().ok()),
            pos.next().and_then(|v| v.parse::<f32>().ok()),
        ) else {
            continue;
        };

        let (angle, center_x, center_y) = parse_cell_rotation(path);
        let (sin, cos) = angle.to_radians().sin_cos();
        let rotate = |px: f32, py: f32| {
            (
                center_x + (px - center_x) * cos - (py - center_y) * sin,
                center_y + (px - center_x) * sin + (py - center_y) * cos,
            )
        };
        let corners = [
            rotate(x, y),
            rotate(x + 17.0, y),
            rotate(x + 17.0, y + 14.0),
            rotate(x, y + 14.0),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (px, py) in corners {
            x0 = x0.min(px);
            y0 = y0.min(py);
            x1 = x1.max(px);
            y1 = y1.max(py);
        }
        rects.insert(
            (letter.clone(), *number),
            CellRect {
                x0: x0 * scale,
                y0: y0 * scale,
                x1: x1 * scale,
                y1: y1 * scale,
            },
        );
    }
    rects
}

/// セルの `transform="rotate(angle cx cy)"` を読む。無指定は 0 度。
fn parse_cell_rotation(path: &str) -> (f32, f32, f32) {
    let Some(start) = path.find("rotate(") else {
        return (0.0, 0.0, 0.0);
    };
    let rest = &path[start + "rotate(".len()..];
    let Some(end) = rest.find(')') else {
        return (0.0, 0.0, 0.0);
    };
    let mut values = rest[..end].split_whitespace();
    let angle = values.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let cx = values.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let cy = values.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    (angle, cx, cy)
}

/// 初出イベント名が選択中イベントと一致する項目だけ返す。
fn filter_first_appearance(items: &[CheckedItem], event_name: &str) -> Vec<CheckedItem> {
    let wanted = normalize_event_name(event_name);
    if wanted.is_empty() {
        return items.to_vec();
    }
    items
        .iter()
        .filter(|item| {
            item.first_appearance_event_name
                .as_deref()
                .is_some_and(|name| normalize_event_name(name) == wanted)
        })
        .cloned()
        .collect()
}

/// イベント名の表記ゆれ（`技術書典 21` / `技術書典21`）を吸収する。
fn normalize_event_name(name: &str) -> String {
    name.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// Web の truncateText 相当。
fn truncate_text(text: &str, max_length: usize) -> String {
    if text.chars().count() <= max_length {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(max_length).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::AppContext as _;
    use gpui_kit::TestAppContext;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ligrol_core::db::checklist as checklist_db;

    use super::*;

    fn seed_event(cx: &mut TestAppContext, slug: &str, name: &str) {
        cx.update(|cx| {
            let state = AppState::global(cx);
            let db = &state.db_pool;
            checklist_db::upsert_event(
                db,
                &checklist_db::TbfEvent {
                    id: slug.into(),
                    site_id: "techbookfest".into(),
                    slug: Some(slug.into()),
                    tbf_event_id: Some(format!("Event:{slug}")),
                    event_name: name.into(),
                    event_date: None,
                    event_start_date: None,
                    event_end_date: None,
                    event_format: "offline".into(),
                    is_cancelled: 0,
                    display_order: 0,
                    is_featured: 0,
                    poll_sync_enabled: 0,
                    created_at: "2026-08-21 00:00:00".into(),
                    updated_at: "2026-08-21 00:00:00".into(),
                },
            )
            .unwrap();
        });
    }

    fn seed_item(cx: &mut TestAppContext, id: &str, slug: &str, circle: &str) {
        cx.update(|cx| {
            let state = AppState::global(cx);
            let db = &state.db_pool;
            checklist_db::upsert_item(
                db,
                &checklist_db::CheckedItem {
                    id: id.into(),
                    event_id: slug.into(),
                    circle_name: circle.into(),
                    space_number: "あ-01".into(),
                    memo: String::new(),
                    is_checked: 0,
                    sort_order: 0,
                    tbf_circle_id: None,
                    product_id: Some("p1".into()),
                    product_title: "本".into(),
                    thumbnail_url: None,
                    thumbnail_data: None,
                    price: Some(1000),
                    is_purchased: 0,
                    first_appearance_event_name: Some(format!(
                        "技術書典{}",
                        slug.trim_start_matches("tbf")
                    )),
                    sample_fetch_attempted_at: None,
                    created_at: "2026-08-21 00:00:00".into(),
                },
            )
            .unwrap();
        });
    }

    /// canonical イベント（技術書典21 を含む）は、まだ同期していなくても
    /// イベント一覧に出る。DB に前回のイベント行しか無い状態で開いたときに
    /// 「技術書典20まで」で止まらないようにする。
    #[gpui_kit::test]
    async fn canonical_events_appear_without_a_sync(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        let view = cx.new(ChecklistView::new);
        let events = view.read_with(cx, |v, _| v.events.clone());
        let tbf21 = events
            .iter()
            .find(|event| event.slug.as_deref() == Some("tbf21"))
            .expect("技術書典21 がイベント一覧に無い");
        assert_eq!(tbf21.event_name, "技術書典21");
        assert_eq!(tbf21.event_start_date.as_deref(), Some("2026-11-21"));
        assert_ne!(tbf21.is_featured, 0, "技術書典21 が注目イベントでない");
        assert_eq!(
            events.iter().filter(|event| event.is_featured != 0).count(),
            1,
            "注目イベントが複数ある"
        );
    }

    #[gpui_kit::test]
    async fn loads_events_and_items_for_selection(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        seed_event(cx, "tbf19", "技術書典19");
        seed_item(cx, "item-1", "tbf20", "サークルA");
        seed_item(cx, "item-2", "tbf19", "サークルB");
        let view = cx.new(ChecklistView::new);
        // canonical イベント（tbf21 など）も種まきされるため、seed した 2 件だけではない
        assert!(view.read_with(cx, |v, _| v.events.len()) >= 2);
        assert!(
            view.read_with(cx, |v, _| {
                v.events
                    .iter()
                    .any(|event| event.slug.as_deref() == Some("tbf20"))
            }),
            "seed した tbf20 が消えている"
        );
        // Web と同じ: 最初はイベント一覧表示（選択なし）
        assert!(view.read_with(cx, |v, _| v.selected_slug.is_none()));
        assert!(view.read_with(cx, |v, _| v.items.is_empty()));
        // イベントを選択 → 詳細（アイテム）が読み込まれる
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, "tbf20")));
        assert_eq!(view.read_with(cx, |v, _| v.items.len()), 1);
        assert_eq!(
            view.read_with(cx, |v, _| v.items[0].circle_name.clone()),
            "サークルA".to_string()
        );
        // 一覧に戻る（Web の「← イベント一覧に戻る」相当）
        cx.update(|cx| view.update(cx, |this, cx| this.back_to_list(cx)));
        assert!(view.read_with(cx, |v, _| v.selected_slug.is_none()));
    }

    #[gpui_kit::test]
    async fn toggle_sort_changes_field_and_direction(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        let view = cx.new(ChecklistView::new);
        assert_eq!(
            view.read_with(cx, |v, _| v.sort_field),
            SortField::SpaceNumber
        );
        assert!(view.read_with(cx, |v, _| v.sort_ascending));
        // 別フィールド → 昇順で切り替え
        cx.update(|cx| view.update(cx, |this, cx| this.toggle_sort(cx, SortField::CircleName)));
        assert_eq!(
            view.read_with(cx, |v, _| v.sort_field),
            SortField::CircleName
        );
        assert!(view.read_with(cx, |v, _| v.sort_ascending));
        // 同じフィールド → 降順に反転
        cx.update(|cx| view.update(cx, |this, cx| this.toggle_sort(cx, SortField::CircleName)));
        assert!(!view.read_with(cx, |v, _| v.sort_ascending));
    }

    /// 既定の並びはスペース番号で、ブロック「あ」から始まる。
    #[test]
    fn default_sort_starts_from_block_a_in_space_order() {
        let mut items = vec![
            test_item("item-i", 0),
            test_item("item-a10", 1),
            test_item("item-a2", 2),
        ];
        items[0].space_number = "い01".into();
        items[1].space_number = "あ10".into();
        items[2].space_number = "あ2".into();
        let view = view_with_items(items);
        assert_eq!(view.sort_field, SortField::SpaceNumber);
        let sorted = view.sorted_display_items("");
        let ids: Vec<&str> = sorted.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(
            ids,
            ["item-a2", "item-a10", "item-i"],
            "ブロックあの番号順→い の順になっていない"
        );
    }

    /// 「初出以外も表示」が ON なら、選択中イベントが初出でない項目も一覧に入る。
    #[test]
    fn show_other_first_appearance_toggles_the_event_filter() {
        let mut items = vec![test_item("item-21", 0), test_item("item-20", 1)];
        items[0].first_appearance_event_name = Some("技術書典21".into());
        items[1].first_appearance_event_name = Some("技術書典20".into());
        let mut view = view_with_items(items);
        assert!(!view.show_other_first_appearance, "既定が ON になっている");
        assert_eq!(
            view.sorted_display_items("技術書典21").len(),
            1,
            "OFF なのに初出以外の項目が出ている"
        );
        view.show_other_first_appearance = true;
        assert_eq!(
            view.sorted_display_items("技術書典21").len(),
            2,
            "ON なのに初出以外の項目が出ていない"
        );
    }

    /// 並び替えの隣の「初出以外も表示」スイッチが描かれ、クリックで切り替わる。
    #[gpui_kit::test]
    async fn show_other_first_appearance_switch_toggles_on_click(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf21", "技術書典21");
        cx.update(|cx| {
            let state = AppState::global(cx);
            let db = &state.db_pool;
            let mut same_event = test_item("item-21", 0);
            same_event.event_id = "tbf21".into();
            same_event.first_appearance_event_name = Some("技術書典21".into());
            let mut other_event = test_item("item-20", 1);
            other_event.event_id = "tbf21".into();
            other_event.first_appearance_event_name = Some("技術書典20".into());
            checklist_db::upsert_item(db, &same_event).unwrap();
            checklist_db::upsert_item(db, &other_event).unwrap();
        });
        let view = cx.new(ChecklistView::new);
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, "tbf21")));
        let window = cx.open_window(
            gpui_kit::Size {
                width: gpui_kit::px(1200.0),
                height: gpui_kit::px(1000.0),
            },
            |window, cx| gpui_kit::component::Root::new(view.clone(), window, cx),
        );
        let visual = gpui_kit::VisualTestContext::from_window(*window, cx).into_mut();
        for _ in 0..3 {
            visual.update(|window, cx| {
                let arena_clear = window.draw(cx);
                arena_clear.clear(cx);
            });
        }
        let toggle = visual
            .debug_bounds("checklist-show-other-first-appearance")
            .expect("「初出以外も表示」スイッチが描かれていない");
        assert!(
            !view.read_with(cx, |v, _| v.show_other_first_appearance),
            "既定が ON になっている"
        );
        assert_eq!(
            view.read_with(cx, |v, _| v.sorted_display_items("技術書典21").len()),
            1,
            "既定で初出以外の項目が出ている"
        );
        visual.simulate_click(toggle.center(), gpui_kit::Modifiers::default());
        assert!(
            view.read_with(cx, |v, _| v.show_other_first_appearance),
            "スイッチのクリックが状態に反映されない"
        );
        assert_eq!(
            view.read_with(cx, |v, _| v.sorted_display_items("技術書典21").len()),
            2,
            "ON にしても初出以外の項目が出ない"
        );
        let toggle = visual
            .debug_bounds("checklist-show-other-first-appearance")
            .expect("「初出以外も表示」スイッチが描かれていない");
        visual.simulate_click(toggle.center(), gpui_kit::Modifiers::default());
        assert!(
            !view.read_with(cx, |v, _| v.show_other_first_appearance),
            "もう一度クリックしても OFF に戻らない"
        );
    }

    #[gpui_kit::test]
    async fn toggle_updates_checked_state(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        seed_item(cx, "item-1", "tbf20", "サークルA");
        let view = cx.new(ChecklistView::new);
        cx.update(|cx| view.update(cx, |this, cx| this.toggle_item(cx, "item-1")));
        let checked = cx.read(|cx| {
            let state = AppState::global(cx);
            let db = &state.db_pool;
            checklist_db::get_item(db, "item-1")
                .unwrap()
                .unwrap()
                .is_checked
        });
        assert_eq!(checked, 1);
    }

    #[test]
    fn decode_thumbnail_scales_down_large_images() {
        // 1024x1024 の画像を作ってデコード → 256px 幅に縮小される
        let img = image::RgbaImage::from_pixel(1024, 1024, image::Rgba([200, 100, 50, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let image = decode_thumbnail(bytes.get_ref()).expect("must decode");
        let data = image.as_bytes(0).expect("pixels");
        // 256x256x4 = 262144
        assert_eq!(data.len(), 256 * 256 * 4, "must be scaled to 256px width");
    }

    #[test]
    fn thumbnail_jpeg_base64_produces_small_jpeg() {
        // 1024x1024 の PNG → 縮小 JPEG base64（幅 256px に縮小）
        let img = image::RgbaImage::from_pixel(1024, 1024, image::Rgba([200, 100, 50, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let b64 = thumbnail_jpeg_base64(bytes.get_ref()).expect("must convert");
        let decoded =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &b64).unwrap();
        assert!(
            matches!(image::guess_format(&decoded), Ok(image::ImageFormat::Jpeg)),
            "persisted thumbnail must be JPEG"
        );
        let loaded = image::load_from_memory(&decoded).unwrap();
        assert_eq!(loaded.width(), 256, "must be scaled to 256px width");
    }

    #[test]
    fn base64_thumbnail_decodes_to_image() {
        // 1x1 の透明 PNG（base64）
        let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, png_b64)
            .expect("base64 decodes");
        let image = decode_thumbnail(&decoded);
        assert!(
            image.is_some(),
            "valid base64 PNG must decode to a RenderImage"
        );
        let image = image.expect("decoded");
        assert!(
            image.as_bytes(0).is_some(),
            "render image must contain pixel data"
        );
    }

    /// テスト用の項目（id と並び順だけ指定）。
    fn test_item(id: &str, sort_order: i64) -> CheckedItem {
        CheckedItem {
            id: id.into(),
            event_id: "tbf20".into(),
            circle_name: "サークルA".into(),
            space_number: "あ-01".into(),
            memo: String::new(),
            is_checked: 0,
            sort_order,
            tbf_circle_id: None,
            product_id: Some("p1".into()),
            product_title: "本".into(),
            thumbnail_url: None,
            thumbnail_data: Some("eA==".into()),
            price: Some(1000),
            is_purchased: 0,
            first_appearance_event_name: Some("技術書典20".into()),
            sample_fetch_attempted_at: None,
            created_at: "2026-08-21 00:00:00".into(),
        }
    }

    /// テスト用のビュー（非同期タスクを伴わず決定的に検証する）。
    fn view_with_items(items: Vec<CheckedItem>) -> ChecklistView {
        ChecklistView {
            events: Vec::new(),
            selected_slug: Some("tbf20".into()),
            items,
            favorite_circles: HashSet::new(),
            sample_progress: HashMap::new(),
            busy: false,
            error: None,
            toast: None,
            last_sync: None,
            sort_field: SortField::SpaceNumber,
            sort_ascending: true,
            show_other_first_appearance: false,
            page: 0,
            visible_older_count: 0,
            thumbnail_images: HashMap::new(),
            thumbnail_order: VecDeque::new(),
            evicted_thumb_images: Vec::new(),
            thumbnail_fetching: HashSet::new(),
            fetch_failed: HashSet::new(),
            auto_sync_attempted: HashSet::new(),
            circle_map_images: HashMap::new(),
            circle_map_fetching: HashSet::new(),
            circle_map_error: HashMap::new(),
            show_circle_map: false,
            circle_map_zoom: 1.0,
            circle_map_fit: false,
            circle_map_pan: gpui_kit::Point::new(0.0, 0.0),
            circle_map_drag_start: None,
            circle_map_pan_velocity: gpui_kit::Point::new(0.0, 0.0),
            circle_map_inertia_generation: 0,
            circle_map_last_click: None,
            circle_map_click_moved: false,
            circle_map_bounds: std::rc::Rc::new(std::cell::Cell::new(None)),
            circle_map_viewport_bounds: std::rc::Rc::new(std::cell::Cell::new(None)),
            circle_map_wheel_anchor: None,
            circle_map_zoom_target: None,
            circle_map_pan_target: None,
            circle_map_zoom_animation_generation: 0,
            circle_map_cells: HashMap::new(),
            selected_item_id: None,
            checklist_scroll_handle: ScrollHandle::new(),
        }
    }

    /// テスト用の 1x1 RenderImage（デコード不要で軽い）。
    fn dummy_render_image() -> Arc<RenderImage> {
        Arc::new(RenderImage::new([image::Frame::new(
            image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255])),
        )]))
    }

    #[test]
    fn decode_pending_limits_to_current_page() {
        // 60 件の項目を直接構築（非同期タスクなしで決定的に検証する）
        let items: Vec<CheckedItem> = (0..60)
            .map(|i| test_item(&format!("item-{i:02}"), i as i64))
            .collect();
        let view = view_with_items(items);
        // ページ 0 では先頭 50 件が対象（1 度の実行は 20 件まで）
        let pending = view.decode_pending();
        assert_eq!(pending.len(), 20, "page 0: at most 20 per batch");
        // ページ 1 では残り 10 件だけが対象（全 60 件を一括デコードしない）
        let mut page1 = view;
        page1.page = 1;
        let pending = page1.decode_pending();
        assert_eq!(pending.len(), 10, "page 1: only the last 10 items");
        // fetch_failed に記録された項目は再試行されない
        page1.fetch_failed.insert("item-59".into());
        let pending = page1.decode_pending();
        assert_eq!(pending.len(), 9, "failed item must be skipped");
    }

    #[test]
    fn cache_thumb_keeps_cache_within_limit() {
        let mut view = view_with_items(Vec::new());
        // 上限を超えてキャッシュしても、保持数と挿入順は上限で止まる
        for i in 0..(MAX_THUMB_CACHE + 10) {
            view.cache_thumb(format!("item-{i:02}"), dummy_render_image());
        }
        assert_eq!(
            view.thumbnail_images.len(),
            MAX_THUMB_CACHE,
            "保持数が上限を超えている"
        );
        assert_eq!(
            view.thumbnail_order.len(),
            MAX_THUMB_CACHE,
            "挿入順の管理数が上限を超えている"
        );
        // 最古の 10 件は追い出され、GPU テクスチャ解放待ちに積まれる
        for i in 0..10 {
            assert!(
                !view.thumbnail_images.contains_key(&format!("item-{i:02}")),
                "最古のサムネイルが残っている"
            );
        }
        assert!(
            view.thumbnail_images
                .contains_key(&format!("item-{:02}", MAX_THUMB_CACHE + 9)),
            "最新のサムネイルが追い出されている"
        );
        assert_eq!(
            view.evicted_thumb_images.len(),
            10,
            "追い出した画像が解放待ちに積まれていない"
        );
    }

    #[test]
    fn cache_thumb_keeps_thumbnails_of_visible_page() {
        // 表示中のページ（先頭 50 件）のサムネイルは、他ページのサムネイルを
        // 上限以上に足しても追い出されない（表示中に表紙が消えない）
        let mut view = view_with_items(
            (0..50)
                .map(|i| test_item(&format!("item-{i:02}"), i as i64))
                .collect(),
        );
        for i in 0..50 {
            view.cache_thumb(format!("item-{i:02}"), dummy_render_image());
        }
        for i in 0..MAX_THUMB_CACHE {
            view.cache_thumb(format!("other-{i:02}"), dummy_render_image());
        }
        assert!(
            view.thumbnail_images.len() <= MAX_THUMB_CACHE,
            "保持数が上限を超えている"
        );
        for i in 0..50 {
            assert!(
                view.thumbnail_images.contains_key(&format!("item-{i:02}")),
                "表示中のサムネイルが追い出された"
            );
        }
    }

    #[gpui_kit::test]
    async fn fetched_thumbnails_are_decoded_only_for_current_page(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        // 1x1 の透明 PNG（base64）。取得済みサムネイルとして渡す
        let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        let view = cx.new(ChecklistView::new);
        cx.update(|cx| {
            view.update(cx, |this, cx| {
                // 60 件（ページ 0 = 先頭 50 件、ページ 1 = 残り 10 件）
                this.items = (0..60)
                    .map(|i| test_item(&format!("item-{i:02}"), i as i64))
                    .collect();
                // ページ 0 とページ 1 の項目の取得結果をまとめて反映する
                this.apply_fetched_thumbnails(
                    cx,
                    vec![
                        ("item-01".into(), png_b64.into()),
                        ("item-59".into(), png_b64.into()),
                    ],
                    vec!["item-02".into()],
                );
            });
        });
        cx.run_until_parked();
        view.read_with(cx, |this, _| {
            // 取得結果は items に反映され、取得中フラグも解除される
            let item = this.items.iter().find(|item| item.id == "item-01").unwrap();
            assert!(
                item.thumbnail_data.is_some(),
                "取得データが反映されていない"
            );
            assert!(
                !this.thumbnail_fetching.contains("item-01"),
                "取得中フラグが残っている"
            );
            // 失敗した項目は再取得しない
            assert!(
                this.fetch_failed.contains("item-02"),
                "失敗が記録されていない"
            );
            // 描画中のページ（ページ 0）だけデコードされる
            assert!(
                this.thumbnail_images.contains_key("item-01"),
                "表示中のサムネイルがデコードされていない"
            );
            assert!(
                !this.thumbnail_images.contains_key("item-59"),
                "非表示ページのサムネイルまでデコードしている"
            );
        });
    }

    #[gpui_kit::test]
    async fn sample_store_writes_base64_rows(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        seed_item(cx, "item-1", "tbf20", "サークルA");
        cx.update(|cx| {
            let state = AppState::global(cx);
            let db = &state.db_pool;
            db::samples::insert_sample_page(
                db,
                &db::samples::SamplePageRow {
                    id: "sp-1".into(),
                    checklist_item_id: "item-1".into(),
                    product_id: Some("p1".into()),
                    page_number: 1,
                    image_url: Some("https://example.com/1.jpg".into()),
                    image_data: Some("aGVsbG8=".into()),
                    mime_type: "image/jpeg".into(),
                    width: Some(100),
                    height: Some(140),
                    file_size: Some(5),
                    fetched_at: "2026-08-21T00:00:00Z".into(),
                },
            )
            .unwrap();
        });
        let rows = cx.read(|cx| {
            let state = AppState::global(cx);
            let db = &state.db_pool;
            db::samples::list_for_item(db, "item-1").unwrap()
        });
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].image_data.as_deref(), Some("aGVsbG8="));
    }

    /// 試し読みの進捗（純関数）: 何ページ目をどれだけ落としたか。
    #[test]
    fn sample_progress_fraction_covers_all_pages() {
        // 6 ページ中 1 ページ目の半分 = 1/12
        assert!((sample_progress_fraction(0, 6, 50, 100) - 1.0 / 12.0).abs() < 1e-5);
        // 4 ページ目を読み切った = 4/6
        assert!((sample_progress_fraction(3, 6, 10, 10) - 4.0 / 6.0).abs() < 1e-5);
        // 総数不明（Content-Length なし）は「そのページの先頭」＝進めない
        assert_eq!(sample_progress_fraction(0, 6, 1024, 0), 0.0);
        // ページ 0 件でも割り算で壊れない
        assert_eq!(sample_progress_fraction(0, 0, 0, 0), 0.0);
        // 端は 0..1 に収める
        assert_eq!(sample_progress_fraction(5, 6, 999, 10), 1.0);
    }

    /// サークルマップはイベントを開くとデフォルトで表示し、トグルで隠せる。
    /// 取得（ネットワーク）は表示が必要になったときだけ走らせる。
    #[gpui_kit::test]
    async fn circle_map_toggle_renders_the_cached_map(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        seed_item(cx, "item-1", "tbf20", "サークルA");
        let view = cx.new(ChecklistView::new);
        let window = cx.open_window(
            gpui_kit::Size {
                width: gpui_kit::px(1200.0),
                height: gpui_kit::px(800.0),
            },
            |window, cx| gpui_kit::component::Root::new(view.clone(), window, cx),
        );
        let visual = gpui_kit::VisualTestContext::from_window(*window, cx).into_mut();
        let draw = |visual: &mut gpui_kit::VisualTestContext| {
            visual.update(|window, cx| {
                let arena_clear = window.draw(cx);
                arena_clear.clear(cx);
            });
        };

        cx.update(|cx| {
            view.update(cx, |this, cx| {
                this.select_event(cx, "tbf20");
                this.apply_circle_map("tbf20", dummy_render_image());
            })
        });
        draw(visual);
        assert!(
            visual.debug_bounds("circle-map-image").is_some(),
            "イベントを開いてもサークルマップがデフォルト表示されていない"
        );

        cx.update(|cx| view.update(cx, |this, cx| this.toggle_circle_map(cx)));
        draw(visual);
        assert!(
            visual.debug_bounds("circle-map-image").is_none(),
            "トグルを戻してもサークルマップが残っている"
        );

        cx.update(|cx| view.update(cx, |this, cx| this.toggle_circle_map(cx)));
        draw(visual);
        assert!(
            visual.debug_bounds("circle-map-image").is_some(),
            "再度トグルしてもサークルマップが表示されない"
        );
    }

    /// 選択中イベントが初出の項目だけを抽出する。
    #[test]
    fn first_appearance_filter_extracts_the_selected_event() {
        let mut items = vec![
            test_item("first-21", 0),
            test_item("first-20", 1),
            test_item("unknown", 2),
            test_item("spaced-21", 3),
        ];
        items[0].first_appearance_event_name = Some("技術書典21".into());
        items[1].first_appearance_event_name = Some("技術書典20".into());
        items[2].first_appearance_event_name = None;
        items[3].first_appearance_event_name = Some("技術書典 21".into());

        let filtered = filter_first_appearance(&items, "技術書典21");
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].id, "first-21");
        assert_eq!(filtered[1].id, "spaced-21");

        // 別イベントを選べば、そちらが初出の項目だけになる。
        let other = filter_first_appearance(&items, "技術書典20");
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].id, "first-20");
    }

    #[test]
    fn parse_space_handles_common_formats() {
        assert_eq!(parse_space("き05"), Some(("き".into(), 5)));
        assert_eq!(parse_space("あ-01"), Some(("あ".into(), 1)));
        assert_eq!(parse_space("協01"), Some(("協".into(), 1)));
        assert_eq!(parse_space("TBD"), None);
        assert_eq!(parse_space(""), None);
    }

    /// 自作マップ: 公式画像を使わず、ブロックを 10 個ずつまとめて描く。
    #[test]
    fn self_made_tbf21_map_uses_one_block_name_and_centered_numbers() {
        let spaces = vec![
            ("き".to_string(), 5),
            ("あ".to_string(), 22),
            ("せ".to_string(), 30),
        ];
        let svg = build_circle_map_svg("tbf21", "技術書典21", &spaces);
        // ブロック名は 1 回だけ（丸なし）
        assert_eq!(svg.matches(">き</text>").count(), 1);
        assert_eq!(svg.matches(">あ</text>").count(), 1);
        assert_eq!(svg.matches(">そ</text>").count(), 1);
        assert!(
            !svg.contains("<circle cx=\"346\""),
            "ブロック名に丸が付いている"
        );
        assert!(svg.contains(">1</text>"), "開始番号 1 が無い");
        assert!(svg.contains(">10</text>"), "終了番号 10 が無い");
        // あ の各列の終了番号（22-31 / 32-40 / 41-49）
        for n in ["22", "31", "32", "40", "41", "49"] {
            assert!(
                svg.contains(&format!(">{n}</text>")),
                "あ の配置番号 {n} が無い"
            );
        }
        // 回転 transform を反映したセル中央（き01 の中心 803.5,233 に baseline 236）
        assert!(
            svg.contains("x=\"803.5\" y=\"236\""),
            "き01 の番号がセル中央に無い"
        );
        assert!(
            svg.contains("x=\"1202\" y=\"94.5\""),
            "あ01 の番号がセル中央に無い"
        );
        // き の名前はセルの外（左上）: き01 の左上 796.5,224.5 の上
        assert!(
            svg.contains("x=\"798.5\" y=\"221.5\""),
            "き の名前がセルの外に無い"
        );
        // 結合した「き(1-10)」表記は使わない
        assert!(!svg.contains("(1-10)"));
        // 赤い順番矢印は無い
        assert!(!svg.contains("fill=\"red\""));
        // SVG として描画できる（公式レイアウト 1416×379 の 2 倍）
        let image = rasterize_svg_scaled(&svg, 2.0).expect("自作マップを描画できない");
        assert_eq!(image.size(0).width.0, 2832);
        assert_eq!(image.size(0).height.0, 758);
    }

    /// tbf21 の配置図から記号（黒丸・黒四角・扉）を消し、指定された壁の段差を直線にしている。
    #[test]
    fn tbf21_map_removes_markers_doors_and_wall_jogs() {
        let svg = build_circle_map_svg("tbf21", "技術書典21", &[]);
        assert!(!svg.contains("<circle"), "黒丸が残っている");
        assert!(
            !svg.contains("M27 341h14v14H27z"),
            "塗りつぶしの黒四角が残っている"
        );
        // 扉の羽（s 曲線）だけを消し、壁の直線は残す。
        assert!(!svg.contains("s0-9.474"), "扉（左壁）が残っている");
        assert!(!svg.contains("s14.684"), "扉（上壁）が残っている");
        // 扉を消したあとに残った太い棒・黒い残骸は、周りの壁と同じ1pxの線にする。
        assert!(
            !svg.contains("M757.5 182h5v-6.5h-5z"),
            "上壁の黒い残骸が残っている"
        );
        assert!(
            !svg.contains("M757.5 183.5v5h-8"),
            "上壁の太い線が残っている"
        );
        assert!(
            !svg.contains("m-650 3.5h-45v3.5h29V189h16z"),
            "左壁の太い線が残っている"
        );
        assert!(svg.contains("M757.5 188h38"), "上壁の細い線が無い");
        // 壁とブロックの接着部は、セルの辺を壁の線に重ねて1本にする。
        assert!(
            svg.contains("M114.5 50h17v14h-17z\" transform=\"rotate(180 114.5 50)"),
            "上壁のセルが壁の線に揃っていない"
        );
        assert!(
            svg.contains("v152h347.5v-109.5"),
            "上壁の横線が細い線になっていない"
        );
        assert!(svg.contains("M66 188h41.5"), "左壁の細い線が無い");
        // 「た」のブロックは他の壁サークルと同じく壁（y=188）に接する。
        assert!(
            svg.contains("M441.5 202h17v14h-17z"),
            "「た」のブロックが壁に接していない"
        );
        assert!(
            !svg.contains("M441.5 203.5h17v14h-17z"),
            "「た」のブロックが壁から離れている"
        );
        assert!(
            !svg.contains("M62.5 188.5h45"),
            "左壁からはみ出した線が残っている"
        );
        assert!(
            !svg.contains("M762.5 175.5v6.5H807"),
            "上壁の余分な横線が残っている"
        );
        // 指定された壁の段差・刻みを直線に省略している。
        for tick in [
            "M1269.5 75.5v4",
            "M1196.5 75.5v4",
            "M1105.5 75.5v4",
            "M976.5 75.5v4",
        ] {
            assert!(!svg.contains(tick), "上壁の刻みが残っている: {tick}");
        }
        assert!(
            !svg.contains("h229v-3h120"),
            "左上の壁の段差が残っている"
        );
        assert!(
            svg.contains("M66 188V36h349v152"),
            "左上の壁が直線になっていない"
        );
        assert!(
            !svg.contains("V172h2v47.5h30.5"),
            "右壁の段差が残っている"
        );
        assert!(
            !svg.contains("h-19.5v11h-37v-11H1282v7"),
            "右下の壁の段差が残っている"
        );
        assert!(
            !svg.contains("V342h-27v2.5H295V342h-27"),
            "左下の壁の段差が残っている"
        );
        // 左壁の小さな段差をなくして、x=66 の縦線を y=188 まで通す。
        assert!(
            !svg.contains("V219h28v-31"),
            "左壁の段差が残っている"
        );
        assert!(
            svg.contains("H34.5V219H66V188"),
            "左壁の小さな段差が直線になっていない"
        );
        // 締切側の横線を「た」の壁へ段差なしで接続する。
        assert!(
            !svg.contains("h342.5v-13h5v-97"),
            "締切側の逆L字が残っている"
        );
        assert!(
            svg.contains("h347.5v-109.5"),
            "締切側の線が「た」の壁へ直線でつながっていない"
        );
    }

    /// tbf21 の固定マップ: チェック済みは橙、フォロー中は青（青優先）。
    #[test]
    fn tbf21_map_highlights_checked_and_favorite_spaces() {
        let checked = HashSet::from([("あ".to_string(), 1), ("あ".to_string(), 2)]);
        let favorite = HashSet::from([("あ".to_string(), 2)]);
        let svg = build_circle_map_svg_with_highlights(
            "tbf21",
            "技術書典21",
            &[],
            &checked,
            &favorite,
            None,
        );
        assert!(
            svg.contains("id=\"\\u304201\" fill=\"#fdba74\" fill-opacity=\"0.4\""),
            "チェック済みセルが橙色になっていない"
        );
        assert!(
            svg.contains("id=\"\\u304202\" fill=\"#93c5fd\" fill-opacity=\"0.4\""),
            "フォロー中セルが青色になっていない（青優先）"
        );
        assert!(
            svg.contains("id=\"\\u304203\" fill=\"#fff\""),
            "未チェックセルまで塗られている"
        );
    }

    /// 選択中セルは通常の橙/青より優先して緑になる。
    #[test]
    fn tbf21_map_selected_cell_uses_selected_color() {
        let checked = HashSet::from([("あ".to_string(), 1)]);
        let selected = ("あ".to_string(), 1);
        let svg = build_circle_map_svg_with_highlights(
            "tbf21",
            "技術書典21",
            &[],
            &checked,
            &HashSet::new(),
            Some(&selected),
        );
        assert!(
            svg.contains("id=\"\\u304201\" fill=\"#ef4444\" fill-opacity=\"0.65\""),
            "選択中セルが選択色になっていない"
        );
    }

    /// 固定マップのクリック判定が、色付きセルの画像座標を持つ。
    #[test]
    fn tbf21_map_has_click_rects_for_colored_cells() {
        let svg = include_str!("../../assets/tbf/tbf21-layout.svg");
        let spaces = HashSet::from([("こ".to_string(), 5), ("う".to_string(), 3)]);
        let rects = tbf21_cell_rects(svg, &spaces, CIRCLE_MAP_RASTER_SCALE);
        assert_eq!(rects.len(), 2);
        for rect in rects.values() {
            assert!(rect.x1 > rect.x0 && rect.y1 > rect.y0, "{rect:?}");
        }
    }

    /// 地図の色付きセルをクリックすると、対応する行を選択する。
    #[gpui_kit::test]
    async fn clicking_map_cell_selects_the_checklist_item(cx: &mut TestAppContext) {
        let mut items = vec![test_item("item-a", 0), test_item("item-b", 1)];
        items[0].space_number = "こ05".into();
        items[1].space_number = "う03".into();
        let mut view = view_with_items(items);
        view.selected_slug = Some("tbf20".into());
        view.circle_map_images.insert(
            "tbf20".into(),
            Arc::new(RenderImage::new([image::Frame::new(
                image::RgbaImage::from_pixel(200, 100, image::Rgba([0, 0, 0, 255])),
            )])),
        );
        view.circle_map_cells.insert(
            "tbf20".into(),
            HashMap::from([(
                ("こ".to_string(), 5),
                CellRect {
                    x0: 10.0,
                    y0: 10.0,
                    x1: 20.0,
                    y1: 20.0,
                },
            )]),
        );
        view.circle_map_bounds
            .set(Some(gpui_kit::Bounds {
                origin: gpui_kit::point(gpui_kit::px(0.0), gpui_kit::px(0.0)),
                size: gpui_kit::Size {
                    width: gpui_kit::px(200.0),
                    height: gpui_kit::px(100.0),
                },
            }));
        let view = cx.new(|_| view);
        cx.update(|cx| {
            view.update(cx, |this, cx| {
                this.select_circle_map_cell(gpui_kit::point(gpui_kit::px(15.0), gpui_kit::px(15.0)), cx);
            })
        });
        assert_eq!(
            view.read_with(cx, |view, _| view.selected_item_id.clone()),
            Some("item-a".into())
        );
    }

    /// 地図付きのテストビュー（2000×1000 の仮画像 + 指定セル）。
    fn view_with_circle_map(
        items: Vec<CheckedItem>,
        cells: HashMap<(String, u32), CellRect>,
    ) -> ChecklistView {
        let mut view = view_with_items(items);
        view.show_circle_map = true;
        view.circle_map_fit = false;
        view.circle_map_zoom = 1.0;
        view.circle_map_images.insert(
            "tbf20".into(),
            Arc::new(RenderImage::new([image::Frame::new(
                image::RgbaImage::from_pixel(2000, 1000, image::Rgba([0, 0, 0, 255])),
            )])),
        );
        view.circle_map_cells.insert("tbf20".into(), cells);
        view.circle_map_bounds.set(Some(gpui_kit::Bounds {
            origin: gpui_kit::point(gpui_kit::px(0.0), gpui_kit::px(0.0)),
            size: gpui_kit::Size {
                width: gpui_kit::px(CIRCLE_MAP_VIEWPORT_WIDTH),
                height: gpui_kit::px(CIRCLE_MAP_VIEWPORT_HEIGHT),
            },
        }));
        view
    }

    /// 地図の端のセルをクリックしても、そのセルがビューポート中央に来る。
    #[gpui_kit::test]
    async fn clicking_map_cell_centers_it_in_the_viewport(cx: &mut TestAppContext) {
        let mut items = vec![test_item("item-a", 0), test_item("item-b", 1)];
        items[0].space_number = "こ05".into();
        items[1].space_number = "う03".into();
        let view = view_with_circle_map(
            items,
            HashMap::from([(
                ("こ".to_string(), 5),
                CellRect {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 20.0,
                    y1: 20.0,
                },
            )]),
        );
        let view = cx.new(|_| view);
        cx.update(|cx| {
            view.update(cx, |this, cx| {
                this.select_circle_map_cell(
                    gpui_kit::point(gpui_kit::px(5.0), gpui_kit::px(5.0)),
                    cx,
                );
            })
        });
        let pan = view.read_with(cx, |view, _| view.circle_map_pan);
        // 画像 (2000×1000) の左上セル中心 (10, 10) がビューポート中央に来る。
        // 画像の端で clamp すると (2000-936)/2 = 532 で止まってしまう。
        assert!(
            (pan.x - (1000.0 - 10.0)).abs() < 0.5,
            "横が中央に来ていない: {pan:?}"
        );
        assert!(
            (pan.y - (500.0 - 10.0)).abs() < 0.5,
            "縦が中央に来ていない: {pan:?}"
        );
    }

    /// 一覧の行を選択すると、対応するサークルが地図の中央に来る。
    #[gpui_kit::test]
    async fn selecting_a_row_centers_its_circle_in_the_map(cx: &mut TestAppContext) {
        let mut items = vec![test_item("item-a", 0), test_item("item-b", 1)];
        items[0].space_number = "こ05".into();
        items[1].space_number = "う03".into();
        let view = view_with_circle_map(
            items,
            HashMap::from([(
                ("う".to_string(), 3),
                CellRect {
                    x0: 1900.0,
                    y0: 900.0,
                    x1: 1920.0,
                    y1: 920.0,
                },
            )]),
        );
        let view = cx.new(|_| view);
        cx.update(|cx| {
            view.update(cx, |this, cx| {
                this.select_checklist_item("item-b", cx);
            })
        });
        let pan = view.read_with(cx, |view, _| view.circle_map_pan);
        assert!(
            (pan.x - (1000.0 - 1910.0)).abs() < 0.5,
            "横が中央に来ていない: {pan:?}"
        );
        assert!(
            (pan.y - (500.0 - 910.0)).abs() < 0.5,
            "縦が中央に来ていない: {pan:?}"
        );
    }

    /// テスト用: チェックリストをウィンドウ内の固定領域に閉じ込める。
    /// 直接 Root に置くと高さが内容に合わせて伸び、リストがスクロールしない。
    struct BoundedChecklist(gpui_kit::Entity<ChecklistView>);

    impl Render for BoundedChecklist {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> impl IntoElement {
            div()
                .w(px(1200.0))
                .h(px(1000.0))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(self.0.clone()),
                )
        }
    }

    /// 地図で色付きセルを選ぶと、チェックリストの該当行が見える位置までスクロールする。
    #[gpui_kit::test]
    async fn selecting_map_cell_scrolls_the_checklist_to_the_row(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        let items: Vec<CheckedItem> = (0..150)
            .map(|index| {
                let mut item = test_item(&format!("item-{index:03}"), index as i64);
                item.space_number = format!("あ{:03}", index + 1);
                item
            })
            .collect();
        let view = view_with_circle_map(
            items,
            HashMap::from([(
                ("あ".to_string(), 100),
                CellRect {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 2000.0,
                    y1: 1000.0,
                },
            )]),
        );
        let view = cx.new(|_| view);
        let wrapper = cx.new(|_| BoundedChecklist(view.clone()));
        let window = cx.open_window(
            gpui_kit::Size {
                width: gpui_kit::px(1200.0),
                height: gpui_kit::px(1000.0),
            },
            |window, cx| gpui_kit::component::Root::new(wrapper, window, cx),
        );
        let visual = gpui_kit::VisualTestContext::from_window(*window, cx).into_mut();
        let draw = |visual: &mut gpui_kit::VisualTestContext| {
            visual.update(|window, cx| {
                let arena_clear = window.draw(cx);
                arena_clear.clear(cx);
            });
        };
        draw(visual);
        let before = view.read_with(cx, |v, _| {
            v.checklist_scroll_handle.offset().y.as_f32()
        });
        assert!(before < 1.0, "初期スクロール位置が 0 でない: {before}");

        cx.update(|cx| {
            view.update(cx, |this, cx| {
                this.select_circle_map_cell(
                    gpui_kit::point(gpui_kit::px(10.0), gpui_kit::px(10.0)),
                    cx,
                );
            })
        });
        // scroll_to_item は prepaint で適用される。数フレーム描画して反映を待つ。
        for _ in 0..3 {
            draw(visual);
        }
        let (selected, page, offset, top, bottom) = view.read_with(cx, |v, _| {
            (
                v.selected_item_id.clone(),
                v.page,
                v.checklist_scroll_handle.offset().y.as_f32(),
                v.checklist_scroll_handle.top_item(),
                v.checklist_scroll_handle.bottom_item(),
            )
        });
        assert_eq!(
            selected.as_deref(),
            Some("item-099"),
            "地図クリックで行が選択されていない"
        );
        assert_eq!(page, 1, "選択行のページへ移動していない");
        assert!(offset < -1.0, "選択行までスクロールしていない: {offset}");
        // 子 0 はヘッダー。ページ 1 の行 49 は子 50。
        assert!(
            top <= 50 && bottom >= 50,
            "選択行が表示範囲に入っていない: top={top} bottom={bottom}"
        );
    }

    /// 色付きセルの無い端のブロック（あ・ち など）も、地図を動かして中央へ寄せられる。
    #[test]
    fn circle_map_pan_can_center_the_map_edges() {
        let mut view = view_with_items(Vec::new());
        view.circle_map_images.insert(
            "tbf20".into(),
            Arc::new(RenderImage::new([image::Frame::new(
                image::RgbaImage::from_pixel(2000, 1000, image::Rgba([0, 0, 0, 255])),
            )])),
        );
        view.circle_map_zoom = 1.0;
        view.circle_map_fit = false;
        // 地図の右下端 (2000, 1000) をビューポート中央へ寄せる pan は
        // 表示サイズ / 2 - 地図座標 = -1000, -500。
        view.circle_map_pan = gpui_kit::Point::new(2000.0 / 2.0 - 2000.0, 1000.0 / 2.0 - 1000.0);
        view.clamp_circle_map_pan();
        assert!(
            (view.circle_map_pan.x - (2000.0 / 2.0 - 2000.0)).abs() < 0.5,
            "右端のブロックが中央に来られない: {:?}",
            view.circle_map_pan
        );
        assert!(
            (view.circle_map_pan.y - (1000.0 / 2.0 - 1000.0)).abs() < 0.5,
            "下端のブロックが中央に来られない: {:?}",
            view.circle_map_pan
        );
    }

    /// 会場データの無いイベントは、チェックリストのスペースから 10 個ずつ作る。
    #[test]
    fn self_made_map_derives_groups_for_other_events() {
        let spaces = vec![
            ("き".to_string(), 5),
            ("き".to_string(), 17),
            ("あ".to_string(), 3),
        ];
        let svg = build_circle_map_svg("tbf20", "技術書典20", &spaces);
        assert!(svg.contains("技術書典20 サークルマップ"));
        assert!(svg.contains("き"));
        assert!(svg.contains("(1-10)"));
        assert!(svg.contains("(11-20)"));
        assert!(svg.contains("あ"));
    }

    /// 地図モード: ドラッグで移動、ホイールで拡大縮小できる。
    #[gpui_kit::test]
    async fn circle_map_drag_pans_and_wheel_zooms(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        seed_item(cx, "item-1", "tbf20", "サークルA");
        // ドラッグ範囲を作るため横長の仮画像（2000×1000）
        let image = Arc::new(RenderImage::new([image::Frame::new(
            image::RgbaImage::from_pixel(2000, 1000, image::Rgba([240, 240, 240, 255])),
        )]));
        let view = cx.new(ChecklistView::new);
        let window = cx.open_window(
            gpui_kit::Size {
                width: gpui_kit::px(1200.0),
                height: gpui_kit::px(1000.0),
            },
            |window, cx| gpui_kit::component::Root::new(view.clone(), window, cx),
        );
        let visual = gpui_kit::VisualTestContext::from_window(*window, cx).into_mut();
        let draw = |visual: &mut gpui_kit::VisualTestContext| {
            visual.update(|window, cx| {
                let arena_clear = window.draw(cx);
                arena_clear.clear(cx);
            });
        };
        cx.update(|cx| {
            view.update(cx, |this, cx| {
                this.select_event(cx, "tbf20");
                this.apply_circle_map("tbf20", image);
                this.show_circle_map = true;
                cx.notify();
            })
        });
        // 自動フィットを解除して、パンの上限計算を画像原寸で検証する。
        cx.run_until_parked();
        cx.update(|cx| {
            view.update(cx, |this, cx| {
                this.circle_map_fit = false;
                this.circle_map_zoom = 1.0;
                this.circle_map_pan = gpui_kit::Point::new(0.0, 0.0);
                cx.notify();
            })
        });
        draw(visual);
        let bounds = visual
            .debug_bounds("circle-map-image")
            .expect("地図の表示領域が無い");
        let start = bounds.center();
        let down = start;
        let moved = gpui_kit::point(start.x + gpui_kit::px(60.0), start.y + gpui_kit::px(30.0));
        visual.simulate_event(gpui_kit::MouseDownEvent {
            position: down,
            modifiers: Default::default(),
            button: gpui_kit::MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        visual.simulate_event(gpui_kit::MouseMoveEvent {
            position: moved,
            modifiers: Default::default(),
            pressed_button: Some(gpui_kit::MouseButton::Left),
        });
        visual.simulate_event(gpui_kit::MouseUpEvent {
            button: gpui_kit::MouseButton::Left,
            position: moved,
            modifiers: Default::default(),
            click_count: 1,
        });
        draw(visual);
        let pan = view.read_with(cx, |v, _| v.circle_map_pan);
        assert!(pan.x > 10.0, "ドラッグで横に動いていない: {pan:?}");
        assert!(pan.y > 5.0, "ドラッグで縦に動いていない: {pan:?}");
        let velocity = view.read_with(cx, |v, _| v.circle_map_pan_velocity);
        assert!(
            velocity.x.abs() > 100.0,
            "ドラッグの速度が慣性用に記録されていない: {velocity:?}"
        );
        assert!(
            view.read_with(cx, |v, _| v.circle_map_drag_start.is_none()),
            "ドラッグ終了後もドラッグ中扱いになっている"
        );

        // 端のブロック（あ・ち など）も中央へ寄せられるよう、画像の端ではなく
        // 画像の中心がビューポートの端まで来る範囲まで動かせる。3 回ドラッグして
        // clamp に当てる。
        let edge = gpui_kit::point(
            bounds.origin.x + gpui_kit::px(12.0),
            bounds.origin.y + gpui_kit::px(12.0),
        );
        for _ in 0..3 {
            visual.simulate_event(gpui_kit::MouseDownEvent {
                position: start,
                modifiers: Default::default(),
                button: gpui_kit::MouseButton::Left,
                click_count: 1,
                first_mouse: false,
            });
            visual.simulate_event(gpui_kit::MouseMoveEvent {
                position: edge,
                modifiers: Default::default(),
                pressed_button: Some(gpui_kit::MouseButton::Left),
            });
            visual.simulate_event(gpui_kit::MouseUpEvent {
                button: gpui_kit::MouseButton::Left,
                position: edge,
                modifiers: Default::default(),
                click_count: 1,
            });
            draw(visual);
        }
        let pan = view.read_with(cx, |v, _| v.circle_map_pan);
        let (image_width, image_height) = view
            .read_with(cx, |v, _| v.circle_map_image_size())
            .expect("地図画像のサイズが取れない");
        let zoom = view.read_with(cx, |v, _| v.circle_map_zoom);
        let expected_x = image_width * zoom / 2.0;
        let expected_y = image_height * zoom / 2.0;
        assert!(
            (pan.x + expected_x).abs() < 1.5,
            "横の端まで移動できていない: {pan:?} (expected_x={expected_x})"
        );
        assert!(
            (pan.y + expected_y).abs() < 1.5,
            "縦の端まで移動できていない: {pan:?} (expected_y={expected_y})"
        );

        let before = view.read_with(cx, |v, _| v.circle_map_zoom);
        visual.simulate_event(gpui_kit::ScrollWheelEvent {
            position: start,
            delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0.0, 3.0)),
            ..Default::default()
        });
        // ホイールズームは 16ms ごとに目標値へ寄せるアニメーション。
        for _ in 0..40 {
            visual
                .cx
                .background_executor
                .advance_clock(std::time::Duration::from_millis(16));
            visual.run_until_parked();
        }
        draw(visual);
        let after = view.read_with(cx, |v, _| v.circle_map_zoom);
        assert!(
            after > before,
            "ホイールで拡大していない: {before} -> {after}"
        );
        assert!(
            after <= before * 1.10,
            "1 ホイールの拡大が粗すぎる: {before} -> {after}"
        );
    }

    /// ホイールの拡大縮小は、マウス直下の地点をビューポート中央へ寄せる。
    /// 同じジェスチャー中は最初の地点を固定する（中央へ寄せた後にカーソル下の
    /// 別の地点を拾って、ズームのたびに地図が流れるのを防ぐ）。
    #[gpui_kit::test]
    async fn circle_map_wheel_zoom_centers_the_point_under_the_mouse(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        seed_item(cx, "item-1", "tbf20", "サークルA");
        let image = Arc::new(RenderImage::new([image::Frame::new(
            image::RgbaImage::from_pixel(2000, 1000, image::Rgba([240, 240, 240, 255])),
        )]));
        let view = cx.new(ChecklistView::new);
        let window = cx.open_window(
            gpui_kit::Size {
                width: gpui_kit::px(1200.0),
                height: gpui_kit::px(1000.0),
            },
            |window, cx| gpui_kit::component::Root::new(view.clone(), window, cx),
        );
        let visual = gpui_kit::VisualTestContext::from_window(*window, cx).into_mut();
        let draw = |visual: &mut gpui_kit::VisualTestContext| {
            visual.update(|window, cx| {
                let arena_clear = window.draw(cx);
                arena_clear.clear(cx);
            });
        };
        cx.update(|cx| {
            view.update(cx, |this, cx| {
                this.select_event(cx, "tbf20");
                this.apply_circle_map("tbf20", image);
                this.show_circle_map = true;
                this.circle_map_fit = false;
                this.circle_map_zoom = 1.0;
                this.circle_map_pan = gpui_kit::Point::new(0.0, 0.0);
                cx.notify();
            })
        });
        cx.run_until_parked();
        draw(visual);

        let bounds = visual
            .debug_bounds("circle-map-image")
            .expect("地図の表示領域が無い");
        let center = bounds.center();
        let offset_x = 150.0;
        let offset_y = -100.0;
        let mouse = gpui_kit::point(
            center.x + gpui_kit::px(offset_x),
            center.y + gpui_kit::px(offset_y),
        );
        let local_mouse = gpui_kit::Point::new(
            (mouse.x - bounds.origin.x).as_f32(),
            (mouse.y - bounds.origin.y).as_f32(),
        );
        let viewport_center = gpui_kit::Point::new(
            bounds.size.width.as_f32() / 2.0,
            bounds.size.height.as_f32() / 2.0,
        );
        // 描画式（viewport - 表示サイズ)/2 + pan + 画像座標*zoom の逆算。
        let image_point = |position: gpui_kit::Point<f32>,
                           zoom: f32,
                           pan: gpui_kit::Point<f32>| {
            gpui_kit::Point::new(
                (position.x - (CIRCLE_MAP_VIEWPORT_WIDTH - 2000.0 * zoom) / 2.0 - pan.x) / zoom,
                (position.y - (CIRCLE_MAP_VIEWPORT_HEIGHT - 1000.0 * zoom) / 2.0 - pan.y) / zoom,
            )
        };
        let point_under_mouse =
            |zoom: f32, pan: gpui_kit::Point<f32>| image_point(local_mouse, zoom, pan);
        let point_at_center =
            |zoom: f32, pan: gpui_kit::Point<f32>| image_point(viewport_center, zoom, pan);
        let advance = |visual: &mut gpui_kit::VisualTestContext, steps: usize| {
            for _ in 0..steps {
                visual.cx.background_executor.advance_clock(std::time::Duration::from_millis(16));
                visual.run_until_parked();
            }
            draw(visual);
        };
        let target_zoom = |base: f32, lines: f32| {
            (base * 1.02_f32.powf(lines))
                .clamp(CIRCLE_MAP_MIN_ZOOM, CIRCLE_MAP_MAX_ZOOM)
        };
        let before_zoom = view.read_with(cx, |v, _| v.circle_map_zoom);
        let before_pan = view.read_with(cx, |v, _| v.circle_map_pan);
        let anchor = point_under_mouse(before_zoom, before_pan);
        let assert_centered = |zoom: f32, pan: gpui_kit::Point<f32>, message: &str| {
            let centered = point_at_center(zoom, pan);
            assert!(
                (centered.x - anchor.x).abs() < 0.6 && (centered.y - anchor.y).abs() < 0.6,
                "{message}: anchor={anchor:?} centered={centered:?} zoom={zoom} pan={pan:?}"
            );
        };

        let first_target = target_zoom(before_zoom, 3.0);
        visual.simulate_event(gpui_kit::ScrollWheelEvent {
            position: mouse,
            delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0.0, 3.0)),
            ..Default::default()
        });
        let (_zoom, pan) = view.read_with(cx, |v, _| (v.circle_map_zoom, v.circle_map_pan));
        assert!(
            (pan.x - before_pan.x).abs() < 1.0 && (pan.y - before_pan.y).abs() < 1.0,
            "ホイール直後に pan が飛んでいる: before={before_pan:?} after={pan:?}"
        );
        advance(visual, 1);
        let (zoom_tick, _) = view.read_with(cx, |v, _| (v.circle_map_zoom, v.circle_map_pan));
        assert!(
            zoom_tick > before_zoom + 0.0001 && zoom_tick < first_target - 0.0001,
            "1 フレームで目標倍率まで飛んでいる: {before_zoom} -> tick={zoom_tick} target={first_target}"
        );
        advance(visual, 40);
        let (zoom_done, pan_done) = view.read_with(cx, |v, _| (v.circle_map_zoom, v.circle_map_pan));
        assert!(
            (zoom_done - first_target).abs() < 0.001,
            "拡大が目標倍率に届いていない: {zoom_done} != {first_target}"
        );
        assert_centered(zoom_done, pan_done, "マウス直下の地点が中央に来ていない");

        let second_target = target_zoom(zoom_done, 3.0);
        visual.simulate_event(gpui_kit::ScrollWheelEvent {
            position: mouse,
            delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0.0, 3.0)),
            ..Default::default()
        });
        advance(visual, 1);
        let (zoom_tick, _) = view.read_with(cx, |v, _| (v.circle_map_zoom, v.circle_map_pan));
        assert!(
            zoom_tick > zoom_done + 0.0001 && zoom_tick < second_target - 0.0001,
            "2 回目の拡大も 1 フレームで飛んでいる: {zoom_done} -> tick={zoom_tick} target={second_target}"
        );
        advance(visual, 40);
        let (zoom_done, pan_done) = view.read_with(cx, |v, _| (v.circle_map_zoom, v.circle_map_pan));
        assert_centered(zoom_done, pan_done, "2 回目の拡大で中央の地点がずれた");

        let third_target = target_zoom(zoom_done, -6.0);
        visual.simulate_event(gpui_kit::ScrollWheelEvent {
            position: mouse,
            delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0.0, -6.0)),
            ..Default::default()
        });
        advance(visual, 1);
        let (zoom_tick, _) = view.read_with(cx, |v, _| (v.circle_map_zoom, v.circle_map_pan));
        assert!(
            zoom_tick < zoom_done - 0.0001 && zoom_tick > third_target + 0.0001,
            "縮小も 1 フレームで飛んでいる: {zoom_done} -> tick={zoom_tick} target={third_target}"
        );
        advance(visual, 40);
        let (zoom_done, pan_done) = view.read_with(cx, |v, _| (v.circle_map_zoom, v.circle_map_pan));
        assert!(
            (zoom_done - third_target).abs() < 0.001,
            "縮小が目標倍率に届いていない: {zoom_done} != {third_target}"
        );
        assert_centered(zoom_done, pan_done, "縮小で中央の地点がずれた");
    }

    /// 取得中はその行の表紙に進捗リングを出し、終わったら消す。
    #[gpui_kit::test]
    async fn sample_progress_ring_is_shown_while_fetching(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        let view = cx.new(|_| view_with_items(vec![test_item("item-1", 0)]));
        let window = cx.open_window(
            gpui_kit::Size {
                width: gpui_kit::px(1200.0),
                height: gpui_kit::px(800.0),
            },
            |window, cx| gpui_kit::component::Root::new(view.clone(), window, cx),
        );
        let visual = gpui_kit::VisualTestContext::from_window(*window, cx).into_mut();
        let draw = |visual: &mut gpui_kit::VisualTestContext| {
            visual.update(|window, cx| {
                let arena_clear = window.draw(cx);
                arena_clear.clear(cx);
            });
        };
        draw(visual);
        assert!(
            visual.debug_bounds("sample-progress-item-1").is_none(),
            "取得していない行にリングを出している"
        );

        cx.update(|cx| {
            view.update(cx, |this, cx| {
                this.sample_progress.insert("item-1".into(), 0.5);
                cx.notify();
            })
        });
        draw(visual);
        assert!(
            visual.debug_bounds("sample-progress-item-1").is_some(),
            "取得中にリングが出ていない"
        );

        cx.update(|cx| {
            view.update(cx, |this, cx| {
                this.sample_progress.remove("item-1");
                cx.notify();
            })
        });
        draw(visual);
        assert!(
            visual.debug_bounds("sample-progress-item-1").is_none(),
            "取得後にリングが残っている"
        );
    }

    /// 試し読みは `/api/image/` の画像を取得して保存し、進捗を片付ける。
    ///
    /// 本体ダウンロード用の許可リスト（`/api/product-dlc/`）をそのまま使うと
    /// **画像が全部ブロックされて試し読みが開けない**（実測で再現した不具合）。
    #[gpui_kit::test]
    async fn fetch_sample_stores_pages_from_the_image_path(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        seed_item(cx, "item-1", "tbf20", "サークルA");
        let fetched = Arc::new(Mutex::new(Vec::new()));
        login_with(
            cx,
            mock_tbf_with_samples(vec!["/api/image/1.png"], fetched.clone()),
        );
        let view = cx.new(|_| view_with_items(vec![test_item("item-1", 0)]));
        let item = test_item("item-1", 0);
        cx.update(|cx| {
            view.update(cx, |this, cx| this.fetch_sample(cx, item));
        });
        // worker（ページ取得）を進める
        for _ in 0..40 {
            cx.background_executor
                .advance_clock(std::time::Duration::from_millis(50));
            cx.run_until_parked();
        }

        assert_eq!(
            fetched.lock().unwrap().clone(),
            vec!["https://techbookfest.org/api/image/1.png".to_string()],
            "試し読み画像を取得していない（許可リストで弾かれている）"
        );
        let rows = cx.read(|cx| {
            let db = &AppState::global(cx).db_pool;
            db::samples::list_for_item(db, "item-1").unwrap()
        });
        assert_eq!(rows.len(), 1, "取得したページを保存していない");
        assert_eq!(
            cx.read(|cx| view.read(cx).sample_progress.len()),
            0,
            "取得後に進捗が残っている"
        );
    }

    /// 試し読み画像と ProductImagesQuery だけを返すモック（実通信しない）。
    fn mock_tbf_with_samples(
        image_urls: Vec<&'static str>,
        fetched: Arc<Mutex<Vec<String>>>,
    ) -> tbf::TbfClient {
        use tbf::{RequestSpec, ResponseSpec, TbfError, Transport};

        struct MockTransport {
            image_urls: Vec<&'static str>,
            fetched: Arc<Mutex<Vec<String>>>,
        }
        impl Transport for MockTransport {
            fn send(&mut self, spec: RequestSpec) -> Result<ResponseSpec, TbfError> {
                if spec.url.contains("operationName=ProductImagesQuery") {
                    let edges: Vec<serde_json::Value> = self
                        .image_urls
                        .iter()
                        .enumerate()
                        .map(|(index, url)| {
                            serde_json::json!({ "node": {
                                "id": format!("img-{index}"),
                                "url": url,
                                "width": 100,
                                "height": 140
                            }})
                        })
                        .collect();
                    let body = serde_json::to_vec(&serde_json::json!({
                        "data": { "product": { "images": { "edges": edges } } }
                    }))
                    .expect("fixture json");
                    return Ok(ResponseSpec {
                        status: 200,
                        headers: Vec::new(),
                        body,
                    });
                }
                if spec.url.contains("/api/image/") {
                    self.fetched.lock().unwrap().push(spec.url.clone());
                    return Ok(ResponseSpec {
                        status: 200,
                        headers: Vec::new(),
                        body: b"IMAGE".to_vec(),
                    });
                }
                Ok(ResponseSpec {
                    status: 404,
                    headers: Vec::new(),
                    body: Vec::new(),
                })
            }
        }
        tbf::TbfClient::with_transport(Box::new(MockTransport {
            image_urls,
            fetched,
        }))
    }

    /// 技術書典のチェックリスト応答を差し替えたクライアント（実通信しない）。
    /// `checklist_calls` にはチェックリスト取得の回数だけを数える。
    fn mock_tbf_client(items: usize, checklist_calls: Arc<AtomicUsize>) -> tbf::TbfClient {
        use tbf::{RequestSpec, ResponseSpec, TbfError, Transport};

        struct MockTransport {
            items: usize,
            checklist_calls: Arc<AtomicUsize>,
        }
        impl Transport for MockTransport {
            fn send(&mut self, spec: RequestSpec) -> Result<ResponseSpec, TbfError> {
                if spec
                    .url
                    .contains("operationName=EventOfflineCircleChecklistQuery")
                {
                    self.checklist_calls.fetch_add(1, Ordering::SeqCst);
                    let edges: Vec<serde_json::Value> = (0..self.items)
                        .map(|index| {
                            serde_json::json!({ "node": {
                                "productInfo": {
                                    "databaseID": format!("fixture-{index}"),
                                    "organization": {
                                        "name": format!("サークル{index}"),
                                        "circles": { "edges": [ { "node": {
                                            "databaseID": format!("circle-{index}"),
                                            "event": { "databaseID": "Event:tbf20" },
                                            "spaces": ["あ-01"],
                                            "hasOfflineCourse": true
                                        }}] }
                                    },
                                    "name": format!("同人誌{index}"),
                                    "productVariants": { "edges": [] },
                                    "loginUserBookShelfItem": {}
                                },
                                "createdAt": "2026-04-12T00:00:00Z"
                            }})
                        })
                        .collect();
                    let body = serde_json::to_vec(&serde_json::json!({
                        "data": { "viewer": { "checkedProductInfos": {
                            "edges": edges,
                            "pageInfo": { "hasNextPage": false }
                        }}}
                    }))
                    .expect("fixture json");
                    return Ok(ResponseSpec {
                        status: 200,
                        headers: Vec::new(),
                        body,
                    });
                }
                // イベント発見（TbfEventQuery）は無効化する → events() は canonical のみ
                Ok(ResponseSpec {
                    status: 404,
                    headers: Vec::new(),
                    body: Vec::new(),
                })
            }
        }
        tbf::TbfClient::with_transport(Box::new(MockTransport {
            items,
            checklist_calls,
        }))
    }

    /// 技術書典にログイン済みの状態にして、通信をモックに差し替える。
    fn login_with(cx: &mut TestAppContext, client: tbf::TbfClient) {
        cx.update(|cx| {
            let state = AppState::global(cx);
            *state.tbf.lock() = client;
            *state.tbf_logged_in.lock() = true;
        });
    }

    /// 空のイベントを開いたら、自動で 1 回同期して項目が表示される
    /// （全データ削除や初回インストールのあとでも、チェックリストが空のままにならない）。
    #[gpui_kit::test]
    async fn opening_empty_event_syncs_automatically(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        let calls = Arc::new(AtomicUsize::new(0));
        login_with(cx, mock_tbf_client(1, calls.clone()));
        let view = cx.new(ChecklistView::new);
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, "tbf20")));
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.items.len()), 1);
        assert_eq!(
            view.read_with(cx, |v, _| v.items[0].circle_name.clone()),
            "サークル0".to_string()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// すでに項目があるイベントを開くときは通信しない。
    #[gpui_kit::test]
    async fn opening_event_with_items_does_not_sync(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        seed_item(cx, "item-1", "tbf20", "サークルA");
        let calls = Arc::new(AtomicUsize::new(0));
        login_with(cx, mock_tbf_client(1, calls.clone()));
        let view = cx.new(ChecklistView::new);
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, "tbf20")));
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            view.read_with(cx, |v, _| v.items[0].circle_name.clone()),
            "サークルA".to_string()
        );
    }

    /// 未ログインなら自動同期しない（ログインを促すトーストも出さない。
    /// 画面を開いただけで邪魔しない）。
    #[gpui_kit::test]
    async fn not_logged_in_does_not_auto_sync(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        let calls = Arc::new(AtomicUsize::new(0));
        cx.update(|cx| *AppState::global(cx).tbf.lock() = mock_tbf_client(1, calls.clone()));
        let view = cx.new(ChecklistView::new);
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, "tbf20")));
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(view.read_with(cx, |v, _| v.toast.is_none()));
    }

    /// サーバー側も空だったときは、同じイベントを開き直しても再試行しない
    /// （自動同期でサーバーを叩き続けない）。
    #[gpui_kit::test]
    async fn empty_event_does_not_sync_again(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        let calls = Arc::new(AtomicUsize::new(0));
        login_with(cx, mock_tbf_client(0, calls.clone()));
        let view = cx.new(ChecklistView::new);
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, "tbf20")));
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // 一覧に戻って開き直しても、もう自動同期しない
        cx.update(|cx| view.update(cx, |this, cx| this.back_to_list(cx)));
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, "tbf20")));
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// 別のイベントを同期している間に空のイベントを開いた場合でも、
    /// 同期が終わった時点でそのイベントの自動同期が走る。
    #[gpui_kit::test]
    async fn empty_event_opened_during_sync_is_synced_after_it_finishes(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        seed_event(cx, "tbf19", "技術書典19");
        // fixture の購入日は技術書典20 の開催期間なので、アイテムは tbf19 側に置く
        seed_item(cx, "item-1", "tbf19", "サークルA");
        let calls = Arc::new(AtomicUsize::new(0));
        login_with(cx, mock_tbf_client(1, calls.clone()));
        let view = cx.new(ChecklistView::new);
        // アイテムのあるイベントを開く（自動同期は走らない）
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, "tbf19")));
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // 同期中（busy）に空のイベントへ移動しても、その場では見送られる
        cx.update(|cx| view.update(cx, |this, _| this.busy = true));
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, "tbf20")));
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // 同期が終わったら、いま開いている空のイベントを取得する
        cx.update(|cx| view.update(cx, |this, cx| this.finish_sync(cx, Ok(0))));
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(view.read_with(cx, |v, _| v.items.len()), 1);
        assert_eq!(
            view.read_with(cx, |v, _| v.items[0].circle_name.clone()),
            "サークル0".to_string()
        );
    }

    /// 設定の「ローカルデータをすべて削除」のあとは、同じイベントを開き直せば
    /// もう一度自動同期する（削除前の試行履歴を引きずらない）。
    #[gpui_kit::test]
    async fn opening_event_after_local_data_delete_syncs_again(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_test);
        seed_event(cx, "tbf20", "技術書典20");
        let calls = Arc::new(AtomicUsize::new(0));
        login_with(cx, mock_tbf_client(0, calls.clone()));
        let view = cx.new(ChecklistView::new);
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, "tbf20")));
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // 設定画面の「ローカルデータをすべて削除」と同じ通知
        cx.update(|cx| {
            AppState::global(cx)
                .local_data_deleted
                .store(true, Ordering::SeqCst)
        });
        cx.update(|cx| view.update(cx, |this, cx| this.back_to_list(cx)));
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, "tbf20")));
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// サンプル（デモ）モードはチェックリストの同期でネットワークへ出ない。
    ///
    /// サンプルはダミーのセッションだけを持ち、本物の技術書典へリクエストを投げない
    /// （投げても「セッション切れ」の通知が出るだけで、実在のアカウントには何も起きない）。
    #[gpui_kit::test]
    async fn demo_mode_sync_does_not_hit_the_network(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(AppState::init_demo);
        let calls = Arc::new(AtomicUsize::new(0));
        // 実クライアントの代わりに、呼ばれたら数えるモックを差し込む
        cx.update(|cx| *AppState::global(cx).tbf.lock() = mock_tbf_client(1, calls.clone()));
        let slug = cx
            .read(|cx| {
                checklist_db::list_events(&AppState::global(cx).db_pool)
                    .unwrap()
                    .first()
                    .and_then(|event| event.slug.clone())
            })
            .expect("サンプルのイベントが無い");
        let view = cx.new(ChecklistView::new);
        cx.update(|cx| view.update(cx, |this, cx| this.select_event(cx, &slug)));
        cx.run_until_parked();
        cx.update(|cx| view.update(cx, |this, cx| this.sync(cx)));
        cx.run_until_parked();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "サンプルモードでチェックリストの同期がネットワークへ出ている"
        );
        assert!(
            view.read_with(cx, |v, _| v.toast.is_none()),
            "サンプルモードで同期の通知が出ている"
        );
    }
}
