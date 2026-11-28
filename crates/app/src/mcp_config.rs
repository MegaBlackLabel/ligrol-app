//! MCP クライアント（Claude Desktop / Cursor など）へ貼る設定スニペットを作る。
//!
//! `ligrol-mcp` の実体は配布経路で置き場所が違う。
//!
//! - MSIX（Microsoft Store / サイドロード）: `WindowsApps` 直下の exe は外部プロセスから
//!   起動できない（`EPERM`）。マニフェストで宣言した App Execution Alias の
//!   **PFN サブディレクトリ**（`%LOCALAPPDATA%\Microsoft\WindowsApps\<PFN>\`）を使う。
//!   このパスはパッケージ登録時に無条件で作られ、Settings のオン/オフや更新の影響を受けない。
//! - MSI / ZIP / macOS `.app`: アプリの実行ファイルと同じディレクトリ。
use std::path::{Path, PathBuf};

/// MCP サーバーの実行ファイル名。実行ファイルが `.exe` なら `.exe` を付ける。
fn mcp_binary_name(exe: &Path) -> &'static str {
    if exe
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        "ligrol-mcp.exe"
    } else {
        "ligrol-mcp"
    }
}

/// MCP サーバーの起動パスを返す。
///
/// `package_family` は MSIX の Package Family Name（パッケージ ID が無ければ `None`）。
/// `local_app_data` は `%LOCALAPPDATA%`（テストから差し替えられるように引数で受ける）。
pub fn server_command(
    exe: &Path,
    package_family: Option<&str>,
    local_app_data: Option<&Path>,
) -> PathBuf {
    let family = package_family.map(str::trim).filter(|name| !name.is_empty());
    if let (Some(family), Some(local_app_data)) = (family, local_app_data) {
        return local_app_data
            .join("Microsoft")
            .join("WindowsApps")
            .join(family)
            .join("ligrol-mcp.exe");
    }
    exe.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(mcp_binary_name(exe))
}

/// そのまま貼れる JSON 設定（`mcpServers.ligrol.command`）を返す。
pub fn config_json(command: &Path) -> String {
    serde_json::json!({
        "mcpServers": {
            "ligrol": {
                "command": command.to_string_lossy(),
            }
        }
    })
    .to_string()
}

/// 実行中プロセスの経路から設定スニペットを作る。
///
/// MSIX で動いているときは App Execution Alias の PFN 固定パス、それ以外は
/// 実行ファイルの隣を案内する。`WindowsApps` 直下を案内すると外部プロセスから
/// 起動できず `EPERM` になるため、パッケージ ID を必ず先に見る。
///
/// 描画のたびに Win32 を呼ばないよう、プロセス内で 1 回だけ計算する。
pub fn config_snippet() -> String {
    static SNIPPET: std::sync::LazyLock<String> = std::sync::LazyLock::new(build_snippet);
    SNIPPET.clone()
}

fn build_snippet() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    let family = current_package_family();
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    config_json(&server_command(&exe, family.as_deref(), local_app_data.as_deref()))
}

/// このプロセスの MSIX Package Family Name（パッケージ ID が無ければ `None`）。
#[cfg(windows)]
fn current_package_family() -> Option<String> {
    use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFamilyName;

    // 1 回目は必要な長さ（終端 NUL 込み）だけを返す。
    let mut length: u32 = 0;
    unsafe { GetCurrentPackageFamilyName(&mut length, std::ptr::null_mut()) };
    if length == 0 {
        return None;
    }
    let mut buffer = vec![0u16; length as usize];
    let status = unsafe { GetCurrentPackageFamilyName(&mut length, buffer.as_mut_ptr()) };
    if status != 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

#[cfg(not(windows))]
fn current_package_family() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msix_uses_the_stable_alias_path() {
        let command = server_command(
            Path::new("ligrol.exe"),
            Some("63355MegaBlackLabel.Ligrol_n2f6xz8p4tfny"),
            Some(Path::new("local-app-data")),
        );
        let expected = Path::new("local-app-data")
            .join("Microsoft")
            .join("WindowsApps")
            .join("63355MegaBlackLabel.Ligrol_n2f6xz8p4tfny")
            .join("ligrol-mcp.exe");
        assert_eq!(command, expected);
    }

    #[test]
    fn unpackaged_windows_uses_the_sibling_executable() {
        let command = server_command(
            Path::new("portable").join("ligrol.exe").as_path(),
            None,
            Some(Path::new("local-app-data")),
        );
        assert_eq!(command, Path::new("portable").join("ligrol-mcp.exe"));
    }

    #[test]
    fn mac_app_bundle_uses_the_binary_next_to_the_app_executable() {
        let exe = Path::new("Ligrol.app").join("Contents").join("MacOS").join("ligrol");
        let command = server_command(&exe, None, None);
        assert_eq!(
            command,
            Path::new("Ligrol.app")
                .join("Contents")
                .join("MacOS")
                .join("ligrol-mcp")
        );
    }

    #[test]
    fn packaged_without_local_app_data_falls_back_to_the_sibling() {
        let exe = Path::new("Ligrol").join("ligrol.exe");
        let command = server_command(&exe, Some("PFN"), None);
        assert_eq!(command, Path::new("Ligrol").join("ligrol-mcp.exe"));
    }

    #[test]
    fn empty_package_family_is_not_treated_as_packaged() {
        let exe = Path::new("Ligrol").join("ligrol.exe");
        let command = server_command(&exe, Some("  "), Some(Path::new("local")));
        assert_eq!(command, Path::new("Ligrol").join("ligrol-mcp.exe"));
    }

    #[test]
    fn config_json_is_valid_and_keeps_the_windows_path() {
        let command = Path::new("local")
            .join("Microsoft")
            .join("WindowsApps")
            .join("PFN")
            .join("ligrol-mcp.exe");
        let text = config_json(&command);
        let value: serde_json::Value =
            serde_json::from_str(&text).expect("設定スニペットが JSON として壊れている");
        assert_eq!(
            value["mcpServers"]["ligrol"]["command"],
            command.to_string_lossy().as_ref()
        );
    }
}
