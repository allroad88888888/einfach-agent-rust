//! [`validate`] 的单测（061，140 起把 skill 携带工具的用例改写成「先撞 400」）。
//!
//! | 测试 | 它看住的那一行 |
//! |---|---|
//! | [`host_side_prefixes_are_accepted`] | `TOOL_PREFIXES` 白名单 |
//! | [`server_side_and_prefixless_names_are_rejected`] | `check_tool` 的前缀分支 |
//! | [`the_part_after_the_prefix_is_whitelisted`] | `check_tool` 的字符集/长度分支 |
//! | [`a_skill_declaring_any_tools_is_rejected_before_shape_or_prefix_checks`] | 140 新加的 `!skill.tools.is_empty()` 早退 |
//! | [`duplicate_tool_names_are_rejected_at_top_level`] | `check_tool` 的重名分支（顶层） |
//! | [`skill_ids_are_whitelisted_and_unique`] | `is_valid_skill_id` + `skill_ids.insert` |
//! | [`an_empty_declaration_is_valid`] | 早返回 `Ok(())` |
//! | [`the_message_names_the_offending_item_and_stays_bounded`] | `elide` + `Display` |
//! | [`the_skill_tools_rejection_names_the_skill_and_explains_the_v1_cut`] | `SkillCarriesTools` 的 `Display` |

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

/// 140：**一个 skill 只要带了 `tools`，不管形状合不合法、跟顶层撞不撞名，
/// 一律先在这一条上被拒**——旧版本这里测的是「skill 自带的工具过同一条前缀/
/// 形状校验」，决策 27 把 skill 的工具注入口整个砍掉之后，那条校验对 skill 已经
/// 不可达：`tools` 非空这件事本身就是整份声明的问题，早于任何一条前缀/形状检查。
#[test]
fn a_skill_declaring_any_tools_is_rejected_before_shape_or_prefix_checks() {
    // 形状本身完全合法的工具，挂在 skill 上照样被拒——证明判定顺序在前缀检查之前。
    let well_formed = caps(json!({
        "skills": [ { "id": "crm-flow", "tools": [ { "name": "web:crm/lookup" } ] } ]
    }));
    assert_eq!(
        validate(&well_formed),
        Err(CapabilityRejection::SkillCarriesTools {
            id: "crm-flow".to_string()
        })
    );

    // 形状本身就不合法的工具（旧版本这里期望 ToolPrefix），现在报的仍然是同一条：
    // 「skill 不许带工具」比「工具名前缀不对」更早说清。
    let bad_prefix = caps(json!({
        "skills": [ { "id": "crm-flow", "tools": [ { "name": "srv:crm/lookup" } ] } ]
    }));
    assert_eq!(
        validate(&bad_prefix),
        Err(CapabilityRejection::SkillCarriesTools {
            id: "crm-flow".to_string()
        })
    );

    // 旧版本这里测的是「skill 工具与顶层撞名」（DuplicateTool）；同一个理由，
    // 现在两种声明（撞名与否）报的都是 SkillCarriesTools——跨边界的重名规则在
    // skill 这一侧因此不再可达（skill.tools 必须先为空才轮得到它）。
    let across = caps(json!({
        "tools": [ { "name": "web:a/b" } ],
        "skills": [ { "id": "s1", "tools": [ { "name": "web:a/b" } ] } ]
    }));
    assert_eq!(
        validate(&across),
        Err(CapabilityRejection::SkillCarriesTools {
            id: "s1".to_string()
        })
    );
    let between_skills = caps(json!({
        "skills": [
            { "id": "s1", "tools": [ { "name": "web:a/b" } ] },
            { "id": "s2", "tools": [ { "name": "web:a/b" } ] }
        ]
    }));
    assert_eq!(
        validate(&between_skills),
        Err(CapabilityRejection::SkillCarriesTools {
            id: "s1".to_string()
        })
    );

    // 空 `tools` 数组仍然合法——140 拒的是「非空」，不是这个字段本身。
    assert_eq!(
        validate(&caps(json!({ "skills": [ { "id": "crm-flow", "tools": [] } ] }))),
        Ok(())
    );
}

/// 重名：顶层内部——工具名在整份声明里全局唯一。skill 这一侧不再参与（见上一条）：
/// skill 的 `tools` 必须先为空才能通过，空集合里自然没有重名可言。
#[test]
fn duplicate_tool_names_are_rejected_at_top_level() {
    assert_eq!(
        validate(&tools(&["web:a/b", "web:a/b"])),
        Err(CapabilityRejection::DuplicateTool {
            origin: Origin::TopLevel,
            name: "web:a/b".to_string()
        })
    );
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

/// 140：`SkillCarriesTools` 的文案要点名是哪个 skill、指向裁剪依据（决策 27）、
/// 说清正确的声明位置（`capabilities.tools`）——这是任务书原文要求的三件事。
#[test]
fn the_skill_tools_rejection_names_the_skill_and_explains_the_v1_cut() {
    let declaration = caps(json!({
        "skills": [ { "id": "crm-flow", "tools": [ { "name": "web:crm/lookup" } ] } ]
    }));
    let message = validate(&declaration).unwrap_err().to_string();
    assert!(message.contains("crm-flow"), "该点名是哪个 skill：{message}");
    assert!(
        message.contains("capabilities.tools"),
        "该说清工具该往哪儿声明：{message}"
    );
    assert!(message.contains("决策 27"), "该指向裁剪依据：{message}");
}
