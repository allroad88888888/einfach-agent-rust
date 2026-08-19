//! `await_spec()` 那段**说明书** + 入参解析 + 拒绝文案。
//!
//! 端到端（真的挂起、真的收敛、真的查环）在 `tests/it/await_indep_*.rs`。
//! **断的是关键子串，不是整段文案**（照 `status_spec_tests` 的既有理由）。

use super::*;

use serde_json::json;

/// 212 §1 点名的那句分工：**`await` 只告诉你「它到了」，不给正文**，
/// 而且「等自己开的后台子直接 `collect` 就行」。
///
/// 不说清的话模型会拿 `await` 当 `collect` 用，然后抱怨拿不到正文——那是
/// 一次白花的往返，而且它会一直这么用下去。
#[test]
fn the_spec_draws_the_line_between_await_and_collect() {
    let text = &*await_spec().description;
    assert!(text.contains("不给你对方的回答正文"), "{text}");
    assert!(
        text.contains(crate::COLLECT_TOOL),
        "得点名正文该找谁要：{text}"
    );
    assert!(
        text.contains("直接 collect"),
        "「等自己开的后台子用不着 await」这句得在：{text}"
    );
    assert!(text.contains("兄弟"), "它的用武之地是等不归你领的：{text}");
}

/// 三档 `until` 都要说，而且要说清**等错了会当场报错而不是一直等**——
/// 这是 `AwaitProgress::Unreachable` 那一档存在的全部理由。
#[test]
fn the_spec_explains_all_three_until_values_and_the_wrong_ending() {
    let spec = await_spec();
    let text = &*spec.description;
    for word in ["settled", "done", "failed"] {
        assert!(text.contains(word), "{word} 没说：{text}");
    }
    assert!(
        text.contains("当场收到一个错误"),
        "等错了会立刻报错这句得在，不然模型会以为它得一直等：{text}"
    );
    assert_eq!(spec.schema["required"], json!(["id"]), "只有 id 必填");
}

/// **互相等待会被拒**这件事要写进描述：模型事先知道，就不会去设计一个注定被拒的
/// 编排。只在拒绝时才告诉它，那一次往返是白花的。
#[test]
fn the_spec_warns_about_mutual_waiting_up_front() {
    let text = &*await_spec().description;
    assert!(text.contains("互相等待"), "{text}");
}

/// 缺省是 `settled`（模型两种写法都会用：省略、或者显式 null）。
#[test]
fn a_missing_until_means_settled() {
    let (_, until) = parse(&json!({"id": "root/a1"})).expect("解析");
    assert_eq!(until, AwaitUntil::Settled);
    let (_, until) = parse(&json!({"id": "root/a1", "until": null})).expect("解析");
    assert_eq!(until, AwaitUntil::Settled);
}

/// 三个词都认，且**认的是线上那组词**（`AwaitUntil::as_str` 那一份），
/// 不是 `Debug` 输出。
#[test]
fn the_three_until_words_round_trip() {
    for (word, expected) in [
        ("settled", AwaitUntil::Settled),
        ("done", AwaitUntil::Done),
        ("failed", AwaitUntil::Failed),
    ] {
        let (_, until) = parse(&json!({"id": "root/a1", "until": word})).expect("解析");
        assert_eq!(until, expected);
    }
}

/// 入参写错一律回给模型看的文本，不 panic（003 的哲学）。
#[test]
fn bad_input_becomes_a_message_for_the_model() {
    for bad in [
        json!({}),
        json!({"id": ""}),
        json!({"id": 3}),
        json!({"id": "root/a1", "until": "whenever"}),
        json!({"id": "root/a1", "until": 7}),
    ] {
        let err = parse(&bad).expect_err("该被拒");
        assert!(err.starts_with("await 失败："), "{bad} → {err}");
    }
}

/// 成环的拒绝文案里**必须把环上那条链原样列出来**（212）：只说「会成环」，
/// 模型不知道该绕开谁，只会换个写法再撞一次。
#[test]
fn the_cycle_refusal_names_everyone_on_the_chain() {
    use agent_core::AwaitDenied;

    let chain = vec![
        agent_core::AgentId::new("root/a2"),
        agent_core::AgentId::new("root/a3"),
        agent_core::AgentId::new("root/a1"),
    ];
    let text = explain(&AwaitDenied::WouldCycle { chain });
    for id in ["root/a1", "root/a2", "root/a3"] {
        assert!(text.contains(id), "{id} 不在拒绝文案里：{text}");
    }
    assert!(
        text.contains("srv:agent/send"),
        "得给一条出路（把结果推过去），不是只说不行：{text}"
    );
}

/// 每一条拒绝都得带上下一步——本仓拒绝文案的通例（`send_tool::explain`）。
#[test]
fn every_refusal_is_actionable() {
    use agent_core::{AgentId, AwaitDenied};

    let all = [
        AwaitDenied::Yourself {
            agent: AgentId::root(),
        },
        AwaitDenied::NotInSession {
            target: AgentId::new("root/x"),
        },
        AwaitDenied::NotLive {
            target: AgentId::new("root/x"),
        },
        AwaitDenied::WouldCycle {
            chain: vec![AgentId::new("root/a1"), AgentId::root()],
        },
    ];
    for denied in all {
        let text = explain(&denied);
        assert!(text.starts_with("await 失败："), "{text}");
        assert!(text.len() > 24, "只说「不行」没有用：{text}");
    }
}
