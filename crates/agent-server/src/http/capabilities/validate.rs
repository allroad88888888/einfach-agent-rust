//! 宿主声明里每个名字的校验：**白名单 + 拒绝，绝不 sanitize**。纯函数、零 IO——
//! [`validate`] 只吃一份已经解析好的 [`Capabilities`]，不碰文件系统、不碰 registry，
//! 所以这一层能被单独测透（issue 061 §范围条款 3）。
//!
//! # 为什么是拒绝，不是改写
//!
//! 同 055 的 chatid（[`crate::http::routes::sessions::create`] 安全点一）：悄悄把
//! `web:a b` 洗成 `web:a_b`，两个本来不同的声明就撞成了同一个工具名——后一个静默
//! 盖掉前一个，模型调到哪一个取决于数组顺序。**静默串工具比拒绝更坏。** 同一条理由
//! 也是「重名不做后来居上」的理由：宿主自己都没想清楚要哪个，server 替它选一个只会
//! 把问题推到运行时。
//!
//! # 规则表
//!
//! | 项 | 规则 |
//! |---|---|
//! | 工具名前缀 | **必须** `web:` 或 `desk:`；`srv:`/`mcp:`/无前缀一律拒 |
//! | 工具名前缀之后 | 非空、只许 `[A-Za-z0-9_/-]`、全名 ≤ 128 字节 |
//! | skill 自带的工具 | 跟顶层工具**同一条规则**（激活后进的是同一张工具表） |
//! | skill id | 非空、只许 `[A-Za-z0-9_-]`、≤ 128 字节 |
//! | 重名 | 整份声明里工具名全局唯一、skill id 唯一，撞了就拒 |
//!
//! **前缀为什么只许 `web:`/`desk:`**：位置从前缀推是既有规则（`agent_runtime` 的
//! `location_of`），而注入进来的工具本来就跑在宿主侧——用这两个既有前缀就等于直接
//! 接上「服务端推 `ToolExecuting` → 宿主 `POST /tool_result`」那条已经通了的远程
//! 工具通道，零新代码。`srv:` 是「服务端进程内执行」，宿主的工具标成 `srv:` 会让
//! dispatch 去本进程里找一个根本不存在的实现；`mcp:` 同理（`location_of` 把它判成
//! `Server`）。前端自己连的 MCP 该叫 `web:mcp-<server>/<tool>`
//! （HOST-CAPABILITIES.md §七）。
//!
//! **skill id 为什么不许 `/` 和 `:`**：它每行一个进常驻索引（`<id>: <描述>`），也是
//! 模型调 `srv:skill/activate` 时原样写回来的参数。字符集收到跟 chatid 同一档，索引
//! 那一行就不可能被 id 里的分隔符或换行撑破。

use std::collections::BTreeSet;
use std::fmt;

use super::{Capabilities, CapabilityTool};

/// 工具名合法前缀的白名单——这里是唯一一处，加第三个前缀改这一行。
const TOOL_PREFIXES: [&str; 2] = ["web:", "desk:"];
const MAX_TOOL_NAME_LEN: usize = 128;
const MAX_SKILL_ID_LEN: usize = 128;
/// 错误文案里回显名字时的上限：说得清「是哪一项」，又不至于让错误响应变成一面
/// 把任意长的请求体原样弹回去的镜子（同 [`crate::http::json`] 的取舍）。
const ECHO_LIMIT: usize = 64;

/// 这个名字是在哪儿声明的——错误文案要能直接指到那一项。
#[derive(Clone, PartialEq, Eq, Debug)]
pub(in crate::http) enum Origin {
    /// `capabilities.tools` 里。
    TopLevel,
    /// `capabilities.skills[..].tools` 里，带上那个 skill 的 id。
    Skill(String),
}

/// 一份声明为什么被拒。**结构化**：调用方拿到的是「哪一项 + 为什么」，不是一句
/// 「请求体不合法」。`Display` 就是回给客户端的那段文案。
#[derive(Clone, PartialEq, Eq, Debug)]
pub(in crate::http) enum CapabilityRejection {
    ToolPrefix { origin: Origin, name: String },
    ToolNameShape { origin: Origin, name: String },
    DuplicateTool { origin: Origin, name: String },
    SkillIdShape { id: String },
    DuplicateSkill { id: String },
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::TopLevel => write!(f, "capabilities.tools"),
            Origin::Skill(id) => write!(f, "skill \"{}\" 自带的 tools", elide(id)),
        }
    }
}

impl fmt::Display for CapabilityRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapabilityRejection::ToolPrefix { origin, name } => write!(
                f,
                "{origin} 里的工具名 \"{name}\" 必须以 \"web:\" 或 \"desk:\" 开头——注入的工具跑在宿主侧，位置从前缀推；\"srv:\"/\"mcp:\" 是服务端执行的前缀，不接受"
            ),
            CapabilityRejection::ToolNameShape { origin, name } => write!(
                f,
                "{origin} 里的工具名 \"{name}\" 前缀之后只能是 ASCII 字母、数字、连字符、下划线和斜杠，且不能为空，全名最多 {MAX_TOOL_NAME_LEN} 字节"
            ),
            CapabilityRejection::DuplicateTool { origin, name } => write!(
                f,
                "工具名 \"{name}\" 被重复声明（这一次在 {origin}）——重名一律拒绝，不做「后来居上」"
            ),
            CapabilityRejection::SkillIdShape { id } => write!(
                f,
                "skill id \"{id}\" 只能包含 ASCII 字母、数字、连字符和下划线，且不能为空，最多 {MAX_SKILL_ID_LEN} 字节"
            ),
            CapabilityRejection::DuplicateSkill { id } => {
                write!(
                    f,
                    "skill id \"{id}\" 被重复声明——重名一律拒绝，不做「后来居上」"
                )
            }
        }
    }
}

/// 整份声明的校验。第一条违规就返回——错误是给人看的，一次说清一项即可。
///
/// 工具名的唯一性是**全局**的（顶层的 + 每个 skill 自带的放在同一个集合里判）：
/// 它们最后进的是同一张工具表，重名在哪儿发生都是同一个问题。
pub(in crate::http) fn validate(capabilities: &Capabilities) -> Result<(), CapabilityRejection> {
    let mut tool_names: BTreeSet<&str> = BTreeSet::new();
    for tool in &capabilities.tools {
        check_tool(tool, &Origin::TopLevel, &mut tool_names)?;
    }
    let mut skill_ids: BTreeSet<&str> = BTreeSet::new();
    for skill in &capabilities.skills {
        if !is_valid_skill_id(&skill.id) {
            return Err(CapabilityRejection::SkillIdShape {
                id: elide(&skill.id),
            });
        }
        if !skill_ids.insert(skill.id.as_str()) {
            return Err(CapabilityRejection::DuplicateSkill {
                id: elide(&skill.id),
            });
        }
        let origin = Origin::Skill(skill.id.clone());
        for tool in &skill.tools {
            check_tool(tool, &origin, &mut tool_names)?;
        }
    }
    Ok(())
}

/// 一个工具名：前缀 → 字符集/长度 → 重名。`seen` 借的是 `tool.name` 本身，
/// 不复制字符串。
fn check_tool<'a>(
    tool: &'a CapabilityTool,
    origin: &Origin,
    seen: &mut BTreeSet<&'a str>,
) -> Result<(), CapabilityRejection> {
    let name = tool.name.as_str();
    let Some(rest) = TOOL_PREFIXES
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
    else {
        return Err(CapabilityRejection::ToolPrefix {
            origin: origin.clone(),
            name: elide(name),
        });
    };
    let shape_ok = !rest.is_empty()
        && name.len() <= MAX_TOOL_NAME_LEN
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'/'));
    if !shape_ok {
        return Err(CapabilityRejection::ToolNameShape {
            origin: origin.clone(),
            name: elide(name),
        });
    }
    if !seen.insert(name) {
        return Err(CapabilityRejection::DuplicateTool {
            origin: origin.clone(),
            name: elide(name),
        });
    }
    Ok(())
}

/// 白名单：非空、`[A-Za-z0-9_-]`、≤128 字节——跟 055 的 chatid 同一档（点号、
/// 斜杠、冒号、换行、非 ASCII 一起被同一条规则挡掉）。
fn is_valid_skill_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SKILL_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// 回显用的截断（按字符边界，不切碎 UTF-8）。076 的
/// [`builtin_switch`](super::builtin_switch) 也用它——「错误文案不做任意长度的
/// 镜子」这条取舍只该有一处实现。
pub(super) fn elide(text: &str) -> String {
    match text.char_indices().nth(ECHO_LIMIT) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn caps(value: serde_json::Value) -> Capabilities {
        serde_json::from_value(value).expect("该解析成功")
    }

    fn tools(names: &[&str]) -> Capabilities {
        caps(json!({ "tools": names.iter().map(|n| json!({ "name": n })).collect::<Vec<_>>() }))
    }

    /// 合法的两种前缀 + 允许的字符集全过。
    #[test]
    fn host_side_prefixes_are_accepted() {
        let ok = tools(&[
            "web:crm/lookup",
            "desk:clipboard/write",
            "web:mcp-figma/get_file",
            "web:a_b-c/d/e",
            "desk:X9",
        ]);
        assert_eq!(validate(&ok), Ok(()));
    }

    /// 服务端前缀、无前缀、空名——一律拒，且拒的是「前缀」这一条。
    #[test]
    fn server_side_and_prefixless_names_are_rejected() {
        for name in [
            "srv:x/y",
            "mcp:everything/echo",
            "nopfx",
            "",
            "web/x",
            "WEB:x",
            " web:x",
        ] {
            assert_eq!(
                validate(&tools(&[name])),
                Err(CapabilityRejection::ToolPrefix {
                    origin: Origin::TopLevel,
                    name: name.to_string()
                }),
                "{name:?} 该因为前缀被拒"
            );
        }
    }

    /// 前缀对了，前缀之后的部分照样要过白名单：空、空格、冒号、点、非 ASCII、超长。
    #[test]
    fn the_part_after_the_prefix_is_whitelisted() {
        let too_long = format!("web:{}", "a".repeat(MAX_TOOL_NAME_LEN));
        for name in [
            "web:",
            "desk:",
            "web:a b",
            "web:a:b",
            "web:a.b",
            "web:客户",
            "web:a\nb",
            &too_long,
        ] {
            assert!(
                matches!(
                    validate(&tools(&[name])),
                    Err(CapabilityRejection::ToolNameShape { .. })
                ),
                "{name:?} 该因为字符集/长度被拒"
            );
        }
        // 边界：正好 128 字节合法。
        let exactly_max = format!("web:{}", "a".repeat(MAX_TOOL_NAME_LEN - 4));
        assert_eq!(validate(&tools(&[&exactly_max])), Ok(()));
    }

    /// **最容易漏的一条**：skill 自带的工具过同一条校验。
    #[test]
    fn tools_carried_by_a_skill_go_through_the_same_check() {
        let declaration = caps(json!({
            "skills": [ { "id": "crm-flow", "tools": [ { "name": "srv:crm/lookup" } ] } ]
        }));
        assert_eq!(
            validate(&declaration),
            Err(CapabilityRejection::ToolPrefix {
                origin: Origin::Skill("crm-flow".to_string()),
                name: "srv:crm/lookup".to_string(),
            })
        );
        let shape =
            caps(json!({ "skills": [ { "id": "crm-flow", "tools": [ { "name": "web:a b" } ] } ] }));
        assert!(matches!(
            validate(&shape),
            Err(CapabilityRejection::ToolNameShape { .. })
        ));
    }

    /// 重名：顶层内部、skill 之间、以及 skill 与顶层之间——工具名在整份声明里全局唯一。
    #[test]
    fn duplicate_tool_names_are_rejected_everywhere() {
        assert_eq!(
            validate(&tools(&["web:a/b", "web:a/b"])),
            Err(CapabilityRejection::DuplicateTool {
                origin: Origin::TopLevel,
                name: "web:a/b".to_string()
            })
        );
        let across = caps(json!({
            "tools": [ { "name": "web:a/b" } ],
            "skills": [ { "id": "s1", "tools": [ { "name": "web:a/b" } ] } ]
        }));
        assert_eq!(
            validate(&across),
            Err(CapabilityRejection::DuplicateTool {
                origin: Origin::Skill("s1".to_string()),
                name: "web:a/b".to_string()
            })
        );
        let between_skills = caps(json!({
            "skills": [
                { "id": "s1", "tools": [ { "name": "web:a/b" } ] },
                { "id": "s2", "tools": [ { "name": "web:a/b" } ] }
            ]
        }));
        assert!(matches!(
            validate(&between_skills),
            Err(CapabilityRejection::DuplicateTool { .. })
        ));
    }

    /// skill id 的字符集与重名。
    #[test]
    fn skill_ids_are_whitelisted_and_unique() {
        for id in ["", "a/b", "a:b", "a.b", "客户", "a b"] {
            assert_eq!(
                validate(&caps(json!({ "skills": [ { "id": id } ] }))),
                Err(CapabilityRejection::SkillIdShape { id: id.to_string() }),
                "{id:?} 该被拒"
            );
        }
        assert_eq!(
            validate(&caps(json!({ "skills": [ { "id": "crm-flow_2" } ] }))),
            Ok(())
        );
        assert_eq!(
            validate(&caps(
                json!({ "skills": [ { "id": "s1" }, { "id": "s1" } ] })
            )),
            Err(CapabilityRejection::DuplicateSkill {
                id: "s1".to_string()
            })
        );
    }

    /// 空声明合法——不声明和声明空数组是一回事。
    #[test]
    fn an_empty_declaration_is_valid() {
        assert_eq!(validate(&Capabilities::default()), Ok(()));
        assert_eq!(
            validate(&caps(json!({ "tools": [], "skills": [] }))),
            Ok(())
        );
    }

    /// 错误文案要说得清「哪一项、为什么」，且**不原样回显任意长的输入**。
    #[test]
    fn the_message_names_the_offending_item_and_stays_bounded() {
        let message = validate(&tools(&["srv:crm/lookup"]))
            .unwrap_err()
            .to_string();
        assert!(message.contains("capabilities.tools"), "{message}");
        assert!(message.contains("srv:crm/lookup"), "{message}");
        assert!(message.contains("web:"), "{message}");

        let huge = format!("srv:{}", "x".repeat(10_000));
        let message = validate(&tools(&[&huge])).unwrap_err().to_string();
        assert!(
            message.len() < 400,
            "错误文案不该把请求体原样弹回去：{} 字节",
            message.len()
        );
        assert!(message.contains('…'), "截断该留个记号：{message}");
    }
}
