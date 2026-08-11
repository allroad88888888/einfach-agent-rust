//! 读 `providers.toml`（**native 专属**——浏览器没有文件系统，wasm 宿主的配置
//! 来源见 issue 114d：`crate::provider_config::ProviderConfig::from_host` /
//! `RootConfig::from_host`，或者对同一个类型喂一份宿主传来的 JSON）。**任何
//! 路径上都不打印 `api_key` 本身**——`Debug` 手写，只吐长度；日志/CLI 要看
//! key 状态就调 [`ProviderConfig::key_len`]，不是拿字符串。
//!
//! 查找顺序、字段形状与 `providers.example.toml` 顶部注释一致：
//! `$AGENT_PROVIDERS_CONFIG` → `./providers.toml` → `~/.config/agent/providers.toml`。
//! 这部分路径查找逻辑是 114d 特意没动的既有行为。
//!
//! 「已经解析好的配置」这个中间形态——`RootConfig`/`DefaultConfig`/
//! `ProviderConfig`/`ConfigError`/`default_provider`——定义在平台无关的
//! `crate::provider_config`，两个目标都编；这里只 `pub use` 重新导出（下面），
//! 本文件只剩「怎么从磁盘找到并读出这份配置」，不重复定义类型，toml 解析和
//! 宿主注入两条来源不会分叉成两套结构。

use std::path::{Path, PathBuf};

pub use crate::provider_config::{
    ConfigError, DefaultConfig, ProviderConfig, RootConfig, default_provider,
};

/// 按查找顺序找第一个存在的 `providers.toml` 并解析。
pub fn load() -> Result<RootConfig, ConfigError> {
    load_from(&candidates())
}

fn candidates() -> Vec<PathBuf> {
    [
        std::env::var("AGENT_PROVIDERS_CONFIG")
            .ok()
            .map(PathBuf::from),
        Some(PathBuf::from("providers.toml")),
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".config/agent/providers.toml")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn load_from(candidates: &[PathBuf]) -> Result<RootConfig, ConfigError> {
    for path in candidates {
        if path.is_file() {
            return parse_file(path);
        }
    }
    Err(ConfigError::NotFound {
        tried: candidates.to_vec(),
    })
}

fn parse_file(path: &Path) -> Result<RootConfig, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    toml::from_str(&text).map_err(|_| ConfigError::Parse {
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
