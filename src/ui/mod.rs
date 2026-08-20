pub mod main_window;
pub mod progress;
pub mod settings;

use std::path::Path;

/// 在系统文件管理器中打开目录(转码完成后用)。
pub fn open_in_file_manager(path: &Path) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    let _ = result;
}
