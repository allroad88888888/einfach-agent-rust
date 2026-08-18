//! `srv:agent/send`：模型用来**给这个会话里另一个 agent 说一句话**的那个工具
//! （206，决策 35 §二）。
//!
//! # 它只负责投递，不负责送达
//!
//! 这个文件做的事就一件：把一条话放进目标的收件箱（`Session::deliver`），当场回写。
//! **什么时候被喂进对方的 prompt 是两个定点的事**——`Deliver::Now` 在收信人下一次
//! 组装 provider 请求之前（`dispatch` 的 `CallProvider` 臂），`Deliver::NextTurn`
//! 在 root 下一轮开始时（`runner::run_turn_async` 顶部）。
//!
//! **214 起它会唤醒收信人**：投的是 `Deliver::Now`、而收信人已经落终态 → 这次
//! 截获额外产出一条 [`Event::Wake`]，把它在**同一个 turn 内**拉回泵里。
//!
//! 206 落地时做不到这件事：`Effect::CallProvider` 全系统只从 `try_call_provider`
//! 一处发出，而它当时的四个入口每一个都要求那个 agent 正走在流程里。214 给了
//! 第五个入口——**一条名字叫得出自己在干什么的转移**，不是把 `on_user_input`
//! 的闸放宽（那会把 turn 边界藏进一格转移里，见 `transitions::wake` 模块文档）。
//!
//! 叫不醒的两种情况仍然落回 206 的行为（条目留在收件箱、轮末由
//! [`crate::unread_inbox`] 告警）：收信人的 `max_turns` 已经用完，
//! 或者投的是 `NextTurn`。
//!
//! # 为什么两档的合法目标不一样
//!
//! `NextTurn` **只能投给 root**：子 agent 不跨 turn（ORCHESTRATION §二/§四.4，
//! 孤儿在 turn 收尾被 `despawn_child` 拆掉），投给别人等于投进一个下一轮不存在的
//! 收件箱。core 的 `deliver` 会拒，这里把那个拒绝翻译成给模型看的话。
//!
//! # 可逆性
//!
//! `Aftermath::Nothing` → `Undoability::StateOnly`：纯状态，没碰外部世界，
//! `/undo` 回滚状态就够了，不需要还原钩子。

use std::sync::Arc;

use agent_core::{AgentId, Deliver, DeliverDenied, Epoch, Event, Session, ToolCallId, ToolSpec};
use serde_json::{Value, json};

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::reply;

/// 工具全名。`srv:` = 服务端本地执行（`Location::Server`，docs/TOOLS.md 的命名
/// 约定），`agent/send` = 这一族里的 send。
pub const SEND_TOOL: &str = "srv:agent/send";

/// 喂给模型的声明。
pub fn send_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(SEND_TOOL),
        description: Arc::from(
            "给这个会话里另一个 agent 说一句话。**不等回复**，当场返回。\n\
             `to` 是对方的 id——用 srv:agent/status 看整棵树就能拿到，你的子孙、\
             上级、以及跟你平级的兄弟都发得到。\n\
             `when` 决定它什么时候被对方读到，两档差别很大：\n\
             - `\"now\"`（缺省）= 加入对方本轮的 loop，**它下一次请求就带上**。\
             用来中途纠偏：你看到某个 agent 在往错方向走，喊一嗓子。\
             **对方已经答完了也发得动**——这条会把它叫醒，它在这一轮里接着干；\
             唯一叫不醒的情况是它自己的轮次预算用光了，那样这条就没人读。\n\
             - `\"next_turn\"` = **这一轮结束之后**、下一轮开始时才送达。\
             用来留话：你后台干完的事，想让下一轮有人知道。\
             它**只能发给 root**（最上面那个），因为子 agent 活不到下一轮。\n\
             要对方的回答正文用 srv:agent/collect，不是这个——这个只是把话放过去，\
             对方看不看得到、答不答你，是它自己的事。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "收信人的 agent id，比如 root/a1。用 srv:agent/status 拿。不能是你自己。"
                },
                "text": {
                    "type": "string",
                    "description": "要说的话。它会原样进对方的对话历史，并标上是你发的。"
                },
                "when": {
                    "type": "string",
                    "enum": ["now", "next_turn"],
                    "description": "now（缺省）= 加入对方本轮 loop；next_turn = 这一轮结束后送达，且只能发给 root。"
                }
            },
            "required": ["to", "text"]
        })),
    }
}

/// 截获一次 `srv:agent/send`。
///
/// **当场回写、无 Pending、无在飞凭据**：投递是一条同步命令。
/// **不调 `persist::sync`**（照 `status_tool` / `collect_tool` 的既有理由）——
/// `deliver` 是命令，它那条 entry 走命令层的常规路；真正要同步的是随后经
/// `Session::step` 的那条 `ToolResult`，泵在 A 段自己会转发。
///
/// 失败（入参写错、目标不活、`next_turn` 投给了子 agent……）→ `is_error` 的
/// tool_result 喂回模型让它自己收敛（决策 20 的哲学，跟 spawn / status / collect
/// 一致），不 panic、不卡住这一轮。
pub(crate) fn intercept(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    // send 也是一次工具调用，该跟别的工具一样看得见「调了什么、参数是什么」。
    let request = ctx.tools.snapshot(SEND_TOOL, Arc::clone(input));
    ctx.emit(
        agent,
        RunnerEvent::ToolExecuting {
            call_id: call_id.clone(),
            request,
        },
    );

    let (to, text, when) = match parse(input) {
        Ok(parsed) => parsed,
        Err(message) => return reply::refuse(ctx, agent, call_id, epoch, SEND_TOOL, message),
    };

    match session.deliver(agent, &to, text, when) {
        Ok(()) => {
            let body = format!("已经投给 {} 了。{}", to.as_str(), when_note(when));
            let settled = reply::ok(ctx, agent, call_id, epoch, SEND_TOOL, body);
            match wake_needed(session, &to, when) {
                Some(wake) => also(settled, wake),
                None => settled,
            }
        }
        Err(denied) => {
            let message = explain(&denied, session);
            reply::refuse(ctx, agent, call_id, epoch, SEND_TOOL, message)
        }
    }
}

/// 这次投递要不要顺带把收信人叫醒（214）。
///
/// 三个条件缺一不可：
///
/// - **`Deliver::Now`**：`NextTurn` 的条目本来就该等到下一轮，叫醒它等于把一条
///   刻意延后的话当场灌进去。
/// - **收信人已经落终态**：还在跑的下一次请求本来就会带上那条话（206 的定点在
///   `CallProvider` 派发处），再推一次只会凭空多烧一轮——而且会撞
///   `on_wake` 的 `protocol_violation`。
/// - 消息**已经投进去了**（这个函数只在 `deliver` 成功之后调）。
///
/// **这里不 `drain_now`**：唤醒转移发出 `Effect::CallProvider`，而排空的定点就在
/// 它的派发处（`crate::dispatch`）。在这儿再排一次等于把话搬进 `Messages` 两遍。
fn wake_needed(session: &Session, to: &AgentId, when: Deliver) -> Option<Event> {
    if when != Deliver::Now || !session.status_of(to).is_terminal() {
        return None;
    }
    Some(Event::Wake {
        agent: to.clone(),
        epoch: session.epoch(),
    })
}

/// 一次截获产出两件事：先收敛发送方那个槽，再叫醒收信人。
///
/// 顺序有意义——反过来的话，收信人这一轮的活会排在发送方的 `tool_result` 之前，
/// 而那条 result 才是发送方接着往下走的凭据。
fn also(settled: Dispatched, extra: Event) -> Dispatched {
    match settled {
        Dispatched::Event(first) => Dispatched::Events(vec![first, extra]),
        // `reply::ok` 今天恒回 `Dispatched::Event`。真走到这儿说明它改了形状，
        // 而那种情况下静默丢掉唤醒会变成「消息投进去了、没人叫醒、轮末才告警」
        // ——一个只在改了别处之后才浮出来的静默降级。
        other => {
            debug_assert!(false, "reply::ok 不再回 Dispatched::Event，214 的唤醒被丢了");
            other
        }
    }
}

/// 成功回执的后半句：**说清它什么时候会被读到**。
///
/// 两档的后果不同，而模型看到的只有这一句——不说清，它会以为 `now` 等于「对方
/// 已经知道了」。
fn when_note(when: Deliver) -> &'static str {
    match when {
        Deliver::Now => "它下一次请求就会带上——已经答完的会被这条叫醒接着干，\
                         除非它的轮次预算已经用光。",
        Deliver::NextTurn => "这一轮结束之后、下一轮开始时它才会读到。",
    }
}

/// 模型给的入参 → 三样东西。**错误一律是给模型看的文本**（跟 `spawn_tool::parse`
/// 一致）：入参是模型写的，写错了该让它自己看见并改（003 的哲学）。
pub(crate) fn parse(input: &Value) -> Result<(AgentId, Arc<str>, Deliver), String> {
    let to = match input.get("to") {
        Some(Value::String(s)) if !s.trim().is_empty() => AgentId::new(s.trim()),
        Some(Value::String(_)) => return Err("send 失败：to 是空的。".to_string()),
        None | Some(Value::Null) => {
            return Err("send 失败：缺少必填参数 to（收信人的 agent id，比如 root/a1）。\
                        用 srv:agent/status 看整棵树就能拿到。"
                .to_string());
        }
        Some(_) => {
            return Err("send 失败：to 得是字符串（agent 的路径 id，比如 root/a1）。".to_string());
        }
    };
    let text = match input.get("text") {
        Some(Value::String(s)) if !s.trim().is_empty() => Arc::from(s.as_str()),
        Some(Value::String(_)) => {
            return Err("send 失败：text 是空的——空话进对方历史只占一格 token。".to_string());
        }
        None | Some(Value::Null) => {
            return Err("send 失败：缺少必填参数 text（要说的话）。".to_string());
        }
        Some(_) => return Err("send 失败：text 得是字符串。".to_string()),
    };
    let when = match input.get("when") {
        None | Some(Value::Null) => Deliver::Now,
        Some(Value::String(s)) if s.trim() == "now" => Deliver::Now,
        Some(Value::String(s)) if s.trim() == "next_turn" => Deliver::NextTurn,
        Some(_) => {
            return Err(
                "send 失败：when 只能是 \"now\"（加入对方本轮 loop）或 \"next_turn\"\
                 （这一轮结束后送达，且只能发给 root）。省略 = now。"
                    .to_string(),
            );
        }
    };
    Ok((to, text, when))
}

/// core 的拒绝 → 给模型看的话。
///
/// **每一条都要给出下一步**，不是只说「不行」——照 `status_tool::not_live` /
/// `collect_tool::not_collectable` 的既有写法。模型拿不到下一步就只会换个写法再撞
/// 一次。
fn explain(denied: &DeliverDenied, session: &Session) -> String {
    match denied {
        DeliverDenied::EmptyText => "send 失败：text 是空的。".to_string(),
        DeliverDenied::ToYourself { .. } => {
            "send 失败：不能发给自己。要给自己记点东西用 srv:agent/notes。".to_string()
        }
        DeliverDenied::NotInSession { target } => format!(
            "send 失败：{} 不在这个会话里。{}",
            target.as_str(),
            you_can_send_to(session),
        ),
        DeliverDenied::TargetNotLive { target } => format!(
            "send 失败：{} 已经不在活 agent 里了——没 spawn 出来过，或者它那一轮\
             已经被撤销/拆掉了。{}",
            target.as_str(),
            you_can_send_to(session),
        ),
        DeliverDenied::SenderNotLive { .. } => {
            "send 失败：你自己已经不在活 agent 里了。".to_string()
        }
        DeliverDenied::NextTurnMustTargetRoot { target, root } => format!(
            "send 失败：when=\"next_turn\" 只能发给 {}（最上面那个）。\
             子 agent 活不到下一轮——{} 到时候已经被拆掉了，这条话没人读得到。\
             **要留话就留给 {}**；要现在就说给 {} 听，把 when 改成 \"now\"。",
            root.as_str(),
            target.as_str(),
            root.as_str(),
            target.as_str(),
        ),
    }
}

/// 拒绝文本的后半句。它也进 prompt，所以顺序自己排一次（红线 11）——**不借
/// `live_agents()` 的排序承诺**，那是被调方的文档，它改了坏的是这段字节。
fn you_can_send_to(session: &Session) -> String {
    let mut live = session.live_agents();
    live.sort();
    if live.is_empty() {
        return "这个会话现在一个活 agent 都没有。".to_string();
    }
    format!(
        "现在能发的是：{}。用 srv:agent/status 看它们在干啥。",
        live.iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join("、"),
    )
}
