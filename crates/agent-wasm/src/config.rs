//! 页面在**建宿主那一刻**交给宿主、此后不再改变的外部输入：连哪个模型
//! （114d 的调用侧），以及这个宿主有哪些页面声明的 capabilities（直接工具和
//! skill）。
//!
//! 浏览器里没有 `providers.toml`（113 明确不移植 `config.rs`），所以配置从页面
//! 进来。**但类型不另起一套**：这里把页面给的 JSON 解成本模块的 [`HostConfig`]，
//! 再当场翻成 `agent_transport::ProviderConfig`——跟 native 那条 toml 路径汇到
//! 同一个类型、走同一个 `ExecutionBinding::from_provider_config`。两份配置结构
//! 分叉之后「native 能跑 wasm 不能」的排查会变成噩梦（111 决策原话）。
//!
//! # 能力声明为什么也落在这个类型上（122）
//!
//! 两样东西共享同一条性质，而这条性质正是 122 最要紧的一条：**建宿主时定死、
//! 会话期间不可变**。会话中途换工具表 = 前缀缓存全断，所以它必须跟 provider 配置
//! 一样，只在构造 [`AgentHost`](crate::AgentHost) 的那一次被给定
//! （[`HostConfig::with_declared_capabilities`] 是消费 `self` 的 builder，而
//! `Inner::config` 之后再没有任何 `&mut` 的取法——「定死」因此是结构性的，
//! 不靠运行时闸）。
//!
//! 顺带也是它落在这里的现实理由：[`crate::assemble::open`] 每开一次会话就要现造
//! 一张工具表，而它拿到的宿主侧输入只有这一份 [`HostConfig`]。声明**怎么解析、
//! 怎么校验**不在这里，在 `agent_runtime::host_tools_from_declaration`；
//! **怎么装进表**在 [`crate::tools`]。这个类型只是那份料的载体。
//! # key 只从使用者来，且不进任何输出
//!
//! 111 的契约第 4 条：**每个用户一把自己的 key**，不得内置任何默认值，也不得
//! 写进任何受版本控制的文件。这个模块因此没有任何默认 `api_key`，
//! [`HostConfig`] **不派生 `Debug`**（派生的 `Debug` 会把字段原样打出来），
//! 解析失败也**不回显输入**——`serde_json::Error` 的 `invalid type` 分支会把
//! 字符串内容抄进错误消息里，而这里第一个可能出错的字符串字段就是 key 本身。

use std::sync::Arc;

use agent_core::{HostSkill, Reversibility, ToolSpec};
use agent_providers::Provider;
use agent_providers::deepseek::DeepSeek;
use agent_providers::glm::Glm;
use agent_providers::kimi::Kimi;
use agent_transport::ProviderConfig;
use serde::Deserialize;

/// 页面传进来的一份配置。前四个字段名与 `providers.toml` 的 `[providers.*]` 段
/// 一致，好让「照着 native 配置抄一份到页面上」这件事不需要翻译表。
#[derive(Deserialize)]
pub(crate) struct HostConfig {
    /// 哪家 adapter：`deepseek` / `kimi` / `glm`。
    pub(crate) provider: String,
    pub(crate) base_url: String,
    pub(crate) model: String,
    /// 使用者自己的 key。见模块文档——它只活在内存里。
    api_key: String,
    /// 这家的上下文窗口（token）。**不填 = M12 的压缩在浏览器里永远不开火。**
    ///
    /// 压缩的触发判据是「上一轮实测 `prompt` / `context_window` 是否过阈值」
    /// （`agent-core` 的 096 决策），窗口是 `None` 时那个比较无从做起，于是整套
    /// 五档分级结构上不可达——**功能全在、状态全对、一次都不会触发**。
    ///
    /// 这个坑在 M12 收尾时于 native 侧踩过一次（当时五个宿主全是 `None`，
    /// 见 issue 110），浏览器宿主是第六个：114d 把配置搬过来时只搬了前四个字段。
    /// 名字与 `providers.toml` 的 `[providers.*].context_window` 一致，
    /// 「照着 native 配置抄一份到页面上」仍然不需要翻译表。
    #[serde(default)]
    context_window: Option<u32>,
    /// 122：页面声明的那一段工具，**已经解析校验完**的料。
    ///
    /// `#[serde(skip)]`：它不来自这份 provider 配置 JSON，而是构造 `AgentHost` 时
    /// 另一个入参（页面把声明写成一个模块级常量原样传进来，见 [`crate::tools`]
    /// 模块文档「红线 11」）。两样东西同住一个类型的理由见模块文档。
    #[serde(skip)]
    declared_tools: Vec<(ToolSpec, Reversibility)>,
    /// 同一份宿主声明里的 skill 正文。它们与工具一样是新会话的输入，但随后会被
    /// journaled 到各自的会话；恢复时绝不以这份当前配置覆盖历史。
    #[serde(skip)]
    declared_skills: Vec<HostSkill>,
    /// 决策 31（157）：页面声明的开局块 `(name, text)`。同一条性质：新会话的
    /// 输入、journaled 落店、恢复只认 journal。
    #[serde(skip)]
    declared_prefix: Vec<(Arc<str>, Arc<str>)>,
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

    /// 把页面声明的 tool / skill 装上。**消费 `self`**，所以它只可能在建
    /// `AgentHost` 的那一次被调用——之后 `Inner::config` 再没有 `&mut` 的取法，
    /// 「第一次 `send()` 之前定死」因此是结构性成立的，不是一条运行时约定。
    ///
    /// 入参是 [`crate::capabilities::parse`] 的产出（已经解析、校验、撞名也挡过）。
    pub(crate) fn with_declared_capabilities(
        mut self,
        tools: Vec<(ToolSpec, Reversibility)>,
        skills: Vec<HostSkill>,
        prefix: Vec<(Arc<str>, Arc<str>)>,
    ) -> Self {
        self.declared_tools = tools;
        self.declared_skills = skills;
        self.declared_prefix = prefix;
        self
    }

    /// 页面声明的那一段，交给 [`crate::tools::browser_tool_table`] 装表。
    pub(crate) fn declared_tools(&self) -> &[(ToolSpec, Reversibility)] {
        &self.declared_tools
    }

    /// 页面声明的 skill，建新会话时注册并写入 journal。
    pub(crate) fn declared_skills(&self) -> &[HostSkill] {
        &self.declared_skills
    }

    /// 页面声明的开局块，建新会话时装表并写入 journal。
    pub(crate) fn declared_prefix(&self) -> &[(Arc<str>, Arc<str>)] {
        &self.declared_prefix
    }

    /// 没有能力时不写任何声明 entry，也不人为推进 turn 边界。
    pub(crate) fn has_declared_capabilities(&self) -> bool {
        !self.declared_tools.is_empty()
            || !self.declared_skills.is_empty()
            || !self.declared_prefix.is_empty()
    }

    /// 翻成 114d 的那个共用类型。`api_key` 在这一步交出所有权的副本——之后这个
    /// 结构体自己那份不再被任何人读。
    pub(crate) fn provider_config(&self) -> ProviderConfig {
        let mut config = ProviderConfig::from_host(
            self.base_url.clone(),
            self.model.clone(),
            self.api_key.clone(),
        );
        // 不填就保持 `None`——跟 native 一样，「没配窗口 = 不压缩」，
        // 而不是替使用者猜一个默认窗口。猜错的后果是压缩过早或过晚，
        // 两头都只在账单和上下文丢失上浮出来。
        config.context_window = self.context_window;
        config
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
