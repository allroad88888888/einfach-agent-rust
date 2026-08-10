//! Issue 107：连续两次压缩，第一份摘要**留在库里不回收**——`Slot::Summaries`
//! 只增不删（回收了 redo 就拿不回来），`SendPlan` 里只留最新那个引用。
//!
//! 同一个文件顺带钉住 issue 里那处刻意的放宽：第二次压缩那条 entry 的
//! `Summaries` 槽位 change 的 `prev` **允许**含第一份摘要的正文（095 形状决策
//! 的已知代价），但**永远不该**含任何一条原始历史消息的正文——那条界线才是
//! 真正的红线，不是「prev 里有没有旧摘要」。

use std::sync::Arc;

use agent_core::{AgentId, AtomKey, Session, Slot};

use crate::support;
use crate::support::session::new_session;

const RAW_HISTORY_FINGERPRINT: &str = "只应该活在原始历史里的指纹字符串__ABC123";

/// 第一轮消息里塞一个指纹字符串，后面几轮都是普通占位文本。
fn session_with_a_fingerprinted_round(rounds: usize) -> Session {
    let mut s = new_session();
    for i in 0..rounds {
        if i > 0 {
            s.begin_turn();
        }
        let text = if i == 0 {
            RAW_HISTORY_FINGERPRINT.to_string()
        } else {
            format!("第 {i} 轮问题")
        };
        let _ = s.step(support::user_input_event(&text));
        let _ = s.step(support::provider_done_end_turn(
            s.epoch(),
            &format!("第 {i} 轮回答"),
        ));
    }
    s
}

#[test]
fn a_second_compaction_keeps_the_first_summarys_text_retrievable() {
    let mut s = new_session();
    let root = AgentId::root();

    let id_1 = s.apply_summary(&root, 5, Arc::from("摘要 1")).unwrap();
    let id_2 = s.apply_summary(&root, 9, Arc::from("摘要 2")).unwrap();

    assert_eq!(
        s.summary_text(&root, &id_1),
        Some(Arc::from("摘要 1")),
        "摘要 1 被摘要 2 取代成当前引用之后，正文仍然在库里取得到——\
         回收了 redo 就拿不回来"
    );
    assert_eq!(s.summary_text(&root, &id_2), Some(Arc::from("摘要 2")));

    let plan = s.send_plan_of(&root);
    assert_eq!(plan.boundary(), 9, "边界继续前进");
    assert_eq!(plan.summary(), Some(&id_2), "SendPlan 里只留最新那个引用");
}

#[test]
fn the_second_compactions_prev_may_carry_the_first_summary_but_never_raw_history() {
    let mut s = session_with_a_fingerprinted_round(3);
    let root = AgentId::root();
    let first_boundary = s.messages_of(&root).len();

    s.apply_summary(&root, first_boundary, Arc::from("摘要 1"))
        .unwrap();

    s.begin_turn();
    let _ = s.step(support::user_input_event("又一轮"));
    let _ = s.step(support::provider_done_end_turn(s.epoch(), "又一轮的回答"));
    let second_boundary = s.messages_of(&root).len();

    s.apply_summary(&root, second_boundary, Arc::from("摘要 2"))
        .unwrap();

    let entry = s.last_entry().expect("第二次压缩该留下一条 entry");
    let summaries_key = AtomKey::Agent(root.clone(), Slot::Summaries);
    let change = entry
        .changes
        .iter()
        .find(|c| c.key == summaries_key)
        .expect("entry 里该有一条改 Summaries 槽位的 change");

    let prev_bytes = serde_json::to_vec(&change.prev).unwrap();
    let prev_text = String::from_utf8_lossy(&prev_bytes);

    assert!(
        prev_text.contains("摘要 1"),
        "刻意的放宽：Slot::Summaries 只增不删，第二次压缩那条 entry 的 prev \
         允许含第一份摘要的正文——这是 095 形状决策的已知代价，不是 bug：{prev_text}"
    );
    assert!(
        !prev_text.contains(RAW_HISTORY_FINGERPRINT),
        "但无论如何都不该含任何一条原始历史消息的正文：{prev_text}"
    );
}
