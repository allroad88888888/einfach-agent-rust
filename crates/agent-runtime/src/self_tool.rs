//! `srv:agent/self`：模型用来看**自己还剩多少额度**的那个工具（208，决策 35 §三）。
//!
//! # 为什么要有它
//!
//! 208 之前模型对自己的账完全瞎着：还剩几轮、还能开几个子、上下文压没压过，
//! 一样都不知道。于是「快没轮次了就收敛输出」这件事它做不到——只能一路调工具
//! 到闸把它切断（`TurnGuard`），那一轮的结论就永远说不出口了。
//!
//! # 它是纯读，一个写口都不开
//!
//! 「改本 agent 状态」的正确形状是 [`crate::notes_tool`]（209）那个**属于模型
//! 自己的槽位**，不是给这里任何一格开写口：这里每一格都是别人的账——`MaxTurns`
//! 是部署方的、`ToolsAllowed` 是父给的、`Summaries` 是 adapter 的。给它们开写口
//! 等于让被约束者改自己的约束。
//!
//! # 自读不经跨 agent 那条路
//!
//! `Slot::TurnsUsed` 这些站 `Private`，而 `Private` 的意思是「**别的** agent
//! 读不到」，不是「自己也读不到」（`graph::visibility` 的类型文档专门澄清过）。
//! 所以这里走的是 core 的 per-agent 取料口（`turns_used_of` 那一批），不经
//! `read_agent`——那是跨 agent 的口，红线 10 的校验在那边。
//!
//! **必须用带 `_of` 的那一批**：不带参数的 `turns_used()` 读的恒是 root，
//! 子 agent 调 `self` 会拿到 root 的预算当成自己的，链通、值错、不报错。
//!
//! # 诚实标注是这个文件最要紧的一句话
//!
//! 这一轮回「本轮已经请求 3 次」，三轮之后模型在历史里读到的还是那个 3，
//! **而它早过期了**。跟时间戳进 prompt 是同一类病：一个看起来永远成立的事实，
//! 冻进历史之后就是假的。所以工具描述与正文都写明「**你调用这一刻**」，
//! 不许写成无时态的断言。
//!
//! # 可逆性
//!
//! `Aftermath::Nothing` → `Undoability::StateOnly`：一次纯读，没碰外部世界，
//! 连一条 command 都没发。

use std::sync::Arc;

use agent_core::{AgentId, Epoch, Session, ToolCallId, ToolSpec};
use serde_json::{Value, json};

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::reply;
use crate::self_render::{SelfFacts, render};

/// 工具全名。`srv:` = 服务端本地执行（`Location::Server`，docs/TOOLS.md 的命名
/// 约定），`agent/self` = 这一族里的 self。
pub const SELF_TOOL: &str = "srv:agent/self";

/// 喂给模型的声明。
///
/// **无入参**：自己是谁由截获现场的 `AgentId` 决定，不给模型一个能填错的口。
/// 要看别人用 `srv:agent/status`。
pub fn self_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(SELF_TOOL),
        description: Arc::from(
            "看一眼你自己现在还剩多少额度：本轮还能请求几次、还能开几个子 agent、\
             还能往下几层、看得到几个工具、上下文压过没有。**不阻塞**，当场返回，\
             不用填任何参数。\n\
             **它给的是你调用那一刻的数**——不是一个以后还成立的事实。\
             你在历史里读到的上一次结果早就过期了，要最新的就再调一次。\n\
             什么时候用：动手拆任务之前（先看还能开几个子、还能往下几层），\
             以及一轮里干了不少事之后（看还剩几次请求，**快用完就先把结论说出来**，\
             别一路调工具到被切断）。\n\
             它只回你自己的账，不回别人的——要看这个会话里还有谁、谁在干啥，\
             用 srv:agent/status。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {}
        })),
    }
}

/// 截获一次 `srv:agent/self`。
///
/// **当场回写、无 Pending、无在飞凭据**：一次纯读，结果在这个函数里就算完了。
/// **不调 `persist::sync`**（照 `status_tool::intercept` 的既有理由）：一条
/// command 都没发，没有任何东西需要同步进持久化后端。
///
/// 入参一律忽略（schema 是空对象，模型多塞了字段也不算错——为一个不影响结果的
/// 多余字段回一句 `is_error` 只会浪费一轮）。
pub(crate) fn intercept(
    session: &Session,
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    // self 也是一次工具调用，该跟别的工具一样看得见「调了什么、参数是什么」。
    let request = ctx.tools.snapshot(SELF_TOOL, Arc::clone(input));
    ctx.emit(
        agent,
        RunnerEvent::ToolExecuting {
            call_id: call_id.clone(),
            request,
        },
    );

    let facts = collect(session, ctx, agent);
    reply::ok(ctx, agent, call_id, epoch, SELF_TOOL, render(&facts))
}

/// 把这个 agent 此刻的账读成一份 [`SelfFacts`]。
///
/// 上限那两个数取 `session.agent_limits()`——**跟 `Session::spawn_child` 真正
/// 拦人的是同一组数**（决策 32 之后它是进程级启动参数）。写死 3/8 两个字面量
/// 会让「告诉模型的」和「真正拦人的」在部署方调过参数之后悄悄分家，而那种
/// 不一致的症状是模型收到一句跟描述对不上的拒绝，没有任何报错。
fn collect(session: &Session, ctx: &RunnerCtx, agent: &AgentId) -> SelfFacts {
    let limits = session.agent_limits();
    SelfFacts {
        id: agent.clone(),
        depth: agent.depth(),
        max_depth: limits.max_depth,
        turns_used: session.turns_used_of(agent),
        max_turns: session.max_turns_of(agent),
        retries_used: session.retries_used_of(agent),
        max_retries: session.max_retries_of(agent),
        children_live: session.children_of(agent).len(),
        max_children: limits.max_children,
        // 有效工具表：root = 宿主整张表，子 agent = 它 spawn 当时那份子集。
        // 走 `subagent::tools_for` 而不是自己数一遍——它是组 prompt 时用的那
        // 同一个函数，两处各数一遍就会有「说的 12 个、实际给 11 个」的一天。
        tools: crate::subagent::tools_for(session, ctx, agent).len(),
        compacted: !session.summary_library(agent).is_empty(),
    }
}

#[cfg(test)]
#[path = "self_tool_tests.rs"]
mod tests;
