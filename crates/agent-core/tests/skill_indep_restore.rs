//! 039 独立测试(agent-core 层):崩溃恢复路径。`docs/TOOLS.md` §Skills 原文
//! ——skill 的**内容**在 store 外的 registry 里,store 里只有「哪些被激活」
//! (`Slot::SkillsActive`)。这份测试只钉 agent-core 这一层的契约:
//! `Session::restore` 灌回快照/日志之后,`active_skills()` 完整地把 id 集合带
//! 回来——它不知道也不该知道 registry 长什么样(那是 agent-runtime 的事,红线 12
//! 的精神:core 不认识"skill 内容"这个概念,只认识"哪些 id 活跃")。
//!
//! 独立测试 agent 规则同本目录其余 `skill_indep_*.rs`:只看 `agent-core` 的
//! 公开签名 + docs,不看 039 新增的实现体。`Session::restore` 的签名是既有的
//! (027/010 定型,不属于 039 新增),这里按已读过的公开签名原样使用。
//!
//! **不猜 `Slot::SkillsActive` 落进哪个具体 `AgentValue` 变体**(可能还是既有的
//! `Json`,也可能是专门开的新变体——两者对这份测试都无所谓):手搭快照要用的值
//! 一律从一个真实激活过的 `Session` 里现取(`skills_active_value_for`),
//! 跟 `activate_skill` 走的是同一条写入路径产出的值,不在这里另起一份猜测。

use agent_core::{AgentId, AgentValue, AtomKey, Session, SkillId, Slot};

fn root() -> AgentId {
    AgentId::root()
}

/// 造一个"只激活了 `names` 这些 skill"时 `Slot::SkillsActive` 该有的值——
/// 现查一个真实驱动出来的会话,不猜具体是哪个 `AgentValue` 变体。
fn skills_active_value_for(names: &[&str]) -> AgentValue {
    let mut session = Session::new(root());
    for name in names {
        let _ = session.activate_skill(&root(), SkillId::new(*name));
    }
    let key = AtomKey::Agent(root(), Slot::SkillsActive);
    session
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .expect("Slot::SkillsActive 是一个 source 槽位,build_agent 建图时就该带默认值")
}

/// 直接注入快照(跟既有 `command/restore.rs` 内联测试同一种手法):走的是
/// `Session::restore` 这条独立路径,不依赖 `activate_skill` 是否真的经过
/// `commit_as`——两条路径分别验证「值形状对不对」和「写入路径对不对」。
#[test]
fn a_snapshot_with_skills_active_restores_the_full_id_list() {
    let snapshot =
        vec![(AtomKey::Agent(root(), Slot::SkillsActive), skills_active_value_for(&["alpha", "beta"]))];
    let mut unknown = Vec::new();
    let session = Session::restore(root(), Some(snapshot), Vec::new(), 0, 0, 100, &mut |k| unknown.push(k.clone()))
        .expect("合法快照该能恢复,不该拒绝");

    assert!(unknown.is_empty(), "SkillsActive 是这一版认识的槽位,不该报进 on_unknown_key");
    assert_eq!(
        session.active_skills(),
        vec![SkillId::new("alpha"), SkillId::new("beta")],
        "恢复出来的 active_skills 必须是快照里那份完整 id 列表,顺序原样\
         (写入时已经是确定序,红线 11——读口不该在这里重新洗一遍序)"
    );
}

/// 空集合(从没激活过任何 skill,或者全部被停用)也要能正常恢复出空集合,
/// 不是 panic 也不是被当成「不认识的槽位」。
#[test]
fn an_empty_skills_active_set_restores_to_no_active_skills() {
    let snapshot = vec![(AtomKey::Agent(root(), Slot::SkillsActive), skills_active_value_for(&[]))];
    let session = Session::restore(root(), Some(snapshot), Vec::new(), 0, 0, 100, &mut |_| {})
        .expect("空集合是合法值,不该被拒绝");
    assert!(session.active_skills().is_empty());
}

/// 快照里根本没有 `SkillsActive` 这个键(比如这个会话落盘时这个槽位还没有任何
/// 写入点)——走 `Slot::default_value()` 兜底,同样是空集合,不是恐慌也不是缺省
/// 成"全部激活"这种危险默认值。
#[test]
fn a_missing_skills_active_key_falls_back_to_the_empty_default() {
    let session = Session::restore(root(), None, Vec::new(), 0, 0, 100, &mut |_| {})
        .expect("全新会话,没有任何快照/日志,该能正常建出来");
    assert!(
        session.active_skills().is_empty(),
        "默认值必须是『没有 skill 活跃』,不能悄悄默认成某些 skill 已激活"
    );
}

/// 全链路往返:真实激活两个 skill(横跨两个 turn)→ 取当前全部日志 → 喂进一个
/// 全新 `Session::restore` → `active_skills()` 跟原会话一致。用真实命令产出
/// 日志,而不是手搭快照,这条覆盖的是「激活确实落进了 `Session::primitives()`
/// / 日志」这件事本身,不只是「`Session::restore` 认得这个值形状」。
#[test]
fn restoring_a_real_sessions_full_log_reproduces_its_active_skills() {
    let mut original = Session::new(root());
    let _ = original.activate_skill(&root(), SkillId::new("alpha"));
    original.begin_turn();
    let _ = original.activate_skill(&root(), SkillId::new("beta"));

    let entries: Vec<_> = original.history().entries().cloned().collect();
    let cursor = original.cursor();
    let next_seq = entries.len() as u64;
    assert_eq!(cursor, entries.len(), "这条用例没有 undo 过,游标该跟日志长度一致");

    let restored = Session::restore(root(), None, entries, cursor, next_seq, 100, &mut |_| {})
        .expect("原会话产出的日志必须能被自己重放");

    assert_eq!(
        restored.active_skills(),
        original.active_skills(),
        "重放出来的激活集合必须跟原会话一致"
    );
}
