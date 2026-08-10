//! Issue 107 的兑现点——095 整个形状决策最终落地的地方：`apply_summary` 落的
//! entry，大小只跟**摘要正文**和 `SendPlan` 里的两个数有关，跟被摘要的历史有
//! 多长、跟摘要正文本身有多长都无关——历史正文从来不会经过这条 entry。
//!
//! 两条测试分别锁「跟历史长度无关」（摘要 3 轮和摘要 150 轮，entry 一样大）与
//! 「跟摘要正文长度无关」（`SendPlan` 只存引用，红线 5）。

use std::sync::Arc;

use agent_core::{AgentId, AtomKey, Session, Slot};

use crate::support;
use crate::support::session::new_session;

fn session_with_rounds(rounds: usize) -> Session {
    let mut s = new_session();
    for i in 0..rounds {
        if i > 0 {
            s.begin_turn();
        }
        let _ = s.step(support::user_input_event(&format!(
            "第 {i} 轮问题，带一点长度免得太巧合地小"
        )));
        let _ = s.step(support::provider_done_end_turn(
            s.epoch(),
            &format!("第 {i} 轮回答，也带一点长度"),
        ));
    }
    s
}

/// entry 里跟 `apply_summary` 相关的那两个 change（`SendPlan` + `Summaries`）
/// 序列化后一共多少字节。
fn compaction_entry_bytes(s: &Session, agent: &AgentId) -> usize {
    let entry = s.last_entry().expect("该有一条 entry");
    let send_plan_key = AtomKey::Agent(agent.clone(), Slot::SendPlan);
    let summaries_key = AtomKey::Agent(agent.clone(), Slot::Summaries);
    entry
        .changes
        .iter()
        .filter(|c| c.key == send_plan_key || c.key == summaries_key)
        .map(|c| serde_json::to_vec(c).unwrap().len())
        .sum()
}

/// 摘要 3 轮的历史，和摘要 150 轮的历史，用同一段摘要正文——entry 的字节数
/// 该几乎一样（只差 upto/边界这个数字位数上的几个字符），不该随历史长度
/// 线性增长。这是 095 §「压缩只在发送侧」形状决策的最终验收点。
#[test]
fn entrys_footprint_does_not_grow_with_the_length_of_history_being_summarized() {
    let summary_text: Arc<str> = Arc::from("固定长度的摘要正文，两边完全一样。");
    let root = AgentId::root();

    let mut small = session_with_rounds(3);
    let small_boundary = small.messages_of(&root).len();
    small
        .apply_summary(&root, small_boundary, summary_text.clone())
        .unwrap();
    let small_bytes = compaction_entry_bytes(&small, &root);

    let mut large = session_with_rounds(150);
    let large_boundary = large.messages_of(&root).len();
    large
        .apply_summary(&root, large_boundary, summary_text.clone())
        .unwrap();
    let large_bytes = compaction_entry_bytes(&large, &root);

    assert!(
        small_bytes < 800,
        "小历史那条 entry 该很小：实际 {small_bytes} bytes"
    );
    assert!(
        large_bytes.abs_diff(small_bytes) < 100,
        "历史从 3 轮涨到 150 轮，entry 大小只该差个位数（upto 数字变长了几位），\
         不该线性增长：小历史 {small_bytes} bytes，大历史 {large_bytes} bytes"
    );
}

/// 反过来：摘要正文从几字节涨到远超 10 KB，`SendPlan` 槽位那一半 change 的
/// 大小不该变——正文只住在 `Slot::Summaries`，`SendPlan` 里只有一个引用
/// （红线 5）。顺带确认 `SendPlan` 的 change 里压根不含正文本身。
#[test]
fn send_plans_own_change_does_not_grow_with_summary_text_length() {
    let root = AgentId::root();
    let send_plan_key = AtomKey::Agent(root.clone(), Slot::SendPlan);

    let mut short = new_session();
    short.apply_summary(&root, 5, Arc::from("短")).unwrap();
    let short_bytes = {
        let entry = short.last_entry().unwrap();
        let change = entry
            .changes
            .iter()
            .find(|c| c.key == send_plan_key)
            .unwrap();
        serde_json::to_vec(change).unwrap().len()
    };

    let mut long = new_session();
    let huge: Arc<str> = Arc::from("摘要正文超过十千字节".repeat(2000));
    assert!(huge.len() > 10 * 1024, "确保正文真的超过 10 KB");
    long.apply_summary(&root, 5, huge.clone()).unwrap();
    let (long_bytes, long_change_json) = {
        let entry = long.last_entry().unwrap();
        let change = entry
            .changes
            .iter()
            .find(|c| c.key == send_plan_key)
            .unwrap();
        let json = serde_json::to_string(change).unwrap();
        (json.len(), json)
    };

    assert_eq!(
        short_bytes, long_bytes,
        "SendPlan 槽位的 change 大小不该随摘要正文长度变化：短正文 {short_bytes} bytes，\
         长正文（>10KB）{long_bytes} bytes"
    );
    assert!(
        !long_change_json.contains(huge.as_ref()),
        "SendPlan 槽位的 change 不该直接嵌入摘要正文——正文只该以引用的形式\
         活在 Summaries 槽位"
    );
}
