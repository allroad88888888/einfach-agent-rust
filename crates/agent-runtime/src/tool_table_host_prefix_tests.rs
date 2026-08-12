//! [`super`]（`tool_table_host_prefix`）的单测，逐条对应 issue 155 §验收：
//! 排序落块（含乱序喂入）、`check_prefix_allowed` 认得声明名、空切片零操作
//! （specs/declares/timed 三面）、与内置 timed 共存时声明块排在后面。

use std::sync::Arc;

use agent_core::{AgentId, HostSkill, Session, SkillId};

use crate::session_start::run_session_start;
use crate::skill::SkillRegistry;
use crate::spawn_tool::check_prefix_allowed;

use super::*;

fn pairs(items: &[(&str, &str)]) -> Vec<(Arc<str>, Arc<str>)> {
    items
        .iter()
        .map(|(name, text)| (Arc::from(*name), Arc::from(*text)))
        .collect()
}

/// 验收第一条：两对声明经 `run_session_start` 落两块 prefix chunk，label 是
/// `init:<name>`、text 原样、序 = name 字典序；乱序喂入同一组对，结果逐字节
/// 不变（排序断言本身）。
#[test]
fn two_declared_pairs_land_in_name_order_regardless_of_input_order() {
    let forward = pairs(&[
        ("web:crm/briefing", "今天的客户上下文"),
        ("desk:mail/draft", "待发邮件草稿"),
    ]);
    let mut session = Session::new(AgentId::root());
    let table = ToolTable::builtin().with_host_prefix(&forward);
    run_session_start(&mut session, &table).expect("两条声明都该成功");
    let chunks = session.prefix_chunks();
    assert_eq!(chunks.len(), 2, "两对声明该恰好落两块");
    assert_eq!(&*chunks[0].label, "init:desk:mail/draft");
    assert_eq!(&*chunks[0].text, "待发邮件草稿");
    assert_eq!(&*chunks[1].label, "init:web:crm/briefing");
    assert_eq!(&*chunks[1].text, "今天的客户上下文");

    // 同一组对，输入顺序倒过来喂——落出来的块必须逐字节相同（排序断言）。
    let shuffled = pairs(&[
        ("desk:mail/draft", "待发邮件草稿"),
        ("web:crm/briefing", "今天的客户上下文"),
    ]);
    let mut other_session = Session::new(AgentId::root());
    let other_table = ToolTable::builtin().with_host_prefix(&shuffled);
    run_session_start(&mut other_session, &other_table).expect("两条声明都该成功");
    let other_chunks = other_session.prefix_chunks();
    assert_eq!(chunks.len(), other_chunks.len());
    for (a, b) in chunks.iter().zip(other_chunks.iter()) {
        assert_eq!(a.label, b.label, "乱序喂入不该改变落块的顺序");
        assert_eq!(a.text, b.text);
    }
}

/// 验收第二条：`check_prefix_allowed`（spawn 的 `inherit_prefix` 校验）读的就是
/// timed 区 spec 名——合成条目自动被它认识；没声明过的名字照旧拒。
#[test]
fn check_prefix_allowed_recognizes_declared_names_and_still_rejects_unknown_ones() {
    let declared = pairs(&[("web:crm/briefing", "今天的客户上下文")]);
    let table = ToolTable::builtin().with_host_prefix(&declared);

    let ok = check_prefix_allowed(vec![Arc::from("web:crm/briefing")], &table);
    assert_eq!(
        ok,
        Ok(vec![Arc::from("web:crm/briefing")]),
        "声明过的名字必须通过"
    );

    let rejected = check_prefix_allowed(vec![Arc::from("web:not/declared")], &table);
    assert!(rejected.is_err(), "没声明过的名字照旧拒");
}

/// 验收第三条：空切片是彻底的无操作——specs/declares/timed 三面都跟压根没调过
/// 这个方法逐字节相同。
#[test]
fn an_empty_slice_leaves_the_table_byte_for_byte_identical() {
    let bare = ToolTable::builtin();
    let untouched = ToolTable::builtin().with_host_prefix(&[]);

    assert_eq!(
        serde_json::to_string(bare.specs()).unwrap(),
        serde_json::to_string(untouched.specs()).unwrap(),
        "specs() 必须逐字节相同"
    );
    assert_eq!(
        bare.declares("srv:fs/read"),
        untouched.declares("srv:fs/read"),
        "declares() 的判定不该被空切片改变"
    );
    assert_eq!(
        bare.timed(CallTiming::SessionStart).count(),
        untouched.timed(CallTiming::SessionStart).count(),
        "timed 区不该多出任何一条"
    );
}

/// 验收第四条：与内置 timed（skills 索引）共存时，内置块先注册、声明块排在
/// 后面——注册序即前缀块序（模块文档「表尾 + 内部排序」，与 with_host_tools
/// 排在 with_skills 之后同一个道理）。
#[test]
fn declared_entries_register_after_the_builtin_skill_index() {
    let registry = SkillRegistry::from_host_skills(vec![HostSkill {
        id: SkillId::new("crm-flow"),
        description: Arc::from("处理客户工单"),
        body: Arc::from("body"),
        tools: Vec::new(),
        tool_reversibility: Default::default(),
    }]);
    let declared = pairs(&[("web:crm/briefing", "今天的客户上下文")]);
    let table = ToolTable::builtin()
        .with_skills(registry)
        .with_host_prefix(&declared);

    let names: Vec<Arc<str>> = table
        .timed(CallTiming::SessionStart)
        .map(|t| Arc::clone(&t.spec().name))
        .collect();
    assert_eq!(names.len(), 2, "内置索引 + 一条声明，共两条");
    assert_eq!(&*names[0], "srv:skill/index", "内置索引必须在前");
    assert_eq!(&*names[1], "web:crm/briefing", "声明块排在内置块之后");
}
