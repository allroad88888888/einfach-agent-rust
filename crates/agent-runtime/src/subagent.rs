//! 子 agent 的**料**：它的 system 是什么、它看得见哪些工具。
//!
//! 一个 agent 的消息历史和前缀镜像是它自己槽位里的东西（`Session` 的 per-agent
//! 读口直接给），这个文件补的是另外两样——它们不在原子图里，来自宿主：
//! system chunks 在 [`RunnerCtx`]，工具表也在 [`RunnerCtx`]。135 起，
//! `system_for` 还要在两者之间接一段——134/135 那份「开局工具跑出来的会话级
//! 前缀」，值在 `Session`（`Session::prefix_chunks`），不在 `RunnerCtx`，见
//! 下面这个函数的实现。
//!
//! # root 与子 agent 走同一个函数
//!
//! 两个函数都收 `agent`，root 只是「`ToolsAllowed` 是 `Null`」的那一支。分成
//! 「root 的取料」和「子 agent 的取料」两条路，第一次改 system 分段就会只改一条。
//!
//! # M3 v1 的固定模板（029 §注意）
//!
//! 普通子 agent 的 system = 宿主的那几段原样 + 一段固定的「你是被分解出的子任务
//! 执行者」。**任务文本不在这里**，它是子 agent 的第一条 user 消息——这
//! 不只是形式选择：模板不带任务文本，兄弟子 agent 的 `[Tools][System]` 前缀可逐
//! 字节相同（红线 11），前缀缓存可以共享。skills 装载仍未排期（029 §注意）。

use std::sync::Arc;

use agent_core::{AgentId, AgentLimits, Session, SystemChunk, ToolSpec};

use crate::ctx::RunnerCtx;

/// 子 agent 那段固定 system 的标签（进日志，不进 prompt——见 `SystemChunk`）。
const SUBAGENT_LABEL: &str = "subagent";

/// 组这个 agent 的 system 分段。
pub(crate) fn system_for(session: &Session, ctx: &RunnerCtx, agent: &AgentId) -> Vec<SystemChunk> {
    let mut chunks = ctx.system.clone();
    // 135：开局工具跑出来的前缀块（134 的状态，创建期定下、之后不变）排在
    // 基础 system 之后、子 agent 模板段之前。root 与子 agent 同路——前缀属于
    // 这个会话，不属于树上某一个 agent（同 `Session::set_prefix_chunks` 的
    // 落点），两者看到的必须是同一份，不因为「谁在问」而不同。空前缀（没开
    // timed 工具的会话）→ `extend` 一个空 `Vec`，逐字节回到 135 之前。
    //
    // 145：`prefix_allowed_of` 按 spawn 当时快照的名单过滤这份前缀——`None`
    // （root、缺省 spawn、145 之前的所有旧状态）不过滤，`extend` 的还是
    // `session.prefix_chunks()` 原样全部，逐字节回到 145 之前（红线 11 向后
    // 兼容）。过滤只读缓存值，**不重跑任何 timed 工具**：会话只在创建期跑一次
    // `run_session_start`（135 的契约），这里晚了、也没有 `ToolTable` 手上那份
    // 执行体可调——`Session` 压根不认识 `TimedRun` 是什么。
    chunks.extend(filter_prefix_chunks(
        session.prefix_chunks(),
        session.prefix_allowed_of(agent).as_deref(),
    ));
    if session.tools_allowed_of(agent).is_some() {
        chunks.push(SystemChunk {
            label: Arc::from(SUBAGENT_LABEL),
            text: Arc::from(subagent_prompt(session.agent_limits())),
        });
    }
    chunks
}

/// 145：按 `allowed` 过滤一份前缀块。`None` = 不过滤，原样返回（缺省语义，
/// 逐字节等同 145 之前）；`Some(set)` = 只留 label 形如 `init:<name>` 且
/// `<name> ∈ set` 的那几块——`init:` 前缀是 135（`session_start.rs`）钉的契约，
/// 这里只认它，不重新发明一套判定；不匹配这个形状的块（目前生产代码里没有，
/// 防的是将来 `prefix_chunks` 多出别的写入点）在过滤生效时一并被拿掉，因为
/// `prefix_allowed_of` 的语义就是「白名单」，不认识的东西不在白名单里。
fn filter_prefix_chunks(
    chunks: Vec<SystemChunk>,
    allowed: Option<&[Arc<str>]>,
) -> Vec<SystemChunk> {
    let Some(allowed) = allowed else {
        return chunks;
    };
    chunks
        .into_iter()
        .filter(|chunk| {
            chunk
                .label
                .strip_prefix("init:")
                .is_some_and(|name| allowed.iter().any(|a| &**a == name))
        })
        .collect()
}

/// 组这个 agent 看得见的工具表。
///
/// **顺序取宿主表的顺序，不取 `ToolsAllowed` 的顺序**（红线 11）：工具表在
/// prompt 最前面，两个子 agent 拿同一份子集却排成两种顺序的话，它们之间的前缀
/// 缓存就一次也命不中。过滤保序天然做到这一点，不需要再排一次。
pub(crate) fn tools_for(session: &Session, ctx: &RunnerCtx, agent: &AgentId) -> Vec<ToolSpec> {
    match session.tools_allowed_of(agent) {
        // root：宿主表。
        None => ctx.tools.specs().iter().cloned().collect(),
        Some(allowed) => ctx
            .tools
            .specs()
            .iter()
            .filter(|spec| allowed.iter().any(|name| **name == *spec.name))
            .cloned()
            .collect(),
    }
}

/// 这个 agent 现在**有效**的工具全名清单：root = 宿主整张表，子 agent = 它的
/// `ToolsAllowed`。
///
/// spawn 用它做两件事：给没指定 `tools` 的子 agent 兜默认值（= 父的工具子集），
/// 以及校验模型指定的名字——**子拿不到父没有的工具**，那是提权。
pub(crate) fn allowed_names(session: &Session, ctx: &RunnerCtx, agent: &AgentId) -> Vec<Arc<str>> {
    match session.tools_allowed_of(agent) {
        None => ctx
            .tools
            .specs()
            .iter()
            .map(|spec| Arc::clone(&spec.name))
            .collect(),
        Some(allowed) => allowed,
    }
}

/// 固定模板本体。上限数字来自会话的 [`AgentLimits`]（决策 20 的「数字参数」），
/// 渲染是纯拼接，同一份 limits 两次渲染逐字节相同（红线 11）。
fn subagent_prompt(limits: AgentLimits) -> String {
    format!(
        "你是被分解出的子任务执行者：上一级 agent 把一件事拆给了你，你只负责这一件。\n\
         - 任务在下一条用户消息里，照它做，做完直接给结论。\n\
         - 你的回复会原样作为工具结果回到上一级，所以**只说结论和必要依据**，\
         不要写「好的，我这就去做」这类过场话，也不要反问——没有人会回答你。\n\
         - 你只看得见上一级分给你的那几个工具。\n\
         - 整棵 agent 树的结构上限：深度最多 {}（root 是 0），每个 agent 最多 {} 个\
         活着的直接子 agent。你自己也受这两个数约束。",
        limits.max_depth, limits.max_children,
    )
}

#[cfg(test)]
#[path = "subagent_tests.rs"]
mod tests;
