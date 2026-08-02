//! 读 providers.toml。**任何路径上都不打印 api_key。**

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Root {
    pub providers: BTreeMap<String, Provider>,
}

#[derive(Debug, Deserialize)]
pub struct Provider {
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    api_key_env: Option<String>,
    pub base_url: String,
    pub model: String,
}

impl Provider {
    /// 环境变量优先 —— 让部署环境能覆盖开发机上的默认值。
    pub fn resolve_key(&self) -> Option<String> {
        if let Some(var) = &self.api_key_env
            && let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return Some(v);
        }
        if self.api_key.is_empty() {
            None
        } else {
            Some(self.api_key.clone())
        }
    }

    /// 给人看的 key 状态，**不含 key 本身**。
    pub fn key_status(&self) -> String {
        match self.resolve_key() {
            Some(k) => format!("已配置（{} 字符）", k.len()),
            None => "未配置".to_string(),
        }
    }
}

/// 查找顺序与 providers.example.toml 里写的一致。
pub fn load() -> Result<Root, String> {
    let candidates: Vec<PathBuf> = [
        std::env::var("AGENT_PROVIDERS_CONFIG").ok().map(PathBuf::from),
        Some(PathBuf::from("../../providers.toml")),
        Some(PathBuf::from("providers.toml")),
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".config/agent/providers.toml")),
    ]
    .into_iter()
    .flatten()
    .collect();

    for path in &candidates {
        if path.is_file() {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let root: Root =
                toml::from_str(&text).map_err(|e| format!("{} 解析失败: {e}", path.display()))?;
            eprintln!("配置: {}", path.display());
            return Ok(root);
        }
    }
    Err(format!(
        "找不到 providers.toml，试过：\n{}",
        candidates
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}
