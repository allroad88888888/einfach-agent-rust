//! [`OpenSpec`]：开一个 session 要的全部东西。
//!
//! **全部字段 `Send + 'static`**——这份配置本身要整体 `move` 进 actor 的
//! `std::thread::spawn` 闭包（`crate::actor` 模块文档），`Session`/`RunnerCtx`
//! 本身（`!Send`，含 `Rc<RefCell<_>>`）绝不允许出现在这个类型里，只能在线程
//! 内部用这份配置现造。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::{AgentLimits, HostSkill, Reversibility, SystemChunk, ToolSpec};
use agent_providers::Provider;
use agent_tools::VisionRuntime;
use agent_transport::Client;

use super::SessionId;

/// 开一个 session 要的全部东西。
///
/// `provider` 直接收 `Arc<dyn Provider>`，不是名字字符串——production 代码用
/// [`crate::provider_dispatch::resolve_provider`] 按名字查表拿到它（那张表照抄
/// `agent_cli::provider::build_provider` 的手法，issue 030 原文点名的「配置
/// 驱动三家分发」），测试可以直接喂一个自造的假 `Provider`（比如崩溃隔离测试
/// 要用到的那种），两者共用同一条 `OpenSpec` 构造路径,不必为「生产」和「测试」
/// 分叉出两个 `open` 入口。
pub struct OpenSpec {
    pub id: SessionId,
    /// `Some` → `Jsonl`（真文件，跟 CLI `--session <path>` 同款语义：有路径就
    /// 落盘，没有就是临时会话）；`None` → `Memory`（actor 关闭/进程退出即丢）。
    pub store_path: Option<PathBuf>,
    pub provider: Arc<dyn Provider>,
    pub endpoint: String,
    pub api_key: String,
    pub model: Arc<str>,
    /// 上下文窗口大小，单位 token——原样来自 [`crate::http::config::
    /// SessionTemplate::context_window`]。`crate::actor::body` 拿它建这个会话的
    /// `SessionConfig::context_window`，压缩触发（096/108）在那之后拿它做纯算术。
    pub context_window: Option<u32>,
    pub tools: ToolTableSpec,
    /// 内置工具的路径监狱根——跟 `agent-cli` 一样锁在这棵目录树之内
    /// （`ToolExecutor::new` 的既有语义，这里不改）。
    pub tools_root: PathBuf,
    pub system: Vec<SystemChunk>,
    pub client: Arc<Client>,
    /// `None` → `agent_core::DEFAULT_HISTORY_CAP`。
    pub history_cap: Option<usize>,
    /// `None` → `agent_runtime` 的默认快照节奏（每 10 个 turn）。
    pub snapshot_every: Option<u64>,
    /// `None` → `agent_runtime` 的默认 provider 超时（120s）。
    pub provider_timeout: Option<Duration>,
    /// `None` → `agent_runtime` 的默认远端工具超时（060，10 分钟）：`web:`/`desk:`
    /// 工具派给宿主之后，等 `POST /tool_result` 等多久算等不到了。
    pub remote_tool_timeout: Option<Duration>,
    /// 宿主这一次建会话时声明的工具（062，`POST /sessions` 的 `capabilities.tools`
    /// 经 `crate::http::capabilities::host_tools` 翻译而来）。
    ///
    /// **这是本 issue 作用域那一条的落点**：`OpenSpec` 本来就是**每个 session 一
    /// 份**的东西（`SessionRegistry::open` 收的就是它），注入的工具因此只进这一个
    /// 会话在 actor 线程里现造的那张 `ToolTable`
    /// （`crate::actor::body` 调 `ToolTable::with_host_tools`）——不进全局表、别的
    /// chatid 看不见、会话结束就没了（docs/HOST-CAPABILITIES.md §二）。空 = 这次
    /// 没有声明，工具表跟 062 之前逐字节相同。
    ///
    /// 可逆性跟着走一份而不进 `ToolSpec`：后者进 prompt，加字段要重算红线 11 的账
    /// （§五，形状照 `ToolTable::with_mcp` 的既有先例）。
    ///
    /// **这个字段是「一份声明」，不是「一次 HTTP 请求」**：类型是纯 `agent_core`
    /// 数据，跟 `POST /sessions` 的请求体没有任何耦合。073 会把声明挪进 store
    /// （建会话时 journaled、恢复时回放，宿主不必重连时重报一遍），那时往这里填的
    /// 人从「路由层翻译请求体」换成「从回放出来的会话状态里取」——**装配这一侧
    /// （这个字段往下的每一行）一行都不用改**。
    pub host_tools: Vec<(ToolSpec, Reversibility)>,
    /// 宿主这一次建会话时声明的 skill（064，`POST /sessions` 的 `capabilities.skills`
    /// 经 `crate::http::capabilities::host_skills` 翻译而来）。
    ///
    /// 跟 [`host_tools`](OpenSpec::host_tools) 一档、同一个作用域论证：`OpenSpec` 每
    /// 个 session 一份，声明因此只进这一个会话在 actor 线程里现造的那个
    /// `SkillRegistry`（`crate::actor::body` 调 `SkillRegistry::from_host_skills`），
    /// 别的 chatid 看不见。空 = 这次没有声明 skill，registry 为空 → 工具表**不接**
    /// `.with_skills(..)`、常驻索引是空文本，会话跟 064 之前逐字节相同。
    ///
    /// 类型是纯 `agent_core` 数据（`HostSkill` 就是它落进 `Slot::HostSkills` 再回放
    /// 出来的同一个形状），跟 `POST /sessions` 的请求体没有任何耦合——「新建」看这次
    /// 请求、「恢复」看回放，往这里填的人不同，装配那一侧一行都不用改。
    pub host_skills: Vec<HostSkill>,
    /// 宿主这一次建会话时**关掉**的内置工具名（076，`POST /sessions` 的
    /// `capabilities.disable_builtin` 经 `crate::http::capabilities::disabled_builtins`
    /// 翻译而来）。
    ///
    /// 跟上面两个字段同一个 per-session 作用域论证，但**方向相反**：那两个是宿主
    /// 往这个会话里加，这一个是从部署方给的那张表里减。列出来的工具**连名字带描述
    /// 都不进 prompt**（`ToolTable::without_builtins`），模型压根不知道有它。
    ///
    /// 名字必须在 [`tools`](OpenSpec::tools) 这一档装配出来的表里——**只能减不能
    /// 加**，那道闸在 HTTP 路由上（`capabilities::check_builtin_switch`，400 且点名）。
    /// 到了这里已经校验过；装配那一侧对认不出的名字静默跳过，因为它每次开会话都跑、
    /// 那时作者早不在场了。
    ///
    /// 空 = 这次什么都没关，工具表跟 076 之前逐字节相同。
    pub disable_builtin: Vec<Arc<str>>,
    /// 宿主这一次建会话时声明的开局块（156，M17，决策 31，`POST /sessions` 的
    /// `capabilities.prefix` 经 `crate::http::capabilities::host_prefix` 翻译
    /// 而来）。跟 [`host_tools`](OpenSpec::host_tools) 同一档「加法」、同一个
    /// per-session 作用域论证：装配那一侧（`crate::actor::capabilities::assemble`）
    /// 在 per-session 装配链**尾部**（`with_host_tools` 之后）接
    /// `ToolTable::with_host_prefix(&host_prefix)`，每对合成一条「执行体 = 返回
    /// 常量文本」的 `SessionStart` timed 工具（155）。
    ///
    /// **这个字段是「一份声明」，不是「一次 HTTP 请求」**：类型是纯 `agent_core`
    /// 数据，跟请求体没有任何耦合——「新建」看这次请求（`spec.host_prefix`），
    /// 「恢复」看回放出的会话状态（`Session::host_prefix()`，154 落的
    /// `Slot::HostPrefix`），装配那一侧一行都不用改（跟 073 时 `host_tools` 的
    /// 论证一样）。空 = 这次没有声明，工具表/前缀块跟 156 之前逐字节相同。
    pub host_prefix: Vec<(Arc<str>, Arc<str>)>,
    /// s5 `srv:vision/inspect` 的运行时。`Some` → actor 线程现造 `ToolExecutor`
    /// 时注入（`ToolExecutor::with_vision`），并把工具追加进工具表
    /// （`ToolTable::with_vision_inspect`）；`None` → 工具不声明。API key 只随
    /// 这份 per-session 配置在 actor 线程内部短暂存在，绝不落盘。
    pub vision: Option<VisionRuntime>,
}

/// 工具表的五档，跟 `agent_runtime::ToolTable::builtin`/`standard_local`/`standard`/
/// `with_shell`/`with_spawn` 一一对应——`OpenSpec` 不直接收一个建好的 `ToolTable`：那个类型
/// 不是 `Clone`，而 `spawn` 失败重试、未来配置热更新一类场景要求这份配置本身
/// 可以廉价复制。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolTableSpec {
    /// 013 的内置只读集：`srv:fs/read`、`srv:fs/list`。
    Builtin,
    /// web-agent 兼容的本地标准工具：只读文件、受版本保护的可撤回文本事务、
    /// 测试/lint 发现与静态命令；不包含远端交互、计划、子 agent 或 MCP。
    StandardLocal,
    /// web-agent 标准工具集：本地标准工具外加 `ask_user_question`、
    /// `browser_action` 与 `save_file`。这三项只经 Web 回传通道执行，不注册计划、
    /// 子 agent 或 MCP。
    Standard,
    /// 内置只读集 + `srv:shell/exec`（020/027 开闸）。
    WithShell,
    /// 034 开闸：内置只读集 + `srv:shell/exec` + `srv:agent/spawn` +
    /// `srv:agent/status`（051）+ `srv:agent/collect`（053，054 接上）
    /// （029 给 runtime 的多 agent 能力，经这一档接满到 HTTP 协议面）。
    /// `spawn_limits` 走 server 配置（`SessionTemplate::tools`——`ServerConfig`
    /// 的一部分），默认 = [`AgentLimits::default`]（029 的默认）；`examples/
    /// serve.rs` 用这一档开满档。**这里的数字必须跟 `Session` 手上那份是同一组
    /// 数**——`agent_runtime::ToolTable::with_spawn` 文档记了这个耦合，工具描述
    /// 里告诉模型的数字得跟真正拦人的两道闸一致。`crate::actor::body` 在新建会话
    /// 时调 [`Session::set_agent_limits`](agent_core::Session::set_agent_limits)
    /// 对齐这组数（走 [`spawn_limits`](ToolTableSpec::spawn_limits) 这个读口），
    /// 不是留给调用方自己记得两边传同一个值。
    Full { spawn_limits: AgentLimits },
}

impl ToolTableSpec {
    pub(crate) fn build(self) -> agent_runtime::ToolTable {
        match self {
            ToolTableSpec::Builtin => agent_runtime::ToolTable::builtin(),
            ToolTableSpec::StandardLocal => agent_runtime::ToolTable::standard_local(),
            ToolTableSpec::Standard => agent_runtime::ToolTable::standard(),
            ToolTableSpec::WithShell => agent_runtime::ToolTable::with_shell(),
            // `with_status`（051）/ `with_collect`（053）跟 spawn 同一档：这一档的
            // 整个意思就是「开子 agent」，status 是模型观测自己子树的那一半、
            // collect 是领后台子结果的那一半（docs/ORCHESTRATION.md §三）。
            // **开了 `background` 却不开 `collect` 是陷阱**——模型能发后台子，却
            // 领不回结果，只能等轮末被当孤儿拆掉（`agent_runtime::orphan`）。
            // 054 之前这一档漏了 collect，CLI（`agent-cli::main`）那边一直是全的。
            //
            // M20（决策 35）追加 `send`（206）/`self`（208）/`notes`（209）：这一档的意思是
            // 「开子 agent」，而横读全开之后兄弟之间说得上话才是这一波的行为
            // 核心。**声明是唯一的开关**——截获注册跟着 `declares()` 走
            // （`agent_runtime::builtin_intercepts`），这一行不加，模型连这两个
            // 工具存在都不知道，206/208 就是死代码。
            ToolTableSpec::Full { spawn_limits } => agent_runtime::ToolTable::with_shell()
                .with_spawn(spawn_limits)
                .with_status()
                .with_collect()
                .with_send()
                .with_self()
                .with_notes(),
        }
    }

    /// `Full` 档带的 spawn 上限，供 `crate::actor::body` 在新建（非恢复）
    /// `Session` 时调 `Session::set_agent_limits` 对齐——两边不能只改工具表这
    /// 一侧的数字（本类型文档），这个读口是让「对齐」从一条纪律变成一次函数
    /// 调用。
    pub(crate) fn spawn_limits(self) -> Option<AgentLimits> {
        match self {
            ToolTableSpec::Full { spawn_limits } => Some(spawn_limits),
            ToolTableSpec::Builtin
            | ToolTableSpec::StandardLocal
            | ToolTableSpec::Standard
            | ToolTableSpec::WithShell => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 054：`Full` 这一档的意思就是「开子 agent」，编排三件套必须**同时**在
    /// （ORCHESTRATION §三）。**开了 `background` 却不开 `collect` 是陷阱**：
    /// 模型能发后台子却领不回结果，只能眼看它在轮末被当孤儿拆掉。这条断言就是
    /// 那个陷阱的看门狗——照 051 给 `with_status` 加的那条同款。
    #[test]
    fn the_full_table_declares_spawn_status_and_collect_together() {
        let table = ToolTableSpec::Full {
            spawn_limits: AgentLimits::default(),
        }
        .build();
        assert!(table.declares(agent_runtime::SPAWN_TOOL), "Full 该有 spawn");
        assert!(
            table.declares(agent_runtime::STATUS_TOOL),
            "Full 该有 status"
        );
        assert!(
            table.declares(agent_runtime::COLLECT_TOOL),
            "Full 该有 collect"
        );
    }

    /// 反面：别的档一个都不该有——`Full` 是唯一开子 agent 的那一档，
    /// 上面那条断言不能靠「反正每一档都有」蒙混过去。
    #[test]
    fn no_other_tier_declares_the_orchestration_trio() {
        for spec in [
            ToolTableSpec::Builtin,
            ToolTableSpec::StandardLocal,
            ToolTableSpec::Standard,
            ToolTableSpec::WithShell,
        ] {
            let table = spec.build();
            for tool in [
                agent_runtime::SPAWN_TOOL,
                agent_runtime::STATUS_TOOL,
                agent_runtime::COLLECT_TOOL,
            ] {
                assert!(!table.declares(tool), "{spec:?} 不该声明 {tool}");
            }
        }
    }

    /// 161：`Full` 这一档配的上限，必须真的走进**模型看得见的那份描述**。
    ///
    /// 这是「两侧数字是同一组」那条耦合里最容易悄悄断掉的一半：`Session` 那侧
    /// 由 `actor::body` 的 `set_agent_limits`/`recover` 保证（160 已钉），描述这侧
    /// 全靠 `build()` 把 `spawn_limits` 递给 `ToolTable::with_spawn`。递丢了不会
    /// 报错——模型只会看到默认档的 8，按 8 规划，然后撞上运维配的那道更紧的闸。
    #[test]
    fn the_configured_limits_reach_the_description_the_model_reads() {
        let table = ToolTableSpec::Full {
            spawn_limits: AgentLimits {
                max_depth: 2,
                max_children: 3,
                ..AgentLimits::default()
            },
        }
        .build();

        let spec = table
            .specs()
            .iter()
            .find(|s| &*s.name == agent_runtime::SPAWN_TOOL)
            .expect("Full 档该有 spawn");
        let description = &*spec.description;

        assert!(
            description.contains("最多 2") && description.contains("3 个"),
            "配的 2/3 该出现在描述里，实际：{description}"
        );
        assert!(
            !description.contains("8 个"),
            "不该还留着默认档的 8——那说明 spawn_limits 没递到 with_spawn：{description}"
        );
    }
}
