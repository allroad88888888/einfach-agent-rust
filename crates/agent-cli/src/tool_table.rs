//! CLI 这个宿主的工具表长什么样：从 `standard_local` 起，按**固定顺序**接上
//! spawn 三件套、skills、MCP、识图、扩展包。
//!
//! 拆自 `main.rs`（162 把它顶过 300 行，红线 9）。抽出来的理由不只是行数：
//! 这一段是「工具表的组成与顺序」这一件事，而 `main` 的职责是把各个模块串成一次
//! 启动——两个层面混在一个函数里，读 main 的人得先跳过三十行工具表细节才能看到
//! 下一步做了什么。
//!
//! # 顺序是契约，不是风格（红线 11）
//!
//! `builtin → shell → spawn → status/collect → send/self/notes/await → skills → MCP → vision → 扩展`
//! 这个次序**只加不改**：进 prompt 的东西序列化必须逐字节确定，工具表在 prompt
//! 最前面，任何一次插队都会让那一段之后的全部字节位移，缓存整段作废
//! （038 探针实测：DeepSeek 上「中途改工具数组」把命中率归零，120 倍差价）。
//! 所以静态的那一段必须在所有会话里逐字节相同，不随装了几个 skill / 几个 MCP
//! 工具而移位——新增的东西一律**追加在表尾**。

use std::path::Path;

use agent_core::{AgentLimits, Reversibility, ToolSpec};
use agent_runtime::{SkillRegistry, ToolTable};

use crate::ext_stats;

/// 装配的输入。字段顺序即装配顺序，读起来就是那张表。
pub struct Parts {
    /// spawn 的两道上限。**必须是 `session.agent_limits()`**——写进工具描述给
    /// 模型看的数字，得跟 `Session::spawn_child` 真正拦人的那两道闸是同一组
    /// （`ToolTable::with_spawn` 的文档记着这个耦合；恢复路径那一半在 160）。
    pub limits: AgentLimits,
    pub skills: SkillRegistry,
    pub mcp_tools: Vec<(ToolSpec, Reversibility)>,
    /// 配了 kimi 段才声明识图工具（s5）。
    pub vision: bool,
    /// `--ext-stats` 开关（149）。关着 = `with_extension` 一次都不调，工具表逐字节
    /// 回到不装扩展的样子。
    pub ext_stats: bool,
    pub session_file: Option<std::path::PathBuf>,
}

/// 装出工具表 + 扩展包的 ctx 半边。
///
/// `ext_pending`（返回值第二项）**必须**在 `RunnerCtx` 建好之后 install——忘了的话
/// debug 构建会在它的 `Drop` 里当场炸（EXTENSIONS.md §防呆）。
pub fn assemble(
    parts: Parts,
    note: &mut dyn FnMut(&str),
) -> (ToolTable, Option<agent_runtime::PendingInterceptors>) {
    // 本地标准工具集含受版本保护、可显式撤回的文件事务；不会把浏览器/桌面交互
    // 伪装成本地工具。
    //
    // spawn 三件套**一起开**：`background=true` 的 spawn 没有 collect 就是个陷阱
    // ——模型看得见后台这条路，却没有任何办法把结果拿回来，发出去的子全部在轮末
    // 被拆掉（`ToolTable::with_collect` 的文档记着这条）。`with_status`（051）/
    // `with_collect`（053）紧跟 `with_spawn` 之后、skills/MCP 之前。
    let mut table = ToolTable::standard_local()
        .with_spawn(parts.limits)
        .with_status()
        .with_collect()
        // M20（决策 35）追加在编排三件套之后、skills/MCP 之前——同一条「静态那
        // 一段在所有会话里逐字节相同」的规矩。`send`（206）给会话里任意活 agent
        // 说一句话，`self`（208）看自己还剩多少额度，`notes`（209）是它自己的
        // 草稿纸。**不声明就等于没有**：
        // 截获注册跟着 `declares()` 走（`agent_runtime::builtin_intercepts`），
        // 表里没有这一行，模型连这个工具存在都不知道。
        .with_send()
        .with_self()
        .with_notes()
        .with_await()
        .with_skills(parts.skills)
        // MCP 工具追加在最后：server 之间按 id、server 内按 tools/list，
        // 已经在 `mcp::bootstrap` 排好序了。
        .with_mcp(parts.mcp_tools);
    if parts.vision {
        table = table.with_vision_inspect();
    }
    ext_stats::install(table, parts.ext_stats, parts.session_file.as_deref(), note)
}

/// `Parts::session_file` 要的是拥有所有权的路径；`main` 手上是 `Option<&Path>`
/// 的场合用这个转一下，省得调用点写 `.map(Path::to_path_buf)`。
pub fn owned(path: Option<&Path>) -> Option<std::path::PathBuf> {
    path.map(Path::to_path_buf)
}
