//! `self_spec()` 那段**说明书**的测试。
//!
//! 措辞渲染在 `self_render_tests.rs`，端到端（真的跑一轮、真的读槽位）在
//! `tests/it/self_indep_*.rs`。这个文件只守「我们跟模型是怎么说的」——描述是
//! 一段每轮都进 prompt 的字符串，它自己就是一份契约。
//!
//! **断的是关键子串，不是整段文案**（照 `status_spec_tests` 的既有理由）：
//! 措辞会随经验改，逐字断言只会让下一个改文案的人顺手把测试一起改掉。

use super::*;

/// 208 唯一防得住「模型把过期的数当事实」的东西：描述里必须有时态限定词。
///
/// 这一轮回「本轮已经请求 3 次」，三轮之后模型在历史里读到的还是那个 3。
/// 跟时间戳进 prompt 是同一类病——一个看起来永远成立的事实，冻进历史之后
/// 就是假的。**这条是 issue 208 验收里点名要写成断言的那一条。**
#[test]
fn the_spec_says_the_numbers_are_a_snapshot_in_time() {
    let text = &*self_spec().description;
    assert!(
        text.contains("那一刻"),
        "描述缺了时态限定词，模型会把上一次的结果当成现在还成立：{text}"
    );
    assert!(
        text.contains("再调一次"),
        "得告诉它怎么拿到最新的，光说会过期没有下一步：{text}"
    );
}

/// 全名 + 无入参。**自己是谁由截获现场决定**，不给模型一个能填错的口。
#[test]
fn it_takes_no_parameters() {
    let spec = self_spec();
    assert_eq!(&*spec.name, SELF_TOOL);
    assert_eq!(spec.schema["type"], "object");
    assert_eq!(
        spec.schema["properties"],
        serde_json::json!({}),
        "self 不该有任何入参：要看别人用 status"
    );
    assert!(
        spec.schema["required"].is_null(),
        "没有入参就不该有 required"
    );
}

/// 它只回自己的账；要看别人得去 `status`。**描述里点名那个工具**，
/// 不然模型只会反复调 self 找别人。
#[test]
fn it_points_at_status_for_everyone_else() {
    let text = &*self_spec().description;
    assert!(
        text.contains(crate::STATUS_TOOL),
        "要看别人得点名 status：{text}"
    );
}

/// 描述要说清**它值得在什么时候调**，尤其是「快用完就先把结论说出来」——
/// 208 存在的全部理由就是让模型有机会在被闸切断之前收敛。
#[test]
fn it_says_what_to_do_when_the_budget_runs_low() {
    let text = &*self_spec().description;
    assert!(text.contains("快用完"), "{text}");
    assert!(text.contains("结论"), "{text}");
}
