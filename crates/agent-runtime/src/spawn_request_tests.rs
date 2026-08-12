//! `srv:agent/spawn` 请求契约的单元面：schema 与入参解析。

use super::*;

#[test]
fn task_is_required_and_must_not_be_blank() {
    assert!(parse(&json!({})).is_err());
    assert!(parse(&json!({ "task": "   " })).is_err());
    assert!(parse(&json!({ "task": 7 })).is_err());
    assert_eq!(
        &*parse(&json!({ "task": "读一下 a.txt" })).unwrap().task,
        "读一下 a.txt"
    );
}

/// `tools` 缺省与显式 `null` 是同一件事（模型两种都会写）：交给父的子集兜底。
#[test]
fn a_missing_tools_field_means_inherit() {
    assert!(parse(&json!({ "task": "t" })).unwrap().tools.is_none());
    assert!(
        parse(&json!({ "task": "t", "tools": null }))
            .unwrap()
            .tools
            .is_none()
    );
    let got = parse(&json!({ "task": "t", "tools": ["srv:fs/read"] }))
        .unwrap()
        .tools
        .unwrap();
    assert_eq!(&*got[0], "srv:fs/read");
}

/// 052：缺省是**前台**（决策 20 一字不改）。缺省、显式 `null`、显式 `false`
/// 三种写法必须落在同一格上。
#[test]
fn background_defaults_to_false() {
    assert!(!parse(&json!({ "task": "t" })).unwrap().background);
    assert!(
        !parse(&json!({ "task": "t", "background": null }))
            .unwrap()
            .background
    );
    assert!(
        !parse(&json!({ "task": "t", "background": false }))
            .unwrap()
            .background
    );
    assert!(
        parse(&json!({ "task": "t", "background": true }))
            .unwrap()
            .background
    );
}

/// 写错类型要**看得见**：静默当真会让模型以为自己发的是前台 spawn，
/// 然后永远等不到那个不会回来的结果。
#[test]
fn background_must_be_a_boolean() {
    assert!(parse(&json!({ "task": "t", "background": "true" })).is_err());
    assert!(parse(&json!({ "task": "t", "background": 1 })).is_err());
}

#[test]
fn tools_must_be_an_array_of_strings() {
    assert!(parse(&json!({ "task": "t", "tools": "srv:fs/read" })).is_err());
    assert!(parse(&json!({ "task": "t", "tools": [1] })).is_err());
}

/// 145：缺省与显式 `null` 都是「不设限」——跟 145 之前唯一的行为（全带）
/// 逐字节等价，不是一个新分支。
#[test]
fn a_missing_inherit_prefix_field_means_no_limit() {
    assert!(
        parse(&json!({ "task": "t" }))
            .unwrap()
            .inherit_prefix
            .is_none()
    );
    assert!(
        parse(&json!({ "task": "t", "inherit_prefix": null }))
            .unwrap()
            .inherit_prefix
            .is_none()
    );
}

/// 145 三档语义的另两档：`[]`（显式一点都不带，跟 `None` 是两个不同的值）
/// 与「列出具体名字」，两条路径解析结果都要保留模型写的原文，不做归一化
/// （归一化只发生在校验那一步，见 `spawn_tool::check_prefix_allowed`）。
#[test]
fn inherit_prefix_accepts_an_empty_array_and_a_list_of_names() {
    let empty = parse(&json!({ "task": "t", "inherit_prefix": [] }))
        .unwrap()
        .inherit_prefix;
    assert_eq!(empty, Some(Vec::new()));

    let named = parse(&json!({ "task": "t", "inherit_prefix": ["srv:skill/index"] }))
        .unwrap()
        .inherit_prefix
        .unwrap();
    assert_eq!(&*named[0], "srv:skill/index");
}

/// 跟 `tools` 同款的写错类型必须看得见——静默当真会让模型以为它点名生效了，
/// 实际上子 agent 收到的是完全不同的一份材料。
#[test]
fn inherit_prefix_must_be_an_array_of_strings() {
    assert!(parse(&json!({ "task": "t", "inherit_prefix": "srv:skill/index" })).is_err());
    assert!(parse(&json!({ "task": "t", "inherit_prefix": [1] })).is_err());
}

/// spawn spec 的 description 得说清三档语义，模型才知道有这个口子、怎么用它
/// 省上下文（145 验收：「spawn spec description 含 inherit_prefix 说明」）。
#[test]
fn the_inherit_prefix_param_describes_all_three_tiers() {
    let spec = spawn_spec(AgentLimits::default());
    let text = spec.schema["properties"]["inherit_prefix"]["description"]
        .as_str()
        .unwrap();
    assert!(text.contains("省略"), "{text}");
    assert!(text.contains("全带"), "{text}");
    assert!(text.contains("一点都不带"), "{text}");
}

/// 050 的另一半：描述里那句「照抄你工具列表里的那个名字」是这次的行为承诺，
/// 掉了模型就没有任何线索该写哪种拼法。
#[test]
fn the_tools_param_tells_the_model_to_copy_the_name_verbatim() {
    let spec = spawn_spec(AgentLimits::default());
    let text = spec.schema["properties"]["tools"]["description"]
        .as_str()
        .unwrap();
    assert!(text.contains("照抄"), "{text}");
}

/// 052 的描述说「需要它的答案就别开后台」，053 之后那是假话——`srv:agent/collect`
/// 就是来领它的。071 改对之后由这条守住：后台那一段必须说清**结果不会自己回来**、
/// 以及去哪领、不领会怎样。三件事都是模型不知道就会白干一整棵子树的。
///
/// **断关键子串不断整段**（理由同 `status_tool_tests` 里那条）：文案是拿来调的，
/// 行为不是；工具名走 [`crate::COLLECT_TOOL`] 常量，改名而描述没跟上一样红。
#[test]
fn the_background_option_says_the_answer_will_not_come_back_by_itself() {
    let spec = spawn_spec(AgentLimits::default());
    let text = &*spec.description;
    assert!(
        text.contains("background=false"),
        "缺省那条路得说清是等的：{text}"
    );
    assert!(text.contains("不会自己回到你这里"), "{text}");
    assert!(text.contains(crate::COLLECT_TOOL), "得告诉它去哪领：{text}");
    assert!(text.contains("拆掉"), "不领的下场也得说：{text}");

    // 参数那一格自己也要立得住：模型读 schema 时未必回头看长描述。
    let param = spec.schema["properties"]["background"]["description"]
        .as_str()
        .unwrap();
    assert!(param.contains("不会自己回来"), "{param}");
    assert!(param.contains(crate::COLLECT_TOOL), "{param}");
}
