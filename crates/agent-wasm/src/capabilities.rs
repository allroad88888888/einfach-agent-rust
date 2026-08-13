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
/// 与 `capabilities.tools` 的工具名同一条形状上限（156 收紧时的拍板：两处最后
/// 都成为表里的名字，同名不同判是说不清的面——server 侧在 `validate_prefix.rs`，
/// 这里是它的 wasm 镜像，判定一字不差、文案各自贴宿主）。
const MAX_PREFIX_NAME_LEN: usize = 128;
const PREFIX_ALLOWED: [&str; 2] = ["web:", "desk:"];

/// 已验证、可交给新会话记录的能力。三组数据必须一起来自同一段 JSON，避免工具、
/// skill 与开局块在恢复时走到不同来源。
#[derive(Debug)]
pub(crate) struct Capabilities {
    pub(crate) tools: Vec<(ToolSpec, Reversibility)>,
    pub(crate) skills: Vec<HostSkill>,
    /// 决策 31（157）：页面声明的开局块 `(name, text)`。装表时经
    /// `ToolTable::with_host_prefix` 合成常量文本 timed 工具。
    pub(crate) prefix: Vec<(Arc<str>, Arc<str>)>,
}

/// 解析可选声明。旧的 `{"tools":[...]}` 形状仍完全兼容；新增的 `skills`、
/// `prefix` 可与它共存。省略全部字段和传 `None` 都是空能力。
pub(crate) fn parse(json: Option<&str>) -> Result<Capabilities, String> {
    let Some(json) = json else {
        return Ok(empty());
    };
    let raw: RawCapabilities = serde_json::from_str(json).map_err(|_| malformed_message())?;
    let tools = tools::declare(Some(json))?;
    let prefix = parse_prefix(raw.prefix, &tools)?;
    Ok(Capabilities {
        tools,
        skills: parse_skills(raw.skills)?,
        prefix,
    })
}

fn empty() -> Capabilities {
    Capabilities {
        tools: Vec::new(),
        skills: Vec::new(),
        prefix: Vec::new(),
    }
}

/// `prefix` 的四条校验，判定与 server 的 `validate_prefix.rs` 一字不差：
/// ①前缀必须 `web:`/`desk:`；②前缀之后的本体过工具名同款白名单（非空、
/// `[A-Za-z0-9_/-]`、全名 ≤128）；③声明内部、以及与 `capabilities.tools`
/// 重名拒；④`text` 空拒（声明常量空文本只能是笔误，069 判据）。
fn parse_prefix(
    prefix: Vec<RawPrefix>,
    tools: &[(ToolSpec, Reversibility)],
) -> Result<Vec<(Arc<str>, Arc<str>)>, String> {
    let mut seen: BTreeSet<String> = tools.iter().map(|(spec, _)| spec.name.to_string()).collect();
    let mut declared = Vec::with_capacity(prefix.len());
    for block in prefix {
        let name = block.name.as_str();
        let Some(rest) = PREFIX_ALLOWED.iter().find_map(|p| name.strip_prefix(p)) else {
            return Err(format!(
                "prefix 里的名字 \"{}\" 必须以 \"web:\" 或 \"desk:\" 开头（跟 tools 同一条前缀规则）",
                elide(name)
            ));
        };
        let shape_ok = !rest.is_empty()
            && name.len() <= MAX_PREFIX_NAME_LEN
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'/'));
        if !shape_ok {
            return Err(format!(
                "prefix 里的名字 \"{}\" 前缀之后只能是 ASCII 字母、数字、连字符、下划线和斜杠，且不能为空，全名最多 {MAX_PREFIX_NAME_LEN} 字节——跟 tools 的工具名同一条形状规则",
                elide(name)
            ));
        }
        if !seen.insert(name.to_string()) {
            return Err(format!(
                "prefix 里的名字 \"{}\" 与另一项重名（prefix 内部或 tools 里的工具）——重名一律拒绝",
                elide(name)
            ));
        }
        if block.text.is_empty() {
            return Err(format!(
                "prefix 里 \"{}\" 的 text 是空字符串——想不要这个开局块就别声明它",
                elide(name)
            ));
        }
        declared.push((Arc::<str>::from(name), Arc::<str>::from(block.text.as_str())));
    }
    Ok(declared)
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
    "能力声明 JSON 解析失败：需要 {\"tools\":[…],\"skills\":[{\"id\":\"…\",\"description\":\"…\",\"body\":\"…\"}],\"prefix\":[{\"name\":\"web:…\",\"text\":\"…\"}]}".to_string()
}

#[derive(Deserialize)]
struct RawCapabilities {
    #[serde(default)]
    skills: Vec<RawSkill>,
    #[serde(default)]
    prefix: Vec<RawPrefix>,
}

#[derive(Deserialize)]
struct RawPrefix {
    #[serde(default)]
    name: String,
    #[serde(default)]
    text: String,
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

    #[test]
    fn prefix_blocks_are_accepted_alongside_tools_and_skills() {
        let declaration = parse(Some(
            r#"{"tools":[{"name":"web:crm/read"}],"prefix":[{"name":"web:ops/briefing","text":"今天的简报"}]}"#,
        ))
        .expect("合法 prefix 应接受");
        assert_eq!(declaration.prefix.len(), 1);
        assert_eq!(&*declaration.prefix[0].0, "web:ops/briefing");
        assert_eq!(&*declaration.prefix[0].1, "今天的简报");
    }

    /// 四条拒绝路各踩一遍——判定与 server 的 `validate_prefix.rs` 一字不差
    /// （`"web:/"` 合法也一并钉住：斜杠在白名单内，两边一致）。
    #[test]
    fn prefix_rejections_mirror_the_server_rules() {
        for (json, needle) in [
            (r#"{"prefix":[{"name":"srv:x/y","text":"t"}]}"#, "web:"),
            (r#"{"prefix":[{"name":"web:","text":"t"}]}"#, "形状"),
            (r#"{"prefix":[{"name":"web:a b","text":"t"}]}"#, "形状"),
            (
                r#"{"prefix":[{"name":"web:a/b","text":"一"},{"name":"web:a/b","text":"二"}]}"#,
                "重名",
            ),
            (
                r#"{"tools":[{"name":"web:a/b"}],"prefix":[{"name":"web:a/b","text":"t"}]}"#,
                "重名",
            ),
            (r#"{"prefix":[{"name":"web:ops/brief","text":""}]}"#, "空字符串"),
        ] {
            let error = parse(Some(json)).expect_err(json);
            assert!(error.contains(needle), "{json} 的错误该点名「{needle}」：{error}");
        }
        let slash = parse(Some(r#"{"prefix":[{"name":"web:/","text":"t"}]}"#));
        assert!(slash.is_ok(), "斜杠本体在白名单内，与 tools 一致合法");
    }
}
