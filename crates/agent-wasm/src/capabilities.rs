//! 页面传进来的能力声明 JSON → 会话可持久化的 host tool / skill 料。
//!
//! 只管 browser `AgentHost` 的协议入口：顶层 `tools` 继续复用 runtime 的既有
//! 校验，`skills` 对齐 server 的能力协议。装表与写 journal 分别由 `tools`、
//! `assemble` 负责，避免一份输入在三个地方各自解释。

use std::collections::BTreeSet;
use std::sync::Arc;

use agent_core::{HostSkill, Reversibility, SkillId, ToolSpec};
use serde::Deserialize;

use crate::tools;

const MAX_SKILL_ID_LEN: usize = 128;

/// 已验证、可交给新会话记录的能力。两组数据必须一起来自同一段 JSON，避免工具与
/// skill 在恢复时走到不同来源。
#[derive(Debug)]
pub(crate) struct Capabilities {
    pub(crate) tools: Vec<(ToolSpec, Reversibility)>,
    pub(crate) skills: Vec<HostSkill>,
}

/// 解析可选声明。旧的 `{"tools":[...]}` 形状仍完全兼容；新增的 `skills` 可与它
/// 共存。省略全部字段和传 `None` 都是空能力。
pub(crate) fn parse(json: Option<&str>) -> Result<Capabilities, String> {
    let Some(json) = json else {
        return Ok(empty());
    };
    let raw: RawCapabilities = serde_json::from_str(json).map_err(|_| malformed_message())?;
    let tools = tools::declare(Some(json))?;
    Ok(Capabilities {
        tools,
        skills: parse_skills(raw.skills)?,
    })
}

fn empty() -> Capabilities {
    Capabilities {
        tools: Vec::new(),
        skills: Vec::new(),
    }
}

fn parse_skills(skills: Vec<RawSkill>) -> Result<Vec<HostSkill>, String> {
    let mut ids = BTreeSet::new();
    let mut declared = Vec::with_capacity(skills.len());
    for skill in skills {
        if !valid_skill_id(&skill.id) {
            return Err(format!(
                "skill id \"{}\" 只能包含 ASCII 字母、数字、连字符和下划线，且不能为空，最多 {MAX_SKILL_ID_LEN} 字节",
                elide(&skill.id)
            ));
        }
        if !ids.insert(skill.id.clone()) {
            return Err(format!(
                "skill id \"{}\" 被重复声明——重名一律拒绝，不做「后来居上」",
                elide(&skill.id)
            ));
        }
        if !skill.tools.is_empty() {
            return Err(format!(
                "skill \"{}\" 带了 tools——v1 不支持 skill 携带工具，工具请经 capabilities.tools 声明",
                elide(&skill.id)
            ));
        }
        declared.push(HostSkill {
            id: SkillId::new(skill.id.as_str()),
            description: Arc::from(skill.description),
            body: Arc::from(skill.body),
            tools: Vec::new(),
            tool_reversibility: Default::default(),
        });
    }
    Ok(declared)
}

fn valid_skill_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SKILL_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn elide(text: &str) -> String {
    match text.char_indices().nth(64) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

fn malformed_message() -> String {
    "能力声明 JSON 解析失败：需要 {\"tools\":[…],\"skills\":[{\"id\":\"…\",\"description\":\"…\",\"body\":\"…\"}]}".to_string()
}

#[derive(Deserialize)]
struct RawCapabilities {
    #[serde(default)]
    skills: Vec<RawSkill>,
}

#[derive(Deserialize)]
struct RawSkill {
    #[serde(default)]
    id: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    tools: Vec<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_only_declaration_stays_compatible() {
        let declaration =
            parse(Some(r#"{"tools":[{"name":"web:crm/read"}]}"#)).expect("旧工具声明仍应接受");
        assert_eq!(declaration.tools.len(), 1);
        assert!(declaration.skills.is_empty());
    }

    #[test]
    fn skills_are_converted_without_embedded_tools() {
        let declaration = parse(Some(
            r#"{"skills":[{"id":"crm-flow","description":"处理工单","body":"先查询工单"}]}"#,
        ))
        .expect("合法 skill 应接受");
        assert!(declaration.tools.is_empty());
        assert_eq!(declaration.skills[0].id.as_str(), "crm-flow");
        assert_eq!(&*declaration.skills[0].body, "先查询工单");
        assert!(declaration.skills[0].tools.is_empty());
    }

    #[test]
    fn skill_embedded_tools_are_rejected() {
        let error = parse(Some(r#"{"skills":[{"id":"crm","tools":[{}]}]}"#))
            .expect_err("v1 不支持 skill 携带工具");
        assert!(error.contains("v1 不支持"));
    }
}
