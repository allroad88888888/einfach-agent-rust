//! 平台标准目录：issue 036「providers.toml 与会话目录放平台标准目录（`dirs`
//! 或 Tauri path API）」。这个文件只干一件事——把 Tauri 的 `app.path()`
//! （底层就是 `dirs` crate，见 `tauri::path::PathResolver::app_config_dir`
//! 源码）翻成这个应用具体要用的四个路径，不掺任何起服务器/读配置的逻辑。
//!
//! macOS 上 `app_config_dir`/`app_data_dir` 会解到同一个目录
//! （`~/Library/Application Support/<identifier>/`——`dirs` crate 对 macOS 的
//! 一贯处理：苹果自己的 HIG 不像 XDG 那样区分配置与数据），issue 原文点名的
//! macOS 例子在这里成立；Linux/Windows 上两者按 XDG/Known Folder 各自分开，
//! 用 Tauri path API 而不是手写 `~/.config/...` 正是为了这个可移植性，不需要
//! 这个 crate 自己再判断平台。

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

/// `providers.toml` 该放的目录（文件本身是否存在由调用方判断——这里只给目录）。
pub fn config_dir(app: &AppHandle) -> tauri::Result<PathBuf> {
    app.path().app_config_dir()
}

/// `providers.toml` 的完整路径——`agent_transport::config` 的
/// `$AGENT_PROVIDERS_CONFIG` 查找档位喂这个。
pub fn providers_toml_path(app: &AppHandle) -> tauri::Result<PathBuf> {
    Ok(config_dir(app)?.join("providers.toml"))
}

/// `POST /sessions` 不给 `session_path` 时自动落盘的目录
/// （`SessionTemplate::default_sessions_dir`）——首启就有意义，不需要用户先手
/// 动建一次会话目录。
pub fn sessions_dir(app: &AppHandle) -> tauri::Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("sessions"))
}

/// 内置工具路径监狱的根目录（`SessionTemplate::tools_root`）——跟会话目录同一个
/// app data 根下分开一个子目录，互不踩脚。
pub fn tools_root(app: &AppHandle) -> tauri::Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("tools"))
}
