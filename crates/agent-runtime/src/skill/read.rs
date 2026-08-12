//! `srv:skill/read`：正文按需读（137，决策 27）。
//!
//! 索引（138）只给模型 id + description，正文要靠这个工具按 id 现取。registry
//! 归 `ToolTable` 拥有，`ToolExecutor` 够不着它，所以执行点是宿主侧 dispatch 的
//! 一次截获，不进 `ToolExecutor`。
//!
//! # 139：`with_skills` 把它放进 specs
//!
//! `ToolTable::with_skills`（`tool_table_skill.rs`）把 [`read_spec`] 追加进模型面
//! 的 specs 区，`ctx.tools.declares(SKILL_READ)` 从此为真，`dispatch.rs` 里那条
//! 截获路由不再是死代码。
//!
//! # 不碰会话状态
//!
//! 读正文不改 `Slot::SkillsActive`，甚至不需要 `Session`——`&SkillRegistry`
//! 已经是它需要的全部输入。跟 `status_tool` 同款「纯读、当场回写、无
//! Pending、无 entry 要同步」。

use std::sync::Arc;

use agent_core::{AgentId, Epoch, ToolCallId, ToolSpec};
use serde_json::{Value, json};

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::reply;

use super::SkillRegistry;

/// 工具全名。`srv:` = 服务端本地执行（docs/TOOLS.md 命名约定）。
pub const SKILL_READ: &str = "srv:skill/read";

/// 读正文工具的声明。
pub fn read_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(SKILL_READ),
        description: Arc::from(
            "按 id 读一个 skill 的正文全文。id 是 system 里 skill 索引那一行列出的\
             那个（每行「<id> — <描述>」），不是别的字符串。什么时候用：索引里的\
             描述让你判断这个 skill 跟当前任务相关，想看它完整的操作说明时调用它。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "skill": { "type": "string", "description": "要读取的 skill 的 id（索引里那一行最前面那个）。" }
            },
            "required": ["skill"]
        })),
    }
}

/// 截获一次 `srv:skill/read`。
///
/// **纯读、当场回写、无 Pending、无在飞凭据、无 entry 要同步**——跟
/// `status_tool::intercept` 同一个形状：这次调用不碰 `Session`，甚至不需要它
/// 作为参数。失败（缺参数、未知 id）→ `is_error` 的 tool_result 喂回模型
/// （决策 20），不 panic、不卡住这一轮。
pub(crate) fn intercept(
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    let request = ctx.tools.snapshot(SKILL_READ, Arc::clone(input));
    ctx.emit(
        agent,
        RunnerEvent::ToolExecuting {
            call_id: call_id.clone(),
            request,
        },
    );

    match read(ctx.tools.skill_registry(), input) {
        Ok(body) => reply::ok(ctx, agent, call_id, epoch, SKILL_READ, body),
        Err(message) => reply::refuse(ctx, agent, call_id, epoch, SKILL_READ, message),
    }
}

/// 干活本体：解析 id → registry 精确查 → 正文或拒绝文本。
///
/// **签名只拿 `&SkillRegistry`**：读取路径上没有文件系统——正文装载期已经进了
/// 内存（`Skill.body`），越界读在结构上不可能，不靠路径清洗兜底。所以
/// `"../etc/passwd"` 这类字符串走的不是「被挡下来」，是压根查不到这个 id，跟
/// 任何别的不存在的 id 是同一条路。
fn read(registry: &SkillRegistry, input: &Value) -> Result<String, String> {
    let id = parse_skill(input)?;
    registry
        .body_of(&id)
        .map(|body| body.to_string())
        .ok_or_else(|| unknown_id(&id))
}

/// 从入参里取 `skill`。**错误一律是给模型看的文本**（跟 `tool::parse_skill` 一致）。
fn parse_skill(input: &Value) -> Result<Arc<str>, String> {
    let Some(id) = input.get("skill").and_then(Value::as_str) else {
        return Err("read 失败：缺少必填参数 skill（字符串，skill 的 id）。".to_string());
    };
    if id.trim().is_empty() {
        return Err("read 失败：skill 是空的。".to_string());
    }
    Ok(Arc::from(id))
}

/// 未知 id 的拒绝文案：**指向索引，不列全量 id**——索引已经给过一遍，重复
/// 一遍是白花 token（跟 `tool.rs` 的 activate 失败文案刻意不同：那边动作后果
/// 小、列出来帮模型收敛；这里正文可能很长，堆一遍 id 列表不划算）。
fn unknown_id(id: &str) -> String {
    format!("read 失败：没有叫「{id}」的 skill。可用的 id 见 system 里的 skill 索引。")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use agent_core::SkillId;

    use super::*;
    use crate::skill::Skill;

    /// 造一个装了若干 skill 的 registry，不落磁盘（跟 `load` 无关的测试）。
    fn registry(skills: Vec<(&str, &str, &str)>) -> SkillRegistry {
        let mut map = BTreeMap::new();
        for (id, description, body) in skills {
            map.insert(
                Arc::from(id),
                Skill {
                    id: SkillId::new(id),
                    description: Arc::from(description),
                    body: Arc::from(body),
                    tools: Vec::new(),
                    hidden: false,
                },
            );
        }
        SkillRegistry { skills: map }
    }

    /// 已装载 id → 正文逐字节返回，覆盖多行 + 非 ASCII。
    #[test]
    fn known_id_returns_the_body_byte_for_byte() {
        let body = "第一行\n第二行，带 emoji 🎯\n第三行。";
        let reg = registry(vec![("foo", "一个 skill", body)]);
        let out = read(&reg, &json!({ "skill": "foo" })).unwrap();
        assert_eq!(out, body);
    }

    /// 未知 id、以及看起来像路径穿越的字符串，走同一条 `is_error` 路——没有
    /// 特殊对待，因为读取路径上压根没有文件系统。
    #[test]
    fn unknown_id_and_path_traversal_look_alike_take_the_same_error_path() {
        let reg = registry(vec![("foo", "d", "b")]);
        let a = read(&reg, &json!({ "skill": "bar" })).unwrap_err();
        let b = read(&reg, &json!({ "skill": "../etc/passwd" })).unwrap_err();
        assert!(a.contains("没有叫「bar」的 skill"));
        assert!(b.contains("没有叫「../etc/passwd」的 skill"));
        // 拒绝文案指向索引，不重复列出全量 id（索引已经给过一遍）。
        assert!(a.contains("索引"));
        assert!(!a.contains("foo"), "不该把已装载的 id 列出来陪跑");
    }

    #[test]
    fn missing_or_blank_skill_param_is_rejected() {
        let reg = registry(vec![]);
        assert!(read(&reg, &json!({})).is_err());
        assert!(read(&reg, &json!({ "skill": "  " })).is_err());
    }

    /// 142：hidden 只挡索引，不挡读——一个 hidden 的 skill 不在 `index_text()`
    /// 里，但 `read`（进而 `body_of`）照常能取到它的正文。
    #[test]
    fn a_hidden_skill_is_absent_from_the_index_but_still_readable() {
        let mut map = BTreeMap::new();
        map.insert(
            Arc::from("secret"),
            Skill {
                id: SkillId::new("secret"),
                description: Arc::from("藏起来的技能"),
                body: Arc::from("藏起来的正文"),
                tools: Vec::new(),
                hidden: true,
            },
        );
        let reg = SkillRegistry { skills: map };

        assert!(
            !reg.index_text().contains("secret"),
            "hidden 的 skill 不该出现在索引里"
        );
        assert_eq!(
            read(&reg, &json!({ "skill": "secret" })).unwrap(),
            "藏起来的正文"
        );
    }

    // `reversibility_of(SKILL_READ) == Pure` 测在 `tool_table_names.rs`
    // 自己的测试里（就近验证，不用把它的可见性拉宽到能被这里直接调用）。
}
