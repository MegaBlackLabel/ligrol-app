//! 自動アップデート（`gpui-updater-pre`）の配線。
//!
//! フィードは GitHub Releases（`SHA256SUMS` + minisign 署名）。インストールは
//! プラットフォーム固定（macOS = dmg / Windows = msi）で、配布物の整備は
//! `docs/macos-update-handoff.md` を参照。
//!
//! このモジュールは **純粋な判定**（インターバル・文言・所有権ゲート）と、
//! GPUI のエンティティ（[`Updates`]）を持つ。ネットワークへ出るのは
//! [`Updates::check_now`] と起動後の自動チェックだけ。

#[cfg(not(test))]
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _,
    ReadGlobal as _, Render, Styled as _, Subscription, Window, div,
};
#[cfg(target_os = "macos")]
use gpui_updater_pre::{BundleVersion, CodeSigning};
use gpui_updater_pre::{
    EngineConfig, ErrorKind, GitHubSource, Ownership, OwnershipHints, PackageManager, UpdateStatus,
    Updater, Verification, Version, VersionBinding, current_install_root, install,
};
use ligrol_core::db;

use crate::app_state::AppState;

/// 自動チェックの間隔（24 時間）。
pub const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// 前回チェック日時の保存キー（`app_settings`）。
const LAST_CHECK_KEY: &str = "updates.last_check_unix";

/// リリースを置いている GitHub リポジトリ。
const REPO_OWNER: &str = "MegaBlackLabel";
const REPO_REPO: &str = "ligrol-app";

/// リリースに同梱する集約チェックサム。
const CHECKSUM_ASSET: &str = "SHA256SUMS";

/// Microsoft Store の「ダウンロードと更新」ページ。
const STORE_UPDATES_URL: &str = "ms-windows-store://downloadsandupdates";

/// 起動後、最初の自動チェックまでの待ち時間（起動直後の重さを避ける）。
#[cfg(not(test))]
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);

/// 更新ボタンが行う操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateAction {
    Check,
    Download,
    Cancel,
    Restart,
    /// Microsoft Store の更新ページを開く（MSIX / Store 版）。
    OpenStore,
}

impl UpdateAction {
    /// ボタンの文言。
    pub fn label(self) -> &'static str {
        match self {
            Self::Check => "アップデートを確認",
            Self::Download => "ダウンロードしてインストール",
            Self::Cancel => "中止",
            Self::Restart => "再起動して更新",
            Self::OpenStore => "Microsoft Store で更新",
        }
    }
}

/// 画面に出す 1 行の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    pub headline: String,
    pub detail: Option<String>,
    pub action: Option<UpdateAction>,
}

/// 実行中のバージョン（`Cargo.toml` の version）。
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION は semver")
}

/// 前回のチェックから 1 日経ったか。`None`（未チェック）は常に真。
pub fn check_due(last_check_unix: Option<i64>, now_unix: i64) -> bool {
    last_check_unix.is_none_or(|last| now_unix.saturating_sub(last) >= CHECK_INTERVAL_SECS)
}

/// 表示用のバイト数（例: `1.5 MiB`）。
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// ダウンロード進捗の表示（例: `25% (2.0 MiB / 8.0 MiB)`）。
pub fn progress_text(downloaded: u64, total: Option<u64>) -> String {
    match total.filter(|total| *total > 0) {
        Some(total) => format!(
            "{}% ({}/{})",
            downloaded.saturating_mul(100) / total,
            human_bytes(downloaded),
            human_bytes(total)
        ),
        None => format!("{} 受信", human_bytes(downloaded)),
    }
}

/// `UpdateStatus` を画面用の文言にする。
pub fn describe(status: &UpdateStatus) -> StatusLine {
    match status {
        UpdateStatus::Idle => StatusLine {
            headline: "未確認".into(),
            detail: Some("アップデートを確認できます".into()),
            action: Some(UpdateAction::Check),
        },
        UpdateStatus::Checking => StatusLine {
            headline: "確認しています…".into(),
            detail: None,
            action: None,
        },
        UpdateStatus::UpToDate => StatusLine {
            headline: "最新版です".into(),
            detail: Some(format!("現在のバージョンは v{} です", current_version())),
            action: Some(UpdateAction::Check),
        },
        UpdateStatus::Available(version) => StatusLine {
            headline: format!("v{version} が利用可能です"),
            detail: Some("ダウンロードしてインストールできます".into()),
            action: Some(UpdateAction::Download),
        },
        UpdateStatus::Downloading { downloaded, total } => StatusLine {
            headline: "ダウンロードしています…".into(),
            detail: Some(progress_text(*downloaded, *total)),
            action: Some(UpdateAction::Cancel),
        },
        UpdateStatus::Installing => StatusLine {
            headline: "インストールしています…".into(),
            detail: None,
            action: None,
        },
        UpdateStatus::Staged(version) => StatusLine {
            headline: format!("v{version} の準備ができました"),
            detail: Some("再起動すると更新が適用されます".into()),
            action: Some(UpdateAction::Restart),
        },
        UpdateStatus::Errored(error) => describe_error(error.kind(), &error.to_string()),
        _ => StatusLine {
            headline: "更新の状態を取得できません".into(),
            detail: None,
            action: Some(UpdateAction::Check),
        },
    }
}

/// 失敗の種類ごとの文言。`Verification` は再試行を出さない。
pub fn describe_error(kind: ErrorKind, message: &str) -> StatusLine {
    let (headline, detail, action) = match kind {
        ErrorKind::Network => (
            "更新を確認できませんでした",
            message.to_string(),
            Some(UpdateAction::Check),
        ),
        ErrorKind::Verification => (
            "更新の検証に失敗しました",
            "配布元の署名またはハッシュが一致しません。手動でダウンロードしてください。"
                .to_string(),
            None,
        ),
        ErrorKind::Permission | ErrorKind::Location => {
            ("この場所では自動更新できません", message.to_string(), None)
        }
        ErrorKind::Metadata => ("更新情報を取得できませんでした", message.to_string(), None),
        ErrorKind::Install | ErrorKind::Unsupported => (
            "自動更新を実行できません",
            "手動でダウンロードしてください。".to_string(),
            None,
        ),
        ErrorKind::Cancelled => (
            "更新を中止しました",
            message.to_string(),
            Some(UpdateAction::Check),
        ),
        _ => (
            "更新に失敗しました",
            message.to_string(),
            Some(UpdateAction::Check),
        ),
    };
    StatusLine {
        headline: headline.into(),
        detail: Some(detail),
        action,
    }
}

/// このインストールをアプリ自身が更新してよいか。
pub fn self_update_allowed(ownership: &Ownership) -> bool {
    ownership.is_self_update()
}

/// Microsoft Store が管理しているインストールか。
pub fn is_store_managed(ownership: &Ownership) -> bool {
    matches!(
        ownership,
        Ownership::Managed {
            manager: PackageManager::MicrosoftStore,
            ..
        }
    )
}

/// 自己更新できないときの案内（`SelfUpdate` では空）。
pub fn ownership_note(ownership: &Ownership) -> String {
    match ownership {
        Ownership::SelfUpdate => String::new(),
        Ownership::Managed { manager, .. } => match ownership.update_command() {
            Some(command) => {
                format!("このアプリは {manager} が管理しています。`{command}` で更新してください。")
            }
            None => format!("このアプリは {manager} が管理しています。"),
        },
        Ownership::Packager(note) => note.clone(),
        _ => String::new(),
    }
}

/// 現在プラットフォームのアセット選択パターン（OS + 拡張子）。
///
/// macOS は aarch64 / x86_64 の両方を配るため、さらにアーキテクチャで絞る。
/// `GitHubSource` は最初にマッチしたアセットを返し、GitHub のアセット順は
/// 不定なので、絞らないと Intel Mac が arm64 の dmg を選び得る。
pub fn asset_patterns() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        vec![
            "macos".to_string(),
            ".dmg".to_string(),
            std::env::consts::ARCH.to_string(),
        ]
    }
    #[cfg(target_os = "windows")]
    {
        vec!["windows".to_string(), ".msi".to_string()]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        vec!["linux".to_string(), ".tar.gz".to_string()]
    }
}

/// 更新フィード（GitHub Releases）。
///
/// 配布物の追加は `docs/macos-update-handoff.md` を参照。
pub fn source() -> GitHubSource {
    GitHubSource::new(REPO_OWNER, REPO_REPO)
        .asset_patterns(asset_patterns())
        .with_checksums(CHECKSUM_ASSET)
        .with_minisig()
}

/// 検証設定。
///
/// minisign 公開鍵がビルドに埋め込まれていれば fail-closed（署名 + SHA-256）。
/// 鍵が無いビルド（開発版）はチェックサムだけを必須にする。
pub fn engine_config() -> EngineConfig {
    let config = EngineConfig::new(current_version());
    // macOS は公証済み .app の版と署名 Team を検証する（差し替え後に壊れていないか）。
    #[cfg(target_os = "macos")]
    let config = config
        .bundle_version(BundleVersion::MustMatch)
        .code_signing(CodeSigning::MatchInstalled);
    match option_env!("LIGROL_UPDATE_MINISIGN_KEY").filter(|key| !key.trim().is_empty()) {
        Some(key) => config
            .verification(Verification::Strict)
            .version_binding(VersionBinding::Required)
            .minisign_public_key(key),
        None => config.verification(Verification::Checksum),
    }
}

/// いまのインストールを誰が更新するか。
pub fn ownership_now() -> Ownership {
    let hints = OwnershipHints::new().explanation(option_env!("LIGROL_UPDATE_EXPLANATION"));
    current_install_root().map_or(Ownership::SelfUpdate, |root| {
        install::ownership(&root, &hints)
    })
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(not(test))]
fn last_check_unix(pool: &ligrol_core::db::SqlitePool) -> Option<i64> {
    db::settings::get(pool, LAST_CHECK_KEY)
        .ok()
        .flatten()?
        .parse()
        .ok()
}

/// 設定画面の「アップデート」節。
///
/// `Entity<Updater>` を内側に持ち、状態の変化を購読して描き直す。
pub struct Updates {
    updater: Entity<Updater>,
    ownership: Ownership,
    demo: bool,
    _observe: Subscription,
}

impl Updates {
    pub fn new(cx: &mut Context<Self>, demo: bool) -> Self {
        let updater = cx.new(|cx| Updater::new(source(), engine_config(), cx));
        let observe = cx.observe(&updater, |_, _, cx| cx.notify());
        let this = Self {
            updater,
            ownership: ownership_now(),
            demo,
            _observe: observe,
        };
        #[cfg(not(test))]
        if !demo && this.can_self_update() {
            this.schedule_auto_check(cx);
        }
        this
    }

    /// 起動後しばらく待ってから、24 時間おきに確認する。
    #[cfg(not(test))]
    fn schedule_auto_check(&self, cx: &mut Context<Self>) {
        let handle = cx.entity();
        cx.spawn(async move |_weak, cx| {
            // 初回だけ起動 30 秒後。以降は 24 時間おき。
            cx.background_executor().timer(FIRST_CHECK_DELAY).await;
            loop {
                handle.update(cx, |this, cx| this.check_if_due(cx));
                cx.background_executor()
                    .timer(Duration::from_secs(CHECK_INTERVAL_SECS as u64))
                    .await;
            }
        })
        .detach();
    }

    fn can_self_update(&self) -> bool {
        self_update_allowed(&self.ownership)
    }

    #[cfg(not(test))]
    fn check_if_due(&mut self, cx: &mut Context<Self>) {
        if self.demo || !self.can_self_update() {
            return;
        }
        let pool = AppState::global(cx).db_pool.clone();
        if check_due(last_check_unix(&pool), now_unix()) {
            self.check_now(cx);
        }
    }

    /// 手動チェック（最終チェック日時を記録する）。
    pub fn check_now(&mut self, cx: &mut Context<Self>) {
        if self.demo || !self.can_self_update() {
            return;
        }
        let pool = AppState::global(cx).db_pool.clone();
        if let Err(error) = db::settings::set(&pool, LAST_CHECK_KEY, &now_unix().to_string()) {
            log::warn!("更新の最終確認日時を保存できない: {error}");
        }
        self.updater.update(cx, |updater, cx| updater.check(cx));
    }

    fn primary_action(&mut self, cx: &mut Context<Self>) {
        let status = self.updater.read(cx).status().clone();
        match status {
            UpdateStatus::Available(_) => {
                self.updater
                    .update(cx, |updater, cx| updater.download_and_install(cx));
            }
            UpdateStatus::Downloading { .. } => {
                self.updater.update(cx, |updater, cx| updater.cancel(cx));
            }
            UpdateStatus::Staged(_) => {
                self.updater.update(cx, |updater, cx| updater.restart(cx));
            }
            _ => self.check_now(cx),
        }
    }

    fn release_url(&self, cx: &Context<Self>) -> Option<String> {
        self.updater
            .read(cx)
            .available()
            .and_then(|release| release.release_url.clone())
    }
}

impl Render for Updates {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_fg = cx.theme().muted_foreground;
        let status = self.updater.read(cx).status().clone();
        let line = describe(&status);
        let store_managed = is_store_managed(&self.ownership);
        // Store 版は GitHub フィードを確認しないので、状態ではなく Store の案内を出す。
        let (headline, detail) = if store_managed {
            (
                "Microsoft Store 版です".to_string(),
                Some("更新は Microsoft Store の「ダウンロードと更新」から行います".to_string()),
            )
        } else {
            (line.headline.clone(), line.detail.clone())
        };
        let handle = cx.entity();
        let mut content = div().flex().flex_col().gap_3().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(headline),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(format!("現在 v{}", current_version())),
                ),
        );
        if let Some(detail) = detail {
            content = content.child(div().text_xs().text_color(muted_fg).child(detail));
        }

        if self.demo {
            content = content.child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child("サンプルモードでは更新しません"),
            );
        } else if store_managed {
            content = content.child(
                div().flex().flex_row().items_center().gap_2().child(
                    Button::new("update-open-store")
                        .cursor_pointer()
                        .label(UpdateAction::OpenStore.label())
                        .on_click(|_, _window, cx: &mut App| cx.open_url(STORE_UPDATES_URL)),
                ),
            );
        } else if !self.can_self_update() {
            content = content.child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(ownership_note(&self.ownership)),
            );
        } else if let Some(action) = line.action {
            content = content.child(
                div().flex().flex_row().items_center().gap_2().child(
                    Button::new("update-action")
                        .cursor_pointer()
                        .label(action.label())
                        .on_click(move |_, _window, cx: &mut App| {
                            handle.update(cx, |this, cx| this.primary_action(cx));
                        }),
                ),
            );
        }

        if !store_managed && let Some(url) = self.release_url(cx) {
            content = content.child(
                div().flex().flex_row().child(
                    Button::new("update-release-notes")
                        .cursor_pointer()
                        .ghost()
                        .small()
                        .label("リリースノート")
                        .on_click(move |_, _window, cx: &mut App| cx.open_url(&url)),
                ),
            );
        }

        if !self.demo {
            content = content.child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(if store_managed {
                        "Microsoft Store が更新を管理しています"
                    } else if self.can_self_update() {
                        "GitHub Releases から更新します"
                    } else {
                        "このインストールはアプリ外で管理されています"
                    }),
            );
        }

        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_is_due_after_one_day() {
        assert!(check_due(None, 1_000));
        assert!(!check_due(Some(1_000), 1_000 + CHECK_INTERVAL_SECS - 1));
        assert!(check_due(Some(1_000), 1_000 + CHECK_INTERVAL_SECS));
    }

    #[test]
    fn idle_offers_a_check() {
        let line = describe(&UpdateStatus::Idle);
        assert_eq!(line.headline, "未確認");
        assert_eq!(line.action, Some(UpdateAction::Check));
    }

    #[test]
    fn available_offers_a_download() {
        let line = describe(&UpdateStatus::Available(Version::new(1, 2, 3)));
        assert!(line.headline.contains("1.2.3"), "{line:?}");
        assert_eq!(line.action, Some(UpdateAction::Download));
    }

    #[test]
    fn staged_offers_a_restart() {
        let line = describe(&UpdateStatus::Staged(Version::new(1, 2, 3)));
        assert_eq!(line.action, Some(UpdateAction::Restart));
    }

    #[test]
    fn downloading_shows_progress_and_cancel() {
        let line = describe(&UpdateStatus::Downloading {
            downloaded: 50,
            total: Some(200),
        });
        assert_eq!(line.action, Some(UpdateAction::Cancel));
        assert!(
            line.detail.as_deref().is_some_and(|d| d.contains("25%")),
            "{line:?}"
        );

        let line = describe(&UpdateStatus::Downloading {
            downloaded: 512 * 1024,
            total: None,
        });
        assert!(
            line.detail.as_deref().is_some_and(|d| d.contains("KiB")),
            "{line:?}"
        );
    }

    #[test]
    fn verification_failures_do_not_offer_a_retry() {
        let line = describe_error(ErrorKind::Verification, "bad signature");
        assert_eq!(line.action, None);
        assert!(line.detail.is_some());
    }

    #[test]
    fn network_failures_can_be_retried() {
        assert_eq!(
            describe_error(ErrorKind::Network, "timeout").action,
            Some(UpdateAction::Check)
        );
    }

    #[test]
    fn managed_installs_do_not_self_update() {
        let brew = Ownership::managed(PackageManager::Homebrew, Some("ligrol".into()));
        assert!(!self_update_allowed(&brew));
        assert!(
            ownership_note(&brew).contains("brew upgrade ligrol"),
            "{}",
            ownership_note(&brew)
        );

        assert!(self_update_allowed(&Ownership::SelfUpdate));
        assert!(ownership_note(&Ownership::SelfUpdate).is_empty());

        assert_eq!(
            ownership_note(&Ownership::Packager("ストアから更新してください".into())),
            "ストアから更新してください"
        );
    }

    #[test]
    fn store_installs_use_the_store_and_skip_self_update() {
        assert_eq!(UpdateAction::OpenStore.label(), "Microsoft Store で更新");

        let store = Ownership::managed(PackageManager::MicrosoftStore, None);
        assert!(is_store_managed(&store));
        assert!(!self_update_allowed(&store));

        let brew = Ownership::managed(PackageManager::Homebrew, Some("ligrol".into()));
        assert!(!is_store_managed(&brew));
        assert!(!self_update_allowed(&brew));
        assert!(!is_store_managed(&Ownership::SelfUpdate));
    }

    #[test]
    fn action_labels_are_japanese() {
        assert_eq!(UpdateAction::Check.label(), "アップデートを確認");
        assert_eq!(
            UpdateAction::Download.label(),
            "ダウンロードしてインストール"
        );
        assert_eq!(UpdateAction::Cancel.label(), "中止");
        assert_eq!(UpdateAction::Restart.label(), "再起動して更新");
    }

    #[test]
    fn asset_patterns_target_the_current_platform() {
        let patterns = asset_patterns();
        assert!(
            patterns.iter().any(|p| p == std::env::consts::OS),
            "OS パターンが無い: {patterns:?}"
        );

        // macOS は aarch64 / x86_64 の両方を配るため、アーキテクチャで絞る。
        #[cfg(target_os = "macos")]
        {
            assert!(patterns.iter().any(|p| p == ".dmg"), "{patterns:?}");
            assert!(
                patterns.iter().any(|p| p == std::env::consts::ARCH),
                "macOS のアセット選択はアーキテクチャで絞るべき: {patterns:?}"
            );
        }
    }
}
