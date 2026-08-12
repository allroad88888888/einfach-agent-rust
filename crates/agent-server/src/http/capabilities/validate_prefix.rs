//! `capabilities.prefix` 的校验（M17，决策 31，156）——独立成文件而不是塞进
//! [`super::validate`]：那个文件已经在 300 行上限边上，而这是一批**新**规则，
//! 不是对既有 `tools`/`skills` 规则的延伸（`validate.rs` 加新逻辑前先 `wc -l`，
//! 顶破就按职责拆，是本次改动的一部分，见 CLAUDE.md 的文件行数硬规则）。
//!
//! 三条新规，全部 400 且点名（HOST-CAPABILITIES.md §八之三）：
//!
//! 1. `name` 必须 `web:`/`desk:` 前缀——跟 [`super::validate`] 里
//!    `capabilities.tools` 的名字同一条前缀规则（`TOOL_PREFIXES`），复用同一个
//!    常量，不重新拍一遍白名单。
//! 2. 声明内部重名、以及跟 `capabilities.tools` 的名字重名，一律拒——两者最后
//!    都成为表里的名字（一个模型面、一个 timed 区），同名会让 `init:<name>`
//!    这个 label 和 spawn 的 `inherit_prefix` 路由说不清。**判法**：调用方传入
//!    校验 `capabilities.tools` 时已经攒好的名字集合，这里的每一项都拿去同一个
//!    集合里 `insert`——插入失败（已经在集合里，不管是另一个 prefix 项还是一个
//!    tools 项）就是重名，两种来源用同一条判断，不必各写一遍。
//! 3. `text` 为空 → 拒。声明一段常量空文本只能是笔误（本地 timed 工具的空文本
//!    是「执行结果」语义——135，跳过不产块；这里是宿主**主动声明**的常量，语义
//!    不同，069 判据「在最早能报给作者的点上失败」）。
//!
//! **字符集/长度跟工具名同一条白名单**（主会话拍板 2026-08-12，156 独测抓出的
//! 分歧）：初版只查前缀、不查本体，理由是「决策 31 原文只列了前缀这一条」；独测
//! 黑盒探出 `"web:"`（空本体）、`"web:crm briefing"`（空格）在 tools 那边 400、
//! 在这边 201。收紧的理由有三：①这个名字要进 journaled 的 `init:<name>` label，
//! ②模型要在 `inherit_prefix` 里逐字打它，③「同一个名字在 tools 是 400、在
//! prefix 是 201」正是 HOST-CAPABILITIES §三之二点名过的「同一件事两个判法」。
//! 判定与 `capabilities.tools` **一字不差**（复用 [`super::validate`] 的
//! `MAX_TOOL_NAME_LEN` 与同一段字节白名单）：本体非空、全名 ≤128 字节、
//! `[A-Za-z0-9_/-]`——所以 `"web:/"` 两边一致合法（`/` 在白名单内），不另立规矩。

use std::collections::BTreeSet;
use std::fmt;

use super::validate::{MAX_TOOL_NAME_LEN, TOOL_PREFIXES, elide};
use super::{Capabilities, CapabilityPrefix};

/// `capabilities.prefix` 里一项为什么被拒。跟 [`super::validate::CapabilityRejection`]
/// 分开建一个类型，而不是往那个枚举里加变体——这批变体只服务这一个字段，`Display`
/// 也只需要说「capabilities.prefix」这一个来源，没有 `Origin` 那种「同一条规则、
/// 多个来源」的需要（[`super::validate::Origin`] 模块文档解释过那个类型存在的
/// 前提：140 之后它已经只剩一个变体）。[`super::validate::CapabilityRejection`]
/// 用一个 `Prefix(PrefixRejection)` 变体把这里的结果包进去，调用方仍然只有一种
/// 错误类型要处理。
#[derive(Clone, PartialEq, Eq, Debug)]
pub(in crate::http) enum PrefixRejection {
    /// `name` 没有 `web:`/`desk:` 前缀。
    Prefix { name: String },
    /// 前缀对了，但前缀之后的本体没过白名单（空 / 越界字符 / 全名超长）——
    /// 跟 `capabilities.tools` 的工具名同一条形状规则，理由见模块文档。
    NameShape { name: String },
    /// `name` 跟另一项重名——声明内部，或者跟 `capabilities.tools` 里的某个
    /// 工具名重名，两种情况报同一条（理由见模块文档第 2 条）。
    Duplicate { name: String },
    /// `text` 是空字符串。
    EmptyText { name: String },
}

impl fmt::Display for PrefixRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrefixRejection::Prefix { name } => write!(
                f,
                "capabilities.prefix 里的名字 \"{name}\" 必须以 \"web:\" 或 \"desk:\" 开头——跟 capabilities.tools 同一条前缀规则（装配期会把它合成一条开局工具，落进 timed 区）；\"srv:\" 是服务端执行前缀，结构性不会撞名，不接受"
            ),
            PrefixRejection::NameShape { name } => write!(
                f,
                "capabilities.prefix 里的名字 \"{name}\" 前缀之后只能是 ASCII 字母、数字、连字符、下划线和斜杠，且不能为空，全名最多 {MAX_TOOL_NAME_LEN} 字节——跟 capabilities.tools 的工具名同一条形状规则"
            ),
            PrefixRejection::Duplicate { name } => write!(
                f,
                "capabilities.prefix 里的名字 \"{name}\" 与另一项重名（可能在 prefix 内部，也可能是 capabilities.tools 里同名的工具）——两者最后落进同一张表的两个区（模型面 / timed 区），同名会让 init:{name} 这个 label 和 inherit_prefix 的路由说不清，一律拒绝，不做「后来居上」"
            ),
            PrefixRejection::EmptyText { name } => write!(
                f,
                "capabilities.prefix 里 \"{name}\" 的 text 是空字符串——声明一段常量空文本只能是笔误：本地开局工具的空文本会被跳过（不产前缀块），但这里是你主动声明的常量，不该有『安静地什么都不做』的这一档，想不要这个开局块就别声明它"
            ),
        }
    }
}

/// 校验 `capabilities.prefix`。`tool_names` 是调用方（[`super::validate::validate`]）
/// 校验 `capabilities.tools` 时攒好的那个集合——第 2 条重名规则要跟它比较，见
/// 模块文档。第一条违规就返回，跟 [`super::validate::validate`] 同一条「错误是
/// 给人看的，一次说清一项即可」。
pub(in crate::http) fn check_prefix<'a>(
    capabilities: &'a Capabilities,
    tool_names: &BTreeSet<&'a str>,
) -> Result<(), PrefixRejection> {
    let mut seen: BTreeSet<&'a str> = tool_names.clone();
    for prefix in &capabilities.prefix {
        check_one(prefix, &mut seen)?;
    }
    Ok(())
}

fn check_one<'a>(
    prefix: &'a CapabilityPrefix,
    seen: &mut BTreeSet<&'a str>,
) -> Result<(), PrefixRejection> {
    let name = prefix.name.as_str();
    let Some(rest) = TOOL_PREFIXES
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
    else {
        return Err(PrefixRejection::Prefix { name: elide(name) });
    };
    let shape_ok = !rest.is_empty()
        && name.len() <= MAX_TOOL_NAME_LEN
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'/'));
    if !shape_ok {
        return Err(PrefixRejection::NameShape { name: elide(name) });
    }
    if !seen.insert(name) {
        return Err(PrefixRejection::Duplicate { name: elide(name) });
    }
    if prefix.text.is_empty() {
        return Err(PrefixRejection::EmptyText { name: elide(name) });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn caps(value: serde_json::Value) -> Capabilities {
        serde_json::from_value(value).expect("该解析成功")
    }

    fn check(capabilities: &Capabilities) -> Result<(), PrefixRejection> {
        check_prefix(capabilities, &BTreeSet::new())
    }

    /// 合法的两种前缀全过。
    #[test]
    fn host_side_prefixes_are_accepted() {
        let ok = caps(json!({
            "prefix": [
                { "name": "web:crm/briefing", "text": "今天的客户上下文" },
                { "name": "desk:sop/day", "text": "今天的运维简报" }
            ]
        }));
        assert_eq!(check(&ok), Ok(()));
    }

    /// 坏前缀：服务端前缀、无前缀、空名——一律拒，且拒的是「前缀」这一条。
    #[test]
    fn a_bad_prefix_is_named_and_rejected() {
        for name in ["srv:x/y", "mcp:everything/echo", "nopfx", ""] {
            let declared = caps(json!({ "prefix": [ { "name": name, "text": "t" } ] }));
            assert_eq!(
                check(&declared),
                Err(PrefixRejection::Prefix {
                    name: name.to_string()
                }),
                "{name:?} 该因为前缀被拒"
            );
        }
    }

    /// 前缀对了，本体照样要过白名单——跟 `capabilities.tools` 的矩阵同款
    /// （156 独测抓出的分歧，收紧后两边一字不差）；`"web:/"` 合法（`/` 在
    /// 白名单内），是「两边一致」的另一半证据。
    #[test]
    fn the_part_after_the_prefix_is_whitelisted_like_a_tool_name() {
        let too_long = format!("web:{}", "a".repeat(MAX_TOOL_NAME_LEN));
        for name in ["web:", "desk:", "web:crm briefing", "web:a.b", "web:客户", &too_long] {
            let declared = caps(json!({ "prefix": [ { "name": name, "text": "t" } ] }));
            assert!(
                matches!(check(&declared), Err(PrefixRejection::NameShape { .. })),
                "{name:?} 该因为形状被拒"
            );
        }
        let slash_only = caps(json!({ "prefix": [ { "name": "web:/", "text": "t" } ] }));
        assert_eq!(check(&slash_only), Ok(()), "斜杠在白名单内，跟 tools 一致");
    }

    /// 声明内部重名。
    #[test]
    fn an_internal_duplicate_is_rejected() {
        let declared = caps(json!({
            "prefix": [
                { "name": "web:a/b", "text": "第一份" },
                { "name": "web:a/b", "text": "第二份" }
            ]
        }));
        assert_eq!(
            check(&declared),
            Err(PrefixRejection::Duplicate {
                name: "web:a/b".to_string()
            })
        );
    }

    /// 跟 `capabilities.tools` 重名——这条必须喂真实的 `tool_names` 集合，不能
    /// 走上面那个「空集合」的 `check` 便捷函数，否则测不出这条规则本身。
    #[test]
    fn a_duplicate_against_top_level_tools_is_rejected() {
        let declared = caps(json!({
            "tools": [ { "name": "web:a/b" } ],
            "prefix": [ { "name": "web:a/b", "text": "t" } ]
        }));
        let mut tool_names = BTreeSet::new();
        tool_names.insert("web:a/b");
        assert_eq!(
            check_prefix(&declared, &tool_names),
            Err(PrefixRejection::Duplicate {
                name: "web:a/b".to_string()
            })
        );
    }

    /// `text` 为空 → 拒，且点名是哪一项。
    #[test]
    fn empty_text_is_named_and_rejected() {
        let declared = caps(json!({ "prefix": [ { "name": "web:crm/briefing", "text": "" } ] }));
        assert_eq!(
            check(&declared),
            Err(PrefixRejection::EmptyText {
                name: "web:crm/briefing".to_string()
            })
        );
    }

    /// 空声明、以及不带这个字段：合法——不声明和声明空数组是一回事。
    #[test]
    fn an_empty_or_omitted_prefix_list_is_valid() {
        assert_eq!(check(&Capabilities::default()), Ok(()));
        assert_eq!(check(&caps(json!({ "prefix": [] }))), Ok(()));
    }

    /// 错误文案要点名、说清合法前缀是什么，且不原样回显任意长的输入
    /// （`elide` 复用自 `validate.rs`，同一处实现）。
    #[test]
    fn the_message_names_the_offending_item_and_stays_bounded() {
        let message = check(&caps(json!({ "prefix": [ { "name": "srv:x/y", "text": "t" } ] })))
            .unwrap_err()
            .to_string();
        assert!(message.contains("capabilities.prefix"), "{message}");
        assert!(message.contains("srv:x/y"), "{message}");
        assert!(message.contains("web:"), "{message}");
    }
}
