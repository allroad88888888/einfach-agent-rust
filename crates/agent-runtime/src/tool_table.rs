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
//! | [`names`]（`tool_table_names.rs`，076 拆出） | **名字规则**：全名怎么机械推出 `Location`/`Reversibility` |
//! | [`host`]（`tool_table_host.rs`，062） | **宿主注入这件事**：那张可逆性映射怎么进表、为什么排序、为什么另挂一张表 |
//! | [`skill_tools`]（`tool_table_skill.rs`，039/064） | **skill 这件事**：registry 归表拥有、每轮怎么展开成注入料、撞名怎么滤 |
//! | [`disable`]（`tool_table_disable.rs`，076） | **关掉内置这件事**：会话建立时把部署方给的某几件整条剔掉（唯一往回减的一件） |
//!
//! 拆的判据是「说得清它是干嘛的、且不含『和』」（红线 9）：注入、skill、关掉内置、
//! 名字规则各自都有一整套自己的理由要写，混在这里会让「工具表是什么」这句话说不完。

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_core::{AgentLimits, Reversibility, ToolCallRequest, ToolSpec};
use serde_json::Value;

use crate::collect_tool::collect_spec;
use crate::skill::SkillRegistry;
use crate::spawn_request::spawn_spec;
use crate::status_tool::status_spec;

use names::{location_of, reversibility_of};

/// 会话期间不变的工具表：喂模型的声明 + 判 `location`/`reversibility` 用的
/// 名字规则 + （039）宿主装载的 skill registry。
pub struct ToolTable {
    specs: Vec<ToolSpec>,
    /// 这张表拥有的 skill registry（039）。为什么它住在表里而不是 `RunnerCtx` 的
    /// 单独字段、每轮怎么展开成注入料、`late_tools` 撞上表里已有的名字怎么办，
    /// 全在 [`skill_tools`]（`with_skills`）。空 = 这个会话没开 skill。
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
    host_reversibility: BTreeMap<Arc<str>, Reversibility>,
}

#[path = "tool_table_names.rs"]
mod names;

#[path = "tool_table_host.rs"]
mod host;

#[path = "tool_table_skill.rs"]
mod skill_tools;

#[path = "tool_table_disable.rs"]
mod disable;

impl ToolTable {
    /// 从一组 specs 造一张表：空 skill registry、空 MCP 映射、空注入映射。四个内置
    /// 构造器共用它，免得每加一个字段就四处补一遍。
    fn from_specs(specs: Vec<ToolSpec>) -> Self {
        ToolTable {
            specs,
            registry: SkillRegistry::empty(),
            mcp_reversibility: BTreeMap::new(),
            host_reversibility: BTreeMap::new(),
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
        if self.declares(&spec.name) {
            debug_assert!(
                false,
                "ToolTable 已经有工具 `{}` 了，同名的后来这一条整条丢弃（specs 不 push，可逆性也不 insert）",
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

    /// web-agent 兼容的本地标准工具集：四个只读文件工具、受版本保护的工作区
    /// 事务、测试/lint 命令发现与六个静态命令工具。`read_file` 直接返回事务所需
    /// revision，因此模型不需要学习额外的内部前置工具。
    ///
    /// 此构造器不夹带历史 `srv:*` 别名，避免模型面对两套同义工具。浏览器交互工具
    /// 必须由 [`ToolTable::standard`] 的远程 router 注册，不能伪装为本地 executor。
    pub fn standard_local() -> Self {
        Self::from_specs(standard_local_specs())
    }

    /// 完整的 web-agent 标准工具集：本地工具外加三个由 Web 宿主执行并回传的交互
    /// 工具。它不注册计划、子 agent 或 MCP 工具。
    pub fn standard() -> Self {
        let mut specs = standard_local_specs();
        specs.extend(agent_tools::interaction_specs());
        Self::from_specs(specs)
    }

    /// 029 开闸：追加 `srv:agent/spawn`，宿主从此允许模型分解任务（决策 20）。
    ///
    /// **`limits` 必须跟 `Session` 手上那份是同一组数**（`Session::agent_limits`，
    /// 默认都是 [`AgentLimits::default`]）：这里的数字只进工具描述给模型看，真正
    /// 拦人的是 `Session::spawn_child` 里那两道闸。两边不一致不会出错，只会让模型
    /// 收到一句跟描述对不上的拒绝——所以宿主要么两边都用默认值，要么两边传同一个
    /// 值。数字进描述而不是让模型试出来，是为了省掉大部分「试→被拒→重试」的往返。
    ///
    /// 追加在末尾而不是插进 `builtin()` 内部：`builtin_specs()` 的顺序是 013 钉死
    /// 的既有契约，工具表在 prompt 最前面（红线 11），只加不改。
    pub fn with_spawn(mut self, limits: AgentLimits) -> Self {
        self.push_spec(spawn_spec(limits));
        self
    }

    /// 051 开闸：追加 `srv:agent/status`，模型从此能在子 agent 还在跑的时候看它们
    /// 此刻在干啥（M8，docs/ORCHESTRATION.md §三）。
    ///
    /// **跟 `with_spawn` 分开两个开关**（而不是塞进它）：每个 `with_*` 是一档独立的
    /// 授权，跟 `with_shell`/`with_skills`/`with_mcp` 一套规矩；而且工具表在 prompt
    /// 最前面（红线 11），把 status 折进 `with_spawn` 会让所有既有宿主的前缀无声
    /// 变一次。只开 status 不开 spawn 是**合法但没用**的组合（永远没有后代可看），
    /// 宿主该两个一起开——`agent-cli` / `ToolTableSpec::Full` 就是这么接的。
    ///
    /// 追加在末尾（红线 11：既有顺序是契约，只加不改）。宿主的链式顺序决定它落在
    /// 哪一格，`agent-cli` 把它紧跟在 `with_spawn` 之后、`with_skills`/`with_mcp`
    /// 之前：这样「静态那一段」在所有会话里逐字节相同，不随装了几个 skill / 几个
    /// MCP 工具而移位。
    pub fn with_status(mut self) -> Self {
        self.push_spec(status_spec());
        self
    }

    /// 053 开闸：追加 `srv:agent/collect`，模型从此能**择时**领后台子 agent 的结果
    /// （M8 闭环的最后一格，docs/ORCHESTRATION.md §三）。
    ///
    /// 跟 `with_spawn`/`with_status` 各自一档，理由同 `with_status`。只开 collect
    /// 不开 spawn 是**合法但没用**的组合（永远没有后台子可领）；反过来——开了
    /// spawn 不开 collect——才是真的会咬人：模型看得见 `background=true` 却没有
    /// 任何办法把结果拿回来，发出去的子全部在轮末被拆掉。宿主要么三个一起开，
    /// 要么把 spawn 也关掉。
    ///
    /// 追加在末尾（红线 11：既有顺序是契约，只加不改）。
    pub fn with_collect(mut self) -> Self {
        self.push_spec(collect_spec());
        self
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

fn standard_local_specs() -> Vec<ToolSpec> {
    let mut specs = agent_tools::standard_readonly_file_specs();
    specs.extend(agent_tools::standard_workspace_file_specs());
    specs.push(agent_tools::find_test_lint_commands_spec());
    specs.extend(agent_tools::command_specs());
    specs
}

#[cfg(test)]
#[path = "tool_table_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "standard_local_tests.rs"]
mod standard_local_tests;
