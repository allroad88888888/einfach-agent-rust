//! CLI 的识图工具（`srv:vision/inspect`，s5）怎么从 `providers.toml` 里解出来。
//!
//! 从 `main.rs` 分出来的一件事（149 装配链又长了一截，那个文件顶破 300 行）：
//! 「vision 这个可选零件的解析与它的横幅措辞」。装配本身仍然在 `main.rs`——这里
//! 只回答「有没有它、它长什么样」。

use std::sync::Arc;

use agent_tools::{VisionLinkSource, VisionRuntime};
use agent_transport::{Client, config};

/// `srv:vision/inspect` 的 CLI 运行时（写死 Kimi 3）：从 `[providers.kimi]` 段解
/// base_url/key/model；`image` 参数按启动目录内的本地相对路径解析
/// （`VisionLinkSource::LocalRoot`）——CLI 没有 server 的上传端点。kimi 段缺失或
/// 没配可用 key → `None`（工具不配置、不声明，其余照常），跟
/// `agent-server::bootstrap::resolve_vision` 同一个取舍。
pub fn resolve(root: &config::RootConfig, tool_root: &std::path::Path) -> Option<VisionRuntime> {
    let kimi = root.providers.get("kimi")?;
    let api_key = kimi.resolve_key()?;
    Some(VisionRuntime::new(
        Arc::new(Client::new()),
        kimi.base_url.clone(),
        api_key,
        Arc::from(kimi.model.as_str()),
        VisionLinkSource::LocalRoot(tool_root.to_path_buf()),
    ))
}

/// 启动横幅里 `vision=` 后面那半句。
pub fn banner(enabled: bool) -> &'static str {
    if enabled {
        "可用（kimi 已配置）"
    } else {
        "（未配置 kimi 段，无 srv:vision/inspect）"
    }
}
