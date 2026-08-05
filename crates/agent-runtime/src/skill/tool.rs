//! `srv:skill/activate` / `srv:skill/deactivate`：模型用来按需装载/卸载一个 skill 的
//! 两个工具（决策 21）。
//!
//! 跟 `srv:agent/spawn` 同款：它们**改的是会话状态**（写 `Slot::SkillsActive`），
//! 而 `ToolExecutor` 够不着 `Session`——所以执行点是宿主侧 dispatch 里的一次
//! **截获**（按工具名 match，宿主本来就持有工具表，不是模型相关判断，红线 12 不管
//! 这一层）。声明进 `ToolTable::with_skills`，registry 也在那里被拥有，供这里查。
//!
//! 激活是 [`Reversibility::Reversible`](agent_core::Reversibility)：补偿动作就是
//! `srv:skill/deactivate`，而且激活走 command 层、journaled——`/undo` 连激活一起
//! 退掉是白拿的，不需要给 skill 写专门的 undo 代码。截获路径**不**登记
//! `mark_irreversible`（跟 spawn 一样），所以它不会在日志上留下屏障位。

use std::sync::Arc;

use agent_core::{AgentId, Epoch, Event, Session, SkillError, SkillId, ToolCallId, ToolSpec};
use serde_json::{Value, json};

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::persist;

/// `srv:` = 服务端本地执行（docs/TOOLS.md 命名约定）。
pub const SKILL_ACTIVATE: &str = "srv:skill/activate";
pub const SKILL_DEACTIVATE: &str = "srv:skill/deactivate";

/// 激活工具的声明。
pub fn activate_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(SKILL_ACTIVATE),
        description: Arc::from(
            "装载一个 skill：把它的说明加进你的上下文，它带的工具也随之可用。\
             可用的 skill 见 system 里的 skill 索引（每行「id: 描述」）。\
             什么时候用：当前任务命中某个 skill 的描述时——先激活它，再照它的说明做。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "skill": { "type": "string", "description": "要激活的 skill 的 id（索引里每行冒号前那个）。" }
            },
            "required": ["skill"]
        })),
    }
}

/// 停用工具的声明。
pub fn deactivate_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(SKILL_DEACTIVATE),
        description: Arc::from(
            "卸载一个之前激活的 skill：它的说明和工具从你的上下文里移出。\
             任务不再需要它时用，省下上下文预算。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "skill": { "type": "string", "description": "要停用的 skill 的 id。" }
            },
            "required": ["skill"]
        })),
    }
}

/// 截获一次 `srv:skill/activate` 或 `srv:skill/deactivate`。
///
/// 成功 → 一条普通的成功 `tool_result`（模型据此接着干活，下一跳请求就带上注入）；
/// 失败（缺参数、skill 不存在、已激活/未激活……）→ `is_error` 的 `tool_result`
/// 喂回模型让它自己收敛（决策 20 的哲学，跟 spawn 一致），不 panic、不卡住这一轮。
pub(crate) fn intercept(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    tool: &str,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    let request = ctx.tools.snapshot(tool, Arc::clone(input));
    ctx.emit(
        agent,
        RunnerEvent::ToolExecuting {
            call_id: call_id.clone(),
            request,
        },
    );

    let activating = tool == SKILL_ACTIVATE;
    let outcome = run(session, ctx, agent, activating, input);
    match outcome {
        Ok(message) => {
            // 激活/停用是一条命令，落了一条 `Entry`——跟 spawn 一样立刻同步进持久化
            // 后端，否则进程在这之后崩溃，恢复出来的会话里激活集会对不上。
            persist::sync(ctx, session);
            ctx.emit(
                agent,
                RunnerEvent::ToolExecuted {
                    call_id: call_id.clone(),
                    tool: Arc::from(tool),
                    output_len: message.len(),
                    is_error: false,
                },
            );
            Dispatched::Event(Event::ToolResult {
                agent: agent.clone(),
                epoch,
                call_id,
                content: Arc::from(message),
            })
        }
        Err(message) => {
            ctx.emit(
                agent,
                RunnerEvent::ToolExecuted {
                    call_id: call_id.clone(),
                    tool: Arc::from(tool),
                    output_len: message.len(),
                    is_error: true,
                },
            );
            Dispatched::Event(Event::ToolFailed {
                agent: agent.clone(),
                epoch,
                call_id,
                error: Arc::from(message),
            })
        }
    }
}

/// 干活本体：解析 id → （激活时）查 registry 有没有 → 调 command → 成功/失败文本。
fn run(
    session: &mut Session,
    ctx: &RunnerCtx,
    agent: &AgentId,
    activating: bool,
    input: &Value,
) -> Result<String, String> {
    let id = parse_skill(input)?;
    let skill = SkillId::new(&*id);

    if activating {
        // 激活一个 registry 里没有的 id：如实回报 + 列出有哪些，让模型自己收敛。
        if !ctx.tools.skill_registry().contains(&id) {
            let known = ctx.tools.skill_registry().known_ids().join("、");
            return Err(format!(
                "激活失败：没有叫「{id}」的 skill。可用的是：{}。",
                if known.is_empty() {
                    "（当前没有装载任何 skill）".to_string()
                } else {
                    known
                }
            ));
        }
        session
            .activate_skill(agent, skill)
            .map(|()| format!("已激活 skill「{id}」：它的说明和携带的工具现在可用。"))
            .map_err(|e| refusal(&e))
    } else {
        session
            .deactivate_skill(agent, skill)
            .map(|()| format!("已停用 skill「{id}」。"))
            .map_err(|e| refusal(&e))
    }
}

/// 从入参里取 `skill`。**错误一律是给模型看的文本**（跟 spawn 的 `parse` 一致）。
fn parse_skill(input: &Value) -> Result<Arc<str>, String> {
    let Some(id) = input.get("skill").and_then(Value::as_str) else {
        return Err("skill 工具失败：缺少必填参数 skill（字符串，skill 的 id）。".to_string());
    };
    if id.trim().is_empty() {
        return Err("skill 工具失败：skill 是空的。".to_string());
    }
    Ok(Arc::from(id))
}

/// [`SkillError`] → 给模型看的一句话。
fn refusal(err: &SkillError) -> String {
    match err {
        SkillError::AlreadyActive { skill, .. } => {
            format!("「{}」已经是激活状态了，不用再激活一次。", skill.as_str())
        }
        SkillError::NotActive { skill, .. } => {
            format!("「{}」本来就没激活，无从停用。", skill.as_str())
        }
        // 下面两条是宿主侧的异常（agent 不在树上/不活着），不是模型能收敛的——照样
        // 如实回给它，让这一轮有个结果而不是卡住。
        SkillError::NotInSession { agent } => {
            format!(
                "skill 工具失败：agent（{}）不在这个会话的 agent 树上。",
                agent.as_str()
            )
        }
        SkillError::NotLive { agent } => {
            format!(
                "skill 工具失败：agent（{}）已经不在活名单上了。",
                agent.as_str()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_is_required_and_must_not_be_blank() {
        assert!(parse_skill(&json!({})).is_err());
        assert!(parse_skill(&json!({ "skill": "  " })).is_err());
        assert_eq!(&*parse_skill(&json!({ "skill": "foo" })).unwrap(), "foo");
    }

    #[test]
    fn refusal_texts_name_the_skill() {
        let e = SkillError::AlreadyActive {
            agent: AgentId::root(),
            skill: SkillId::new("foo"),
        };
        assert!(refusal(&e).contains("foo"));
    }
}
