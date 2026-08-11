//! 宿主页面给的那份 provider 配置（114d 的调用侧）。
//!
//! 浏览器里没有 `providers.toml`（113 明确不移植 `config.rs`），所以配置从页面
//! 进来。**但类型不另起一套**：这里把页面给的 JSON 解成本模块的 [`HostConfig`]，
//! 再当场翻成 `agent_transport::ProviderConfig`——跟 native 那条 toml 路径汇到
//! 同一个类型、走同一个 `ExecutionBinding::from_provider_config`。两份配置结构
//! 分叉之后「native 能跑 wasm 不能」的排查会变成噩梦（111 决策原话）。
//!
//! # key 只从使用者来，且不进任何输出
//!
//! 111 的契约第 4 条：**每个用户一把自己的 key**，不得内置任何默认值，也不得
//! 写进任何受版本控制的文件。这个模块因此没有任何默认 `api_key`，
//! [`HostConfig`] **不派生 `Debug`**（派生的 `Debug` 会把字段原样打出来），
//! 解析失败也**不回显输入**——`serde_json::Error` 的 `invalid type` 分支会把
//! 字符串内容抄进错误消息里，而这里第一个可能出错的字符串字段就是 key 本身。

use std::sync::Arc;

use agent_providers::Provider;
use agent_providers::deepseek::DeepSeek;
use agent_providers::glm::Glm;
use agent_providers::kimi::Kimi;
use agent_transport::ProviderConfig;
use serde::Deserialize;

/// 页面传进来的一份配置。字段名与 `providers.toml` 的 `[providers.*]` 段一致，
/// 好让「照着 native 配置抄一份到页面上」这件事不需要翻译表。
#[derive(Deserialize)]
pub(crate) struct HostConfig {
    /// 哪家 adapter：`deepseek` / `kimi` / `glm`。
    pub(crate) provider: String,
    pub(crate) base_url: String,
    pub(crate) model: String,
    /// 使用者自己的 key。见模块文档——它只活在内存里。
    api_key: String,
}

impl HostConfig {
    /// 解析页面给的 JSON。**错误消息里不带任何输入内容**，见模块文档。
    pub(crate) fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str::<HostConfig>(json).map_err(|_| {
            "配置 JSON 解析失败：需要 {\"provider\":\"deepseek|kimi|glm\", \
             \"base_url\":\"…\", \"model\":\"…\", \"api_key\":\"…\"} 四个字符串字段"
                .to_string()
        })
    }

    /// 翻成 114d 的那个共用类型。`api_key` 在这一步交出所有权的副本——之后这个
    /// 结构体自己那份不再被任何人读。
    pub(crate) fn provider_config(&self) -> ProviderConfig {
        ProviderConfig::from_host(
            self.base_url.clone(),
            self.model.clone(),
            self.api_key.clone(),
        )
    }

    /// provider 名字 → 具体 adapter。跟 `agent_cli::provider::build_provider`
    /// 是同一张表；这条 `match` 写在宿主装配层是合法的（红线 12 只约束
    /// `agent-core`/`agent-store`）。
    pub(crate) fn adapter(&self) -> Result<Arc<dyn Provider>, String> {
        match self.provider.as_str() {
            "deepseek" => Ok(Arc::new(DeepSeek)),
            "kimi" => Ok(Arc::new(Kimi)),
            "glm" => Ok(Arc::new(Glm)),
            other => Err(format!(
                "\"{other}\" 没有对应的 adapter。可选：deepseek / kimi / glm"
            )),
        }
    }

    /// 给人看的 key 长度——**不是 key 本身**。页面横幅只许打印这个，跟
    /// `ProviderConfig::key_len` 同一条规矩。
    pub(crate) fn key_len(&self) -> usize {
        self.api_key.len()
    }
}
