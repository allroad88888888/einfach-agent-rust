//! 039 独立测试(agent-core 层,红线 11):`Slot::SkillsActive` 的值是**有序 Json
//! 数组**,序列化必须逐字节确定,且这个顺序不能是「谁先谁后激活的」这种偶然序
//! ——否则同一个最终激活集合,换一种激活顺序就会拼出不同的 system 前缀,
//! 前缀缓存全价(跟 `Slot::ToolsAllowed` 的排序去重是同一条道理,`spawn.rs` 的
//! `tools_value` 已经示范过一次)。
//!
//! 这份文件不猜「到底是字典序还是定义序」(issue 原文:「顺序是确定的(定义序或
//! 字典序,读实做记录哪个)」)——只钉这条更硬的性质:**同一个激活集合,不同的
//! 激活调用顺序,最终序列化必须逐字节相同**。具体是哪种策略留给独立测试报告里
//! 记一笔,不在这里赌。
//!
//! 独立测试 agent 规则同 `skill_indep_activation_journaled.rs`:只看
//! `agent-core` 的公开签名 + docs,不看 039 新增的实现体。

use agent_core::{AgentId, AgentValue, AtomKey, Session, SkillId, Slot};

fn root() -> AgentId {
    AgentId::root()
}

fn skills_active_value(session: &Session) -> AgentValue {
    let key = AtomKey::Agent(root(), Slot::SkillsActive);
    session
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .expect("Slot::SkillsActive 是一个 source 槽位,build_agent 建图时就该带默认值")
}

fn skills_active_bytes(session: &Session) -> Vec<u8> {
    serde_json::to_vec(&skills_active_value(session)).expect("AgentValue 必须全部可序列化(红线 3)")
}

#[test]
fn serializing_the_same_active_set_twice_is_byte_identical() {
    let mut session = Session::new(root());
    let _ = session.activate_skill(&root(), SkillId::new("alpha"));
    let _ = session.activate_skill(&root(), SkillId::new("beta"));
    let _ = session.activate_skill(&root(), SkillId::new("gamma"));

    assert_eq!(
        skills_active_bytes(&session),
        skills_active_bytes(&session),
        "同一个会话状态,两次序列化 SkillsActive 槽位必须逐字节相同"
    );
}

#[test]
fn the_serialized_active_set_does_not_depend_on_activation_call_order() {
    let mut forward = Session::new(root());
    for name in ["alpha", "beta", "gamma"] {
        let _ = forward.activate_skill(&root(), SkillId::new(name));
    }

    let mut backward = Session::new(root());
    for name in ["gamma", "alpha", "beta"] {
        let _ = backward.activate_skill(&root(), SkillId::new(name));
    }

    assert_eq!(
        skills_active_bytes(&forward),
        skills_active_bytes(&backward),
        "红线 11: 同一个激活集合、不同的激活调用顺序,落进 Slot::SkillsActive 的字节必须相同\
         ——否则模型看到的 skill 索引/激活列表前缀会因为『谁先谁后激活的』而漂,前缀缓存全价"
    );
}

#[test]
fn active_skills_reader_agrees_across_activation_orders() {
    let mut forward = Session::new(root());
    for name in ["alpha", "beta", "gamma"] {
        let _ = forward.activate_skill(&root(), SkillId::new(name));
    }
    let mut backward = Session::new(root());
    for name in ["gamma", "alpha", "beta"] {
        let _ = backward.activate_skill(&root(), SkillId::new(name));
    }

    assert_eq!(
        forward.active_skills(),
        backward.active_skills(),
        "读口(`active_skills`)看到的顺序也必须跟激活调用顺序无关,不只是底层字节"
    );
}

#[test]
fn reactivating_an_already_active_skill_does_not_duplicate_it() {
    let mut session = Session::new(root());
    let _ = session.activate_skill(&root(), SkillId::new("alpha"));
    let _ = session.activate_skill(&root(), SkillId::new("alpha"));

    let active = session.active_skills();
    assert_eq!(
        active.iter().filter(|s| **s == SkillId::new("alpha")).count(),
        1,
        "重复激活同一个 skill 不该产生重复项,实际: {active:?}"
    );
}

/// 三个不同的会话,各自用不同顺序激活同一个五元集合,五份序列化字节必须两两相同
/// ——比「两份」更强的证据,排除「凑巧两份一样」的可能。
#[test]
fn five_different_activation_orders_of_the_same_set_all_serialize_identically() {
    let names = ["alpha", "beta", "gamma", "delta", "epsilon"];
    let orders: [[&str; 5]; 5] = [
        ["alpha", "beta", "gamma", "delta", "epsilon"],
        ["epsilon", "delta", "gamma", "beta", "alpha"],
        ["gamma", "alpha", "epsilon", "beta", "delta"],
        ["delta", "epsilon", "alpha", "gamma", "beta"],
        ["beta", "gamma", "delta", "alpha", "epsilon"],
    ];
    let mut bytes_per_order = Vec::new();
    for order in &orders {
        let mut session = Session::new(root());
        for name in order {
            let _ = session.activate_skill(&root(), SkillId::new(*name));
        }
        assert_eq!(session.active_skills().len(), names.len(), "五个都该激活上");
        bytes_per_order.push(skills_active_bytes(&session));
    }
    for pair in bytes_per_order.windows(2) {
        assert_eq!(pair[0], pair[1], "不同激活顺序的最终字节必须两两相同");
    }
}
