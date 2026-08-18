//! 工具表：宿主侧持有的「工具在哪跑、可逆性怎么定」（002 合并记录：core 没有
//! 这份数据，`ExecuteTool` 快照的构造点在宿主/command 层——M1 这里就是那个
//! 宿主）。
//!
//! `agent_core::ToolSpec` 只有喂给模型的三个字段（name/description/schema），
//! 没有 `Location`/`Reversibility`——那两个维度是 router/undo 用的，`agent-tools`
//! 也没暴露它们（013 的 `ToolExecutor` 只按全名分发，不声明位置/可逆性）。这张表
//! 补的就是这一格。
//!
//! # 这个文件只留「表本身」，专门的事各在一个文件里
//!
//! | 文件 | 那一件事 |
//! |---|---|
//! | 本文件 | 五档装配 + `snapshot` 的三级判定 + `push_spec` 判重（075） |
//! | [`agent_family`]（`tool_table_agent.rs`，208 拆出） | **子 agent 一族这件事**：`spawn`/`status`/`collect`/`send`/`self` 五档授权、它们的陷阱组合 |
//! | [`names`]（`tool_table_names.rs`，076 拆出） | **名字规则**：全名怎么机械推出 `Location`/`Reversibility` |
//! | [`host`]（`tool_table_host.rs`，062） | **宿主注入这件事**：那张可逆性映射怎么进表、为什么排序、为什么另挂一张表 |
//! | [`host_prefix`]（`tool_table_host_prefix.rs`，155/决策 31） | **宿主声明开局块这件事**：`(name, text)` 对怎么合成 `SessionStart` timed 条目 |
//! | [`skill_tools`]（`tool_table_skill.rs`，039/139） | **skill 这件事**：registry 归表拥有，`read`/`index` 怎么装配 |
//! | [`disable`]（`tool_table_disable.rs`，076） | **关掉内置这件事**：会话建立时把部署方给的某几件整条剔掉（唯一往回减的一件） |
//! | [`timed`]（`tool_table_timed.rs`，133） | **调用时机这件事**：timed 工具独立区，`specs`/`declares`/`snapshot` 一个字节看不见它 |
//! | [`standard`]（`tool_table_standard.rs`，148 拆出） | **web-agent 标准集这件事**：裸名一族的两档构造 |
//! | [`extension`]（`tool_table_extension.rs`，148） | **装一个扩展包这件事**：两阶段（表半边/ctx 半边）怎么拆、怎么防「只装一半」 |
//!
//! 拆的判据是「说得清它是干嘛的、且不含『和』」（红线 9）：注入、skill、关掉内置、
//! 名字规则各自都有一整套自己的理由要写，混在这里会让「工具表是什么」这句话说不完。

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_core::{Reversibility, ToolCallRequest, ToolSpec};
use serde_json::Value;

use crate::skill::SkillRegistry;

use names::{location_of, reversibility_of};
pub use timed::{CallTiming, TimedRun, TimedTool};

/// 会话期间不变的工具表：喂模型的声明 + 判 `location`/`reversibility` 用的
/// 名字规则 + （039）宿主装载的 skill registry。
pub struct ToolTable {
    specs: Vec<ToolSpec>,
    /// 这张表拥有的 skill registry（039）。为什么它住在表里而不是 `RunnerCtx` 的
    /// 单独字段，全在 [`skill_tools`]（`with_skills`）。空 = 这个会话没开 skill。
    registry: SkillRegistry,
    /// `mcp:<server>/<tool>` → 它的可逆性（042 握手时经 [`ToolTable::with_mcp`] 装进
    /// 来，041 从 `readOnlyHint` 翻译）。`snapshot` 撞 `mcp:` 前缀查这份映射，**查不到
    /// 落保守 `Irreversible`**——MCP 可逆性是 per-tool 元数据，不能从名字推
    /// （docs/MCP.md §「可逆性不能再从名字推」）。有序容器（红线 11 精神；也不进
    /// prompt，纯查表）。空表 = 没接 MCP。
    mcp_reversibility: BTreeMap<Arc<str>, Reversibility>,
    /// 宿主建会话时注入的工具（062）→ 它的可逆性。装配、排序与「为什么另挂一张表
    /// 而不是给 `ToolSpec` 加字段」全在 [`host`]（`with_host_tools`）。空表 = 这个
    /// 会话没有注入。
    ///
    /// 148 曾让扩展包声明的可逆性也进这张表；**201 撤掉了那一路**——决策 199 之后
    /// 扩展不再在注册时声明可逆性（依据是执行体返回的 `Aftermath`），`ext:` 工具
    /// 因此落到第三级名字规则的保守兜底，见 [`extension`] 模块文档。
    host_reversibility: BTreeMap<Arc<str>, Reversibility>,
    /// timed 工具独立区（133）。**不进 `specs`，`declares()`/`snapshot()` 看不见
    /// 它**——模型面的表只有一个答案，这是 076 disable 判据的延续；`with_timed`/
    /// `timed` 全在 [`timed`] 模块。空 Vec = 这个会话没有 timed 工具（v1 全部会话
    /// 都是，装驱动是 135/136 的事，本条只加维度）。
    timed_tools: Vec<TimedTool>,
}

#[path = "tool_table_names.rs"]
mod names;

#[path = "tool_table_agent.rs"]
mod agent_family;

#[path = "tool_table_host.rs"]
mod host;

#[path = "tool_table_host_prefix.rs"]
mod host_prefix;

#[path = "tool_table_skill.rs"]
mod skill_tools;

#[path = "tool_table_disable.rs"]
mod disable;

#[path = "tool_table_timed.rs"]
mod timed;

#[path = "tool_table_standard.rs"]
mod standard;

#[path = "tool_table_extension.rs"]
pub(crate) mod extension;

impl ToolTable {
    /// 从一组 specs 造一张表：空 skill registry、空 MCP 映射、空注入映射。四个内置
    /// 构造器共用它，免得每加一个字段就四处补一遍。
    fn from_specs(specs: Vec<ToolSpec>) -> Self {
        ToolTable {
            specs,
            registry: SkillRegistry::empty(),
            mcp_reversibility: BTreeMap::new(),
            host_reversibility: BTreeMap::new(),
            timed_tools: Vec::new(),
        }
    }

    /// 不向模型声明任何工具的隔离档。公开改写等不可信输出路径必须从空表开始，
    /// 不能先装部署期工具再靠名称黑名单回减。
    pub fn empty() -> Self {
        Self::from_specs(Vec::new())
    }

    /// 075：`with_*` 系列 `push` 进 `specs` 的唯一入口。**名字已经在表里 → 整条丢弃**
    /// （不 `push`），返回 `false` 交给调用方——`with_mcp`/`with_host_tools` 靠这个
    /// 返回值判断要不要顺带 `insert` 对应的可逆性映射，不然会造出「表里没有对应 spec
    /// 的映射项」这种隐式耦合（069 §拍板 D 第二条理由）。丢的是**后来的**那一条：
    /// 工具表在 prompt 最前面（红线 11），既有前缀一个字节不能动。
    ///
    /// **生产不 panic**（069 §拍板 D 被否①明确否决过硬失败）：`with_mcp` 收的是
    /// 第三方 MCP server 的 `tools/list` 回包，`with_host_tools` 收的是客户端建会话
    /// 的请求体——外部数据写错了就把宿主进程打死不可接受，两条路各自也已经有更早、
    /// 更该报错的裁判点（074 的 `list_tools` 去重 / 061 的 400）。`debug_assert!`
    /// 只在 debug 构建炸，点得出撞的是哪个名字；release 静默丢弃。
    fn push_spec(&mut self, spec: ToolSpec) -> bool {
        // 133：也要查 timed 区——`with_timed` 可能在装配链的更早一步已经注册了
        // 同名的时机工具，链式顺序不是这个函数能控制的，所以查重必须双向
        // （见 [`timed`] 模块文档「撞名：双向查」）。
        if self.declares(&spec.name) || self.declares_timed(&spec.name) {
            debug_assert!(
                false,
                "ToolTable 已经有工具 `{}` 了（specs 区或 timed 区），同名的后来这一条整条丢弃（specs 不 push，可逆性也不 insert）",
                spec.name
            );
            return false;
        }
        self.specs.push(spec);
        true
    }

    /// 013 的内置工具集：`srv:fs/read`、`srv:fs/list`，服务端本地、纯读。
    pub fn builtin() -> Self {
        Self::from_specs(agent_tools::builtin_specs())
    }

    /// 027 开闸：内置只读集 + `srv:shell/exec`（020 声明、`agent-tools` 的
    /// `ToolExecutor` 早已支持分发，唯独没接进任何工具表——020 的范围裁决要求
    /// 「屏障 UI 齐了才许打开」，027 就是那个集成 issue）。追加在末尾而不是
    /// 插进 `builtin()` 内部：`builtin_specs()` 的顺序是 013 钉死的既有契约，
    /// 这里只加不改。
    pub fn with_shell() -> Self {
        let mut specs = agent_tools::builtin_specs();
        specs.push(agent_tools::shell_spec());
        Self::from_specs(specs)
    }

    /// s5 开闸：追加 `srv:vision/inspect`（写死 Kimi 3 的识图工具）。它调
    /// 第三方 API（计费、网络 IO），不在已知 pure 名单里，按名字规则保守落
    /// `Reversibility::Irreversible`——undo 不重放。
    ///
    /// **只有配了 vision 的宿主才调它**：`ToolExecutor` 注入了
    /// `VisionRuntime`（server：kimi 段 + 上传目录；CLI：kimi 段 + 本地 root）
    /// 才声明这个工具，不配置就不声明，模型根本不知道有它，也就不会把调用
    /// 发到没有实现的分发分支上。
    ///
    /// 追加在末尾（红线 11：既有顺序是契约，只加不改）。
    pub fn with_vision_inspect(mut self) -> Self {
        self.push_spec(agent_tools::vision_inspect_spec());
        self
    }

    /// 043 装载：把一批 MCP 工具（041 翻译产出的 `(ToolSpec, Reversibility)`，042
    /// 握手时 `McpClient::list_tools` 拿到）追加进表，并把每个工具的可逆性记进
    /// `mcp:` 映射。`snapshot("mcp:...")` 从此查这份映射（见 `snapshot`）。
    ///
    /// 追加在末尾（红线 11：工具表在 prompt 最前面，顺序是既有契约，只加不改）。
    /// 名字里已带 server id 消歧（`mcp:<server>/<tool>`），所以两个 server 的同名
    /// 工具在这份映射里也不会撞键。
    pub fn with_mcp(mut self, tools: Vec<(ToolSpec, Reversibility)>) -> Self {
        for (spec, reversibility) in tools {
            let name = Arc::clone(&spec.name);
            if self.push_spec(spec) {
                self.mcp_reversibility.insert(name, reversibility);
            }
        }
        self
    }

    /// 喂给 `Ingredients::tools` 的那张表，顺序原样保留（红线 11）。
    pub fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }

    /// 这张表里有这个工具吗。
    ///
    /// 唯一的用处是 spawn 的截获闸（`crate::dispatch`）：**宿主没声明就不截获**，
    /// 模型凭空猜出来的 `srv:agent/spawn` 跟别的不存在的工具走同一条路
    /// （`unknown_tool`），而不是在一个没打算开子 agent 的宿主上凭空长出一棵树。
    pub fn declares(&self, tool: &str) -> bool {
        self.specs.iter().any(|spec| &*spec.name == tool)
    }

    /// 按全名 + 这次调用的 `input` 构造一次调用的「发起时快照」。
    ///
    /// `location` 从名字前缀机械解析（docs/TOOLS.md 的命名约定：`srv:`/`web:`/
    /// `desk:`）；`reversibility` **拿不准就 `Irreversible`**
    /// （value/tool.rs 的判据：判错代价不对称，保守值必须是默认值）——M1 的
    /// 两个内置工具都是已知的纯读，显式列出，其余一律走保守默认，不臆造
    /// `Pure`。
    pub fn snapshot(&self, tool: &str, input: Arc<Value>) -> ToolCallRequest {
        // **三张表的优先级写死在这里**（062）：宿主注入的映射 → MCP 映射 → 名字规则。
        // 第一级按**表**查、不按前缀查，理由（以及 062 之前那个前缀门会怎么静默咬人）
        // 见 [`host`] 模块文档。MCP 那一级仍然按前缀进：它的可逆性是 per-tool 元数据
        // （来自 server 的 `readOnlyHint`，042 翻译进映射），**不从名字推**——
        // `mcp:everything/echo` 和 `mcp:everything/sendEmail` 同前缀，一个 readOnly
        // 一个不是。查不到落保守 `Irreversible`（把数据事故开关交给第三方的代价不
        // 对称，docs/MCP.md）。
        let reversibility = match self.host_reversibility.get(tool).copied() {
            Some(declared) => declared,
            None if tool.starts_with("mcp:") => self
                .mcp_reversibility
                .get(tool)
                .copied()
                .unwrap_or(Reversibility::Irreversible),
            None => reversibility_of(tool),
        };
        ToolCallRequest {
            tool: Arc::from(tool),
            input,
            location: location_of(tool),
            reversibility,
        }
    }
}

#[cfg(test)]
#[path = "tool_table_tests.rs"]
mod tests;
