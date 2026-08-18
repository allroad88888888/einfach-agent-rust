//! `self_render` 的单元测试：**措辞**这一半。
//!
//! 取数那一半（哪几个槽位、上限从哪来）在 `self_tool_tests.rs` 与
//! `tests/it/self_indep_*.rs`。这里一个 `Session` 都不造。

use super::*;

/// 一份基准账，各条测试只改自己关心的那一格——省得每条都写十一个字段，
/// 也让「改了哪一格」在测试里一眼看得见。
fn facts() -> SelfFacts {
    SelfFacts {
        id: AgentId::root(),
        depth: 0,
        max_depth: 3,
        turns_used: 2,
        max_turns: 20,
        retries_used: 0,
        max_retries: 3,
        children_live: 1,
        max_children: 8,
        tools: 12,
        compacted: false,
    }
}

/// **红线 11**：同一份账渲染两次逐字节相同。没有时间戳、没有调用序号、
/// 没有随机——这段字节会进历史并被缓存。
#[test]
fn the_same_facts_render_byte_identically() {
    assert_eq!(render(&facts()), render(&facts()));
}

/// 每个数都要真的出现在正文里，不是渲染完丢掉一半。
#[test]
fn every_number_shows_up() {
    let body = render(&facts());
    assert!(body.contains("root"), "{body}");
    assert!(body.contains("第 0 层"), "{body}");
    assert!(body.contains("已经请求 2 次"), "{body}");
    assert!(body.contains("上限 20 次"), "{body}");
    assert!(body.contains("还剩 18 次"), "{body}");
    assert!(body.contains("连续失败 0 次"), "{body}");
    assert!(body.contains("1 个活着的直接子 agent"), "{body}");
    assert!(body.contains("还能再开 7 个"), "{body}");
    assert!(body.contains("再往下 3 层"), "{body}");
    assert!(body.contains("12 个工具"), "{body}");
}

/// 208 的核心一句：**这是调用那一刻的数**。正文里必须带时态限定，
/// 不许写成一个无时态的断言——三轮之后模型在历史里读到的那个数早过期了。
#[test]
fn the_body_says_it_is_a_snapshot_in_time() {
    let body = render(&facts());
    assert!(
        body.contains("这一刻"),
        "正文缺了时态限定词，模型会把过期的数当事实：{body}"
    );
}

/// 轮次撞顶不是「还剩 0 次」，是「现在就把结论说出来」——只有后一种说法
/// 模型会据此收敛。
#[test]
fn a_used_up_turn_budget_says_what_to_do_about_it() {
    let f = SelfFacts {
        turns_used: 20,
        ..facts()
    };
    let body = render(&f);
    assert!(body.contains("已经用完"), "{body}");
    assert!(body.contains("现在就把结论给出来"), "{body}");
}

/// 只剩一次跟还剩很多是两句话。
#[test]
fn the_last_turn_gets_its_own_warning() {
    let f = SelfFacts {
        turns_used: 19,
        ..facts()
    };
    assert!(render(&f).contains("只剩最后 1 次"));
}

/// `turns_used > max_turns`（重试链把它顶过头这类）不该 panic，也不该
/// 回一个环绕出来的巨大数字。
#[test]
fn overshooting_the_budget_does_not_underflow() {
    let f = SelfFacts {
        turns_used: 25,
        max_turns: 20,
        ..facts()
    };
    let body = render(&f);
    assert!(body.contains("已经用完"), "{body}");
}

/// 子数满了要说清「做完的子不占格子」——不然模型只会换个名字再 spawn 一次。
#[test]
fn a_full_child_slot_explains_the_way_out() {
    let f = SelfFacts {
        children_live: 8,
        ..facts()
    };
    let body = render(&f);
    assert!(body.contains("已经满了"), "{body}");
    assert!(body.contains("再 spawn 会被拒"), "{body}");
}

/// 最深一层说的是另一件事：不是「还能几层」，是「一层都不能了，这件事得
/// 你自己做」。
#[test]
fn the_deepest_agent_is_told_it_cannot_spawn() {
    let f = SelfFacts {
        depth: 3,
        ..facts()
    };
    let body = render(&f);
    assert!(body.contains("已经在最深一层"), "{body}");
    assert!(body.contains("spawn 不出子 agent"), "{body}");
}

/// 压缩只回布尔，**不回正文**——摘要正文塞进 tool_result 等于同一段文字在
/// prompt 里出现两次。
#[test]
fn compaction_is_a_yes_or_no() {
    assert!(render(&facts()).contains("没压过"));
    let f = SelfFacts {
        compacted: true,
        ..facts()
    };
    assert!(render(&f).contains("压过"));
}

/// 子 agent 那一份是**它自己的** id 和深度，不是 root 的。
#[test]
fn a_child_renders_its_own_identity() {
    let f = SelfFacts {
        id: AgentId::new("root/a1"),
        depth: 1,
        ..facts()
    };
    let body = render(&f);
    assert!(body.contains("root/a1"), "{body}");
    assert!(body.contains("第 1 层"), "{body}");
}
