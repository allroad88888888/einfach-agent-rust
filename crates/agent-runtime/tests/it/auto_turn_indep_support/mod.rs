//! 211 独立验收共用件：**自我续写的留言链**怎么拼一段假 SSE 脚本，以及从
//! `AgentEvent` 流里挑出 `AutoTurnStarted`/`AutoTurnHeld` 两种事件。
//!
//! 复用 `send_indep_support`（206 独立测试留下的并发假服务器 / `RunnerCtx` 装配 /
//! SSE 脚本生成）——同样的理由：这份夹具里没有一处是 211 专属的。**不改
//! `send_indep_support` 本身**，避免影响它现有的一大批用例；211 专属的只有
//! 「一条链的一节长什么样」和「怎么从事件流里挑自驱动事件」这两件事，装在这里。
//!
//! # 一条链的「一节」（[`Leg`]）
//!
//! 211 的自驱动轮次靠的是：一个 agent 给 root 留一句 `when="next_turn"` 的话，
//! 这一轮结束后会话自己开下一轮把它读掉。但 `srv:agent/send` 不允许自己给自己
//! 发消息（`send_indep_refusals.rs`「自言自语」那条），所以 root 没法直接给自己
//! 留条——**留言必须经一个子 agent 转手**：root spawn 一个子、子干完活顺手给
//! root 留一条 `next_turn` 的话、子自己收尾、root 也收尾。这四跳是一节
//! （[`Leg`]），下一节的触发点就是这一节留下的那句话。
//!
//! # 黑盒来源
//!
//! `docs/issues/211-auto-driven-turns.md`、`agent_runtime`/`agent_core` 导出面上
//! 的签名与 rustdoc（`run_auto_turns`/`AutoTurnStep`/`RunnerEvent::AutoTurnStarted`
//! /`AutoTurnHeld`/`AutoTurnHold`/`Session::auto_turn_budget`/`spend_auto_turn`
//! 等）。**没有读** `crates/agent-runtime/src/auto_turn.rs`、
//! `crates/agent-core/src/command/auto_turn.rs`、
//! `crates/agent-core/src/command/transitions/user_input.rs`。
#![allow(dead_code, unused_imports)]

use std::time::Duration;

use agent_runtime::{AgentEvent, AutoTurnHold, RunnerEvent};

pub use crate::send_indep_support::{
    Route, RoutedServer, SEND_WIRE, build_ctx, calls_matching, index_of, injected, sse_text,
    sse_tool_call, temp_dir, tool_result, unread_warnings, wire_tool_name,
};

/// 零延迟的路由——211 的脚本全是本地假服务器，没有哪一条故意需要延迟
/// （喊停那条测试自己另外构造带延迟的路由，不经过这个helper）。
pub fn no_delay(needle: &'static str, lines: Vec<String>) -> Route {
    Route {
        needle,
        delay: Duration::ZERO,
        status: 200,
        lines,
    }
}

/// 一节链路：root 看到 `trigger_needle`（可能是用户这句话，也可能是上一节留下的
/// 那句 `next_turn` 笔记）→ spawn 一个子 → 子接到 `task_needle` 这个任务 → 子先
/// `send` 一条 `next_turn` 的笔记给 root（正文是 `note_text`，也是**下一节**的
/// `trigger_needle`）→ 子收尾 → root 收尾。四跳，四个 call_id 各不相同。
pub struct Leg {
    pub trigger_needle: &'static str,
    pub spawn_call_id: &'static str,
    pub task_needle: &'static str,
    pub send_call_id: &'static str,
    pub note_text: &'static str,
    pub child_final_text: &'static str,
    pub root_final_text: &'static str,
}

/// root 收尾跳：拿到 spawn 的 tool_result，答一句，这一节结束。
fn root2_route(leg: &Leg) -> Route {
    no_delay(leg.spawn_call_id, sse_text(leg.root_final_text))
}

/// 子收尾跳：拿到 send 的 tool_result，答一句，子落终态。`call_id` 认领，
/// **天然不会跟别的 agent 撞**——call_id 从来不会作为字面文本出现在别的 agent
/// 的历史里（只会撞*同一个* agent 自己更晚的那几跳，见 [`chain_routes`] 文档）。
fn child2_route(leg: &Leg) -> Route {
    no_delay(leg.send_call_id, sse_text(leg.child_final_text))
}

/// 子第一跳：接到任务，给 root 留一条 next_turn 笔记。
fn child1_route(leg: &Leg) -> Route {
    no_delay(
        leg.task_needle,
        sse_tool_call(
            leg.send_call_id,
            SEND_WIRE,
            &format!(
                r#"{{"to":"root","text":"{}","when":"next_turn"}}"#,
                leg.note_text
            ),
        ),
    )
}

/// root 第一跳：看到触发点，决定 spawn 一个子去干 `task_needle`。
fn root1_route(leg: &Leg) -> Route {
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    no_delay(
        leg.trigger_needle,
        sse_tool_call(
            leg.spawn_call_id,
            &spawn_wire,
            &format!(r#"{{"task":"{}"}}"#, leg.task_needle),
        ),
    )
}

/// 一整条链展开成路由表。**顺序不是任意的**——这份夹具第一版按“纯粹整体倒序”
/// 铺过一次，两处真机断言都红了，原因值得记下来：
///
/// - **子会在自己头上再长一层子**（`root/a1/a1` 而不是 `root/a2`）：子留笔记
///   那次调用的入参里原样写着 `note_text`（`{"text":"..."}` 那段 JSON），这段
///   文本从那一刻起**永远留在子自己的历史里**；而 `note_text` 同时也是**下一节**
///   的 `trigger_needle`。子的下一跳本该被 [`child2_route`]（`call_id` 认领）
///   截住，但如果“下一节的 root1”整个排在“这一节的 child2”前面，会先被那条
///   按文本认领的路由错误地当成「root 看到笔记决定 spawn 新子」。
/// - **自驱动那一轮答非所问**（返回上一节的收尾文案）：root 自己的历史一直在
///   累积，`call_spawn_i`/`TASK-Ai`/`trigger_needle_i` 这些文本会一直留在
///   root 后面每一跳的请求体里；如果“上一节的 call_id”排在“下一节的 root1”
///   前面，自驱动那一跳会被那个过期的 `call_id` 路由抢先认领，回的是上一节
///   已经说过的话，而不是这一跳该有的新回应。
///
/// 两条约束合起来，唯一自洽的顺序是：**每一节自己的 `child2`，要跟着排在
/// “下一节的 `root1`”前面；而“下一节的 `root1`”，要排在“这一节剩下的
/// `root2`/`child1`/`root1`”前面**——`chain_routes_with_extra` 的文档有一条
/// 具体展开的例子。
pub fn chain_routes(legs: &[Leg]) -> Vec<Route> {
    let n = legs.len();
    let mut out = Vec::with_capacity(n * 4);
    for i in (0..n).rev() {
        out.push(root2_route(&legs[i]));
        if i == n - 1 {
            // 最后一节没有「下一节」替它把 child2 提前拉走，这里补上。
            out.push(child2_route(&legs[i]));
        }
        out.push(child1_route(&legs[i]));
        if i > 0 {
            // 上一节的 child2，必须夹在“这一节剩下三跳”和“这一节的 root1”
            // 之间——它自己也在“上一节的 child1”那次请求体里出现过，但那次
            // 请求已经在更早（更后面被推入的 `out`）被这条路由认领过了。
            out.push(child2_route(&legs[i - 1]));
        }
        out.push(root1_route(&legs[i]));
    }
    out
}

/// root 看到触发点之后**直接答完**，不再 spawn 任何人——链在这一节收口，
/// 不留下新的 `next_turn` 笔记。
pub fn terminal_route(trigger_needle: &'static str, answer_text: &str) -> Route {
    no_delay(trigger_needle, sse_text(answer_text))
}

/// `chain_routes` 之外**再加一条**、触发点比链上任何一节都晚的路由（链收口
/// 用的 [`terminal_route`]，或是喊停测试里故意让某一跳挂住的那条）——把 `extra`
/// 当成「虚拟的下一节」的 `root1`，套 [`chain_routes`] 同一条规则：它要排在
/// **最后一节自己的 `child2`**之后，但排在**最后一节剩下的
/// `root2`/`child1`/`root1`**之前。
///
/// 举例（一节 `leg0` + 一条 `extra`）拼出来的顺序是
/// `[child2_0, extra, root2_0, child1_0, root1_0]`——`extra` 两边都贴着它该贴
/// 的边界，不是随手 `Vec::insert(0, extra)` 就能凑对的（第一版就是这么栽的：
/// 插在最前面会跟 `child2_0` 撞车，重演上面文档说的第一种坏法）。
pub fn chain_routes_with_extra(legs: &[Leg], extra: Route) -> Vec<Route> {
    let n = legs.len();
    if n == 0 {
        return vec![extra];
    }
    let mut out = Vec::with_capacity(n * 4 + 1);
    out.push(child2_route(&legs[n - 1]));
    out.push(extra);
    for i in (0..n).rev() {
        out.push(root2_route(&legs[i]));
        out.push(child1_route(&legs[i]));
        if i > 0 {
            out.push(child2_route(&legs[i - 1]));
        }
        out.push(root1_route(&legs[i]));
    }
    out
}

/// 事件流里全部 `AutoTurnStarted.remaining`，按发生顺序——**减在开轮之前**这条
/// 断言就靠它：n 格预算跑 k 轮，该看到 `n-1, n-2, .., n-k`。
pub fn auto_turn_started_remaining(events: &[AgentEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            RunnerEvent::AutoTurnStarted { remaining } => Some(*remaining),
            _ => None,
        })
        .collect()
}

/// 事件流里全部 `AutoTurnHeld`，按发生顺序：`(还剩几条留言, 为什么没开)`。
pub fn auto_turn_held(events: &[AgentEvent]) -> Vec<(usize, AutoTurnHold)> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            RunnerEvent::AutoTurnHeld { pending, reason } => Some((*pending, reason.clone())),
            _ => None,
        })
        .collect()
}
