//! `status_spec()` 那段**说明书**的测试（207 从 `status_tool_tests.rs` 拆出来，
//! 红线 9）。跟隔壁那个文件的分工：那边测「这次调用该看到哪些节点」的判定，
//! 这边测「我们跟模型是怎么说的」——描述文案是一段每轮都进 prompt 的字符串，
//! 它自己就是一份需要被守住的契约。

use super::*;

/// 工具声明的固定事实：全名、可选的 `id`，以及描述里那两句分水岭——**不返回正文**、
/// **看得见整棵树**。
///
/// 051 写的原文是「正文会在那次 spawn 调用的结果里回到你这里」，052 加了
/// `background=true` 之后它成了假话（后台 spawn 只回 `{"agent_id":...}`，正文要
/// 用 `collect` 领）。207 又把「你 spawn 出来的子 agent」改成了整棵树。这段字符串
/// 每一轮都进 prompt，模型每次都读到并可能照它办事，所以它需要一条测试守着。
///
/// **断的是关键子串，不是整段文案。** 措辞会随经验改（它就是拿来调的），逐字
/// 断言只会让下一个改文案的人顺手把测试一起改掉——那条测试从此什么都不守。
#[test]
fn the_spec_tells_the_model_the_scope_and_where_an_answer_actually_comes_from() {
    let spec = status_spec();
    let text = &*spec.description;
    assert_eq!(&*spec.name, STATUS_TOOL);

    // 207：范围是整棵树，而且这份清单是 send 的 id 来源。
    assert!(text.contains("整棵树"), "范围得说清：{text}");
    assert!(text.contains("兄弟"), "兄弟看得见是这一波的核心：{text}");
    // 206 落地之后这里从字面量换成了常量（208 补做）：「send 改了名而这段描述
    // 没跟上」从此也一样红，照下面 `COLLECT_TOOL` 那条的写法。
    assert!(
        text.contains(crate::SEND_TOOL),
        "这份清单里的 id 就是 send 的 to，描述里该点名：{text}"
    );

    // 051/052 起的老约定：不返回正文，两种 spawn 各说各的。
    assert!(text.contains("不返回任何 agent 的回答正文"), "{text}");
    assert!(
        text.contains("前台"),
        "前台那条路（正文从 spawn 槽回来）得说：{text}"
    );
    assert!(text.contains("background=true"), "后台那条路得点名：{text}");
    assert!(
        text.contains(crate::COLLECT_TOOL),
        "后台子的正文要用 collect 领：{text}"
    );

    assert_eq!(spec.schema["properties"]["id"]["type"], "string");
    assert!(
        spec.schema["required"].is_null(),
        "id 是可选的，不该有 required"
    );
}
