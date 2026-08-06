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
