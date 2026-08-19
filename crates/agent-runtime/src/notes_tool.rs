//! `srv:agent/notes` + `srv:agent/notes/set`：模型自己的草稿纸（209，决策 35 §三）。
//!
//! # 它是这一波里唯一的**写**口
//!
//! 用户要「改本 agent 状态」。做法不是给现有槽位开写口——那些格子每一个都是
//! 别人的账（`MaxTurns` 是部署方的、`ToolsAllowed` 是父给的、`Summaries` 是
//! adapter 的），**给它们开写口等于让被约束者改自己的约束**。做法是给模型一个
//! 属于它自己的槽位：[`Slot::Notes`](agent_core::Slot)。
//!
//! 新槽位白拿全套机制：`/undo` 连带撤销、崩溃恢复自动带回、审计看得到每一次改。
//! **这个文件里没有一行代码认识「撤销」或「恢复」**——它只是走了 command 层
//! （红线 2），剩下的是架构直接掉出来的。
//!
//! # 只在被问的时候进 prompt
//!
//! 草稿纸**不自动注入进 system 前缀**。那会让每一次写 notes 都动 system 前缀、
//! 把前缀缓存整段打掉（红线 11 要防的那类代价的另一半）。它是模型自己要看时
//! 去查的东西，不是背景板——所以写成功的回执里明说了这一句，不然模型会以为
//! 「记下了 = 下一轮我会看见」。
//!
//! # 两道上限，两种处理
//!
//! - **条目数**撞顶：core 显式拒（[`NoteDenied::TooManyNotes`]），翻成 `is_error`
//!   的 tool_result 让模型自己收敛（决策 20 的哲学）。
//! - **单条正文**超长：这一层**截断并如实说**（照 004 的工具结果上限同款处理），
//!   截完再交给 core。截断点走 [`truncated_content_bytes`]，UTF-8 安全——按字节
//!   硬切会把一个中文字劈成半个，落进状态、进 prompt、序列化时才炸。
//! - **key** 超长：不截断，直接拒。截短的 key 是**另一个名字**，模型下一轮拿
//!   原来的名字查不到，而它记的时候明明成功了。
//!
//! # 可逆性
//!
//! 两条都是 `Aftermath::Nothing` → `Undoability::StateOnly`：纯状态，没碰外部
//! 世界。读那一条连 command 都没发。

use std::sync::Arc;

use agent_core::{
    AgentId, Epoch, NOTE_VALUE_CAP, Session, ToolCallId, ToolSpec, truncated_content_bytes,
};
use serde_json::{Value, json};

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::notes_render::{KEY_CAP, explain, removed, render, wrote};
use crate::reply;

/// 读的那一半。`srv:` = 服务端本地执行（docs/TOOLS.md 的命名约定）。
pub const NOTES_TOOL: &str = "srv:agent/notes";

/// 写的那一半。名字带 `/set` 后缀而不是给读那个加一个 `action` 参数：
/// 一个工具一件事，模型看 schema 就知道这次调用是读还是写，不用先读一遍描述
/// 才知道哪几个参数在哪种模式下有意义。
pub const NOTES_SET_TOOL: &str = "srv:agent/notes/set";

/// 读的声明。**无入参**——草稿纸是谁的由截获现场的 `AgentId` 决定。
pub fn notes_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(NOTES_TOOL),
        description: Arc::from(
            "看一眼你自己的草稿纸：你之前用 srv:agent/notes/set 记下的全部条目，\
             按 key 排。**不阻塞**，当场返回，不用填任何参数。\n\
             它**只有你看得见**——别的 agent（包括你的上级和你开出来的子 agent）\
             读不到，也写不了。\n\
             草稿纸**不会自动出现在你的对话里**，要看就得调这个工具。\
             所以适合记「过一会儿才用得上、现在说了浪费上下文」的东西：\
             中间结论、待办、你自己定下的约定。\n\
             要给别的 agent 传话用 srv:agent/send，不是这个——记在草稿纸上没人看得到。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {}
        })),
    }
}

/// 写的声明。
pub fn notes_set_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(NOTES_SET_TOOL),
        description: Arc::from(
            "往你自己的草稿纸上记一条，或者删一条。当场返回。\n\
             同一个 key 写第二次是**覆盖**，不是追加——它是一张表，不是流水账。\n\
             `value` 传 null（或者省掉）= **删掉这条**。\n\
             记下的东西只有你看得见，而且**不会自动出现在你以后的对话里**——\
             要看得调 srv:agent/notes。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": format!(
                        "这条笔记的名字，短一点（上限 {KEY_CAP} 字节）。同名会覆盖。"
                    ),
                },
                "value": {
                    "type": ["string", "null"],
                    "description": format!(
                        "要记的内容（上限 {NOTE_VALUE_CAP} 字节，超了会被截断并告诉你）。\
                         传 null 或省掉 = 删掉这条。"
                    ),
                }
            },
            "required": ["key"]
        })),
    }
}

/// 截获一次 `srv:agent/notes`（读）。
///
/// **纯读、当场回写、不调 `persist::sync`**（照 `status_tool::intercept` 的既有
/// 理由：一条 command 都没发）。入参一律忽略（schema 是空对象）。
pub(crate) fn read_intercept(
    session: &Session,
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    announce(ctx, agent, &call_id, NOTES_TOOL, input);
    let body = render(&session.notes_of(agent));
    reply::ok(ctx, agent, call_id, epoch, NOTES_TOOL, body)
}

/// 截获一次 `srv:agent/notes/set`（写）。
///
/// **不调 `persist::sync`**：`set_note` 是一条 command，它那条 entry 走命令层的
/// 常规路；真正要同步的是随后经 `Session::step` 的那条 `ToolResult`，泵在 A 段
/// 自己会转发（同 `send_tool::intercept` 的既有理由）。
pub(crate) fn set_intercept(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    announce(ctx, agent, &call_id, NOTES_SET_TOOL, input);

    let (key, value, truncated_from) = match parse(input) {
        Ok(parsed) => parsed,
        Err(message) => return reply::refuse(ctx, agent, call_id, epoch, NOTES_SET_TOOL, message),
    };
    let deleting = value.is_none();

    match session.set_note(agent, Arc::clone(&key), value) {
        Ok(()) => {
            let body = if deleting {
                removed(&key)
            } else {
                wrote(&key, truncated_from)
            };
            reply::ok(ctx, agent, call_id, epoch, NOTES_SET_TOOL, body)
        }
        Err(denied) => {
            reply::refuse(ctx, agent, call_id, epoch, NOTES_SET_TOOL, explain(&denied))
        }
    }
}

/// 两条截获共用的开场：跟别的工具一样让人看得见「调了什么、参数是什么」。
fn announce(
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: &ToolCallId,
    tool: &'static str,
    input: &Arc<Value>,
) {
    let request = ctx.tools.snapshot(tool, Arc::clone(input));
    ctx.emit(
        agent,
        RunnerEvent::ToolExecuting {
            call_id: call_id.clone(),
            request,
        },
    );
}

/// 一次写调用解析出来的三样东西：记在哪个 key 下、记什么（`None` = 删）、
/// 以及**截断前**原本有多长（`None` = 没截）。
///
/// 第三样单独留着而不是就地丢掉：回执要如实说「你给了多少、我留了多少」，
/// 而截断发生在这里、说话发生在别处。
pub(crate) type ParsedNote = (Arc<str>, Option<Arc<str>>, Option<usize>);

/// 模型给的入参 → [`ParsedNote`]。
///
/// **错误一律是给模型看的文本**（跟 `spawn_tool::parse` 一致）：入参是模型写的，
/// 写错了该让它自己看见并改（003 的哲学）。
///
/// 正文超长在这里就截断，返回原始长度让回执如实说——**不是静默截**：模型基于
/// 一段残缺的笔记下结论，比拿到一句拒绝糟得多。
pub(crate) fn parse(input: &Value) -> Result<ParsedNote, String> {
    let key: Arc<str> = match input.get("key") {
        Some(Value::String(s)) if !s.trim().is_empty() => Arc::from(s.trim()),
        Some(Value::String(_)) => return Err("记笔记失败：key 是空的。".to_string()),
        None | Some(Value::Null) => {
            return Err(
                "记笔记失败：缺少必填参数 key（这条笔记的名字，比如 \"下一步\"）。".to_string(),
            );
        }
        Some(_) => return Err("记笔记失败：key 得是字符串。".to_string()),
    };
    let (value, truncated_from) = match input.get("value") {
        // 缺省与显式 `null` 是同一件事（模型两种都会写）：删掉这条。
        None | Some(Value::Null) => (None, None),
        Some(Value::String(s)) if s.len() > NOTE_VALUE_CAP => {
            let cut = truncated_content_bytes(s, NOTE_VALUE_CAP);
            (Some(Arc::from(&s[..cut])), Some(s.len()))
        }
        Some(Value::String(s)) => (Some(Arc::from(s.as_str())), None),
        Some(_) => {
            return Err(
                "记笔记失败：value 得是字符串（或者 null——那是删掉这条的意思）。".to_string(),
            );
        }
    };
    Ok((key, value, truncated_from))
}

#[cfg(test)]
#[path = "notes_tool_tests.rs"]
mod tests;
