//! `srv:agent/await`：模型用来**挂起等另一个 agent 到某个状态**的那个工具
//! （212，决策 35 §一）。
//!
//! # 它跟 `collect` 的分工，必须在描述里说清
//!
//! **`await` 只告诉你「它到了」，不给正文；正文要 `collect`。** 而 `collect` 本身
//! 就会等——所以「等一个自己 spawn 的后台子」直接 `collect` 就行，用不着先 `await`。
//! `await` 的用武之地是**等一个不归你领的**：兄弟，或者别人开的。
//!
//! 不说清的话模型会拿 `await` 当 `collect` 用，然后抱怨拿不到正文。
//!
//! # 建立那一刻查环，不是卡住之后再救
//!
//! 判据在 core（`Session::await_agent`），这里只把它的拒绝翻成给模型看的话。
//! **为什么必须在门口挡**：两个互等的 agent 都在等、都没有 provider 调用在飞，
//! 泵的静止条件是「两张在飞表都空」，于是它**安静地返回**，留下两个永远
//! `Pending` 的槽——没有 panic、没有超时、没有告警。
//!
//! # 可逆性
//!
//! `Aftermath::Nothing` → `Undoability::StateOnly`：它落一条 entry（等待边），
//! 但没碰外部世界，回滚那条 entry 就是全部补偿。

use std::sync::Arc;

use agent_core::{AgentId, AwaitDenied, AwaitUntil, Epoch, Session, ToolCallId, ToolSpec};
use serde_json::{Value, json};

use crate::await_slot::AwaitSlots;
use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::reply;

/// 工具全名。`srv:` = 服务端本地执行（docs/TOOLS.md 的命名约定）。
pub const AWAIT_TOOL: &str = "srv:agent/await";

/// 喂给模型的声明。
pub fn await_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(AWAIT_TOOL),
        description: Arc::from(
            "挂起等这个会话里另一个 agent 到达某个状态——**你这次调用会一直等**，\
             等到了才返回。\n\
             `id` 是对方的 agent id，用 srv:agent/status 拿。**兄弟也等得了**，\
             不限于你自己开出来的。\n\
             `until` 三档：`\"settled\"`（缺省）= 它收场了，不管成没成；\
             `\"done\"` = 只等它成功收场；`\"failed\"` = 只等它失败收场。\
             要是它以**别的方式**收场了（比如你等 done 而它失败了），\
             你会当场收到一个错误，而不是一直等下去。\n\
             **它只告诉你「到了」，不给你对方的回答正文**——正文用 srv:agent/collect 领。\
             而 collect 本身就会等，所以**等一个你自己开的后台子 agent，直接 collect \
             就行，不用先 await**。这个工具是用来等**不归你领的那些**：兄弟，\
             或者别人开出来的。\n\
             等不成的情况会当场告诉你：等自己、等一个不在会话里的 id、\
             等一个已经不活着的 agent、以及**会造成互相等待**（你等它、它（直接或\
             间接）在等你）——最后这种一定会被拒，因为真等下去两边都永远动不了。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "要等的 agent id，比如 root/a1。用 srv:agent/status 拿。不能是你自己。"
                },
                "until": {
                    "type": "string",
                    "enum": ["settled", "done", "failed"],
                    "description": "等到什么算等到。省略 = settled（收场了就算，不管成没成）。"
                }
            },
            "required": ["id"]
        })),
    }
}

/// 截获一次 `srv:agent/await`。
///
/// 成功 → **不产出任何事件**（`Dispatched::Nothing`）：调用方那个工具槽保持
/// `Pending`，由 [`crate::await_slot::AwaitSlots::harvest`] 在等到时收敛。
/// 这跟 `collect` 等一个还在跑的子是同一条路。
///
/// **调 `persist::sync`**：`await_agent` 落了一条 entry（等待边），跟 spawn 那条
/// 截获同一条理由——不同步的话，恢复出来的会话查不到这条边，反向 `await` 会被
/// 错误地放行。
pub(crate) fn intercept(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    awaits: &mut AwaitSlots,
    agent: &AgentId,
    call_id: ToolCallId,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    let request = ctx.tools.snapshot(AWAIT_TOOL, Arc::clone(input));
    ctx.emit(
        agent,
        RunnerEvent::ToolExecuting {
            call_id: call_id.clone(),
            request,
        },
    );

    let (target, until) = match parse(input) {
        Ok(parsed) => parsed,
        Err(message) => return reply::refuse(ctx, agent, call_id, epoch, AWAIT_TOOL, message),
    };

    match session.await_agent(agent, &target, until) {
        Ok(()) => {
            crate::persist::sync(ctx, session);
            awaits.record(agent.clone(), target, until, call_id, epoch);
            Dispatched::Nothing
        }
        Err(denied) => {
            let message = explain(&denied);
            reply::refuse(ctx, agent, call_id, epoch, AWAIT_TOOL, message)
        }
    }
}

/// 模型给的入参 → `(目标, 条件)`。**错误一律是给模型看的文本**（003 的哲学）。
pub(crate) fn parse(input: &Value) -> Result<(AgentId, AwaitUntil), String> {
    let target = match input.get("id") {
        Some(Value::String(s)) if !s.trim().is_empty() => AgentId::new(s.trim()),
        Some(Value::String(_)) => return Err("await 失败：id 是空的。".to_string()),
        None | Some(Value::Null) => {
            return Err("await 失败：缺少必填参数 id（要等的 agent id，比如 root/a1）。\
                        用 srv:agent/status 看整棵树就能拿到。"
                .to_string());
        }
        Some(_) => {
            return Err("await 失败：id 得是字符串（agent 的路径 id，比如 root/a1）。".to_string());
        }
    };
    let until = match input.get("until") {
        None | Some(Value::Null) => AwaitUntil::Settled,
        Some(Value::String(s)) => AwaitUntil::parse(s.trim()).ok_or_else(|| {
            "await 失败：until 只能是 \"settled\"（收场就算）、\"done\"（只等成功）\
             或 \"failed\"（只等失败）。省略 = settled。"
                .to_string()
        })?,
        Some(_) => {
            return Err("await 失败：until 得是字符串。".to_string());
        }
    };
    Ok((target, until))
}

/// core 的拒绝 → 给模型看的话。**每一条都要给出下一步**（同 `send_tool::explain`）。
fn explain(denied: &AwaitDenied) -> String {
    match denied {
        AwaitDenied::Yourself { .. } => "await 失败：不能等你自己——你正在跑，\
             等待期间你不可能收场，那是一个永远等不到的条件。"
            .to_string(),
        AwaitDenied::NotInSession { target } => format!(
            "await 失败：{} 不在这个会话里。用 srv:agent/status 看有哪些 agent。",
            target.as_str(),
        ),
        AwaitDenied::NotLive { target } => format!(
            "await 失败：{} 已经不在活 agent 里了——没 spawn 出来过，或者已经被\
             撤销/拆掉。它不会再到达任何状态，等它就是永远等。",
            target.as_str(),
        ),
        // **把环上那条链原样列出来**（212）：只说「会成环」模型不知道该绕开谁，
        // 只会换个写法再撞一次。
        AwaitDenied::WouldCycle { chain } => format!(
            "await 失败：这会造成**互相等待**——{}。真等下去两边都永远动不了，\
             而且不会有任何报错，所以这里直接拒。\
             要么让其中一方先做完自己的事，要么改用 srv:agent/send 把结果推过去。",
            render_chain(chain),
        ),
    }
}

/// 环上那条链 → 一句话。**进 prompt，逐字节确定**（红线 11）：顺序就是遍历顺序，
/// 没有排序、没有时间戳。
fn render_chain(chain: &[AgentId]) -> String {
    let ids: Vec<&str> = chain.iter().map(|id| id.as_str()).collect();
    format!("你要等的那条链是 {}，绕回来正好是你", ids.join(" 在等 "))
}

#[cfg(test)]
#[path = "await_tool_tests.rs"]
mod tests;
