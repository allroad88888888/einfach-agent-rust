//! 039 独立测试(agent-core 层):`activate_skill` / `deactivate_skill` 必须走
//! command 层、journaled(红线 2/4)——undo 一步能把激活/停用原样退掉,靠的是
//! `Session::undo_turn` 本来就有的通用机制,不是靠给 skill 写一份专门的 undo
//! 代码。这份文件只钉一件事:**激活/停用记不记账、undo/redo 认不认账**。
//!
//! 独立测试 agent 规则(与 001/024/026 同一条):这份文件只依据
//! `docs/issues/039-skills-loading.md`、`docs/TOOLS.md` §Skills、
//! `docs/STATE-MODEL.md`(`skills_active` 槽位)、红线 2/4、以及
//! `crates/agent-core/src/lib.rs` 的公开签名写成,**不看**
//! `crates/agent-core/src/command/` 里 039 新增的实现体。
//!
//! 假定的公开签名(未见实现体,接口经文档 + 既有 `spawn_child` / `mark_irreversible`
//! 同款命令的约定推定;若与实现不符,记进独立测试报告的「分歧」,不改这里也不改 src):
//!
//! ```ignore
//! pub struct SkillId(pub Arc<str>); // 派生集合比照 ToolCallId:Clone+PartialEq+Eq+
//!                                   // PartialOrd+Ord+Hash+Debug+Serialize+Deserialize
//! impl SkillId { pub fn new(id: impl Into<Arc<str>>) -> Self; }
//!
//! impl Session {
//!     pub fn activate_skill(&mut self, agent: &AgentId, skill: SkillId) -> _;   // 返回值形状不确定
//!     pub fn deactivate_skill(&mut self, agent: &AgentId, skill: SkillId) -> _; // 同上
//!     pub fn active_skills(&self) -> Vec<SkillId>;
//! }
//! ```
//!
//! 返回值形状不确定(`bool`/`Result<_,_>`/`()`都有可能),全部用 `let _ =` 接住——
//! 这份测试要钉的是「journaled + undo 能退」,不是返回值契约本身。

use agent_core::{AgentId, Session, SkillId, UndoReport};

fn root() -> AgentId {
    AgentId::root()
}

#[test]
fn activating_a_skill_appends_one_history_entry_and_shows_up_in_active_skills() {
    let mut session = Session::new(root());
    let before = session.history_len();

    let _ = session.activate_skill(&root(), SkillId::new("testskill"));

    assert_eq!(
        session.history_len(),
        before + 1,
        "激活必须走 command 层,留下一条 Entry(红线 2)——不能是绕过 undo log 的裸写"
    );
    assert!(
        session.active_skills().contains(&SkillId::new("testskill")),
        "激活后 active_skills 该含它,实际: {:?}",
        session.active_skills()
    );
}

#[test]
fn undo_turn_reverts_activation_and_active_skills_no_longer_contains_it() {
    let mut session = Session::new(root());
    let _ = session.activate_skill(&root(), SkillId::new("testskill"));
    assert!(!session.active_skills().is_empty(), "激活该先生效,不然下面这条 undo 断言没有意义");

    let report = session.undo_turn();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "激活是 Reversible 的(TOOLS.md 判据:有明确补偿动作——反激活),undo_turn 不该被挡,实际: {report:?}"
    );
    assert!(
        session.active_skills().is_empty(),
        "undo 之后 active_skills 不该再含被撤销的那个——这是 command 层白拿的证据,\
         没有任何 skill 专门的 undo 代码在起作用"
    );
}

#[test]
fn redo_turn_reapplies_the_activation() {
    let mut session = Session::new(root());
    let _ = session.activate_skill(&root(), SkillId::new("testskill"));
    let _ = session.undo_turn();
    assert!(session.active_skills().is_empty(), "undo 后应该已经不活跃");

    let report = session.redo_turn();
    assert!(matches!(report, UndoReport::Applied { .. }), "redo 该把激活重放回来: {report:?}");
    assert!(
        session.active_skills().contains(&SkillId::new("testskill")),
        "redo 之后该重新含它,实际: {:?}",
        session.active_skills()
    );
}

#[test]
fn deactivating_an_active_skill_also_journals_and_undo_restores_it() {
    let mut session = Session::new(root());
    let _ = session.activate_skill(&root(), SkillId::new("alpha"));
    // 开新一轮,让停用落在一条**不同**的 turn 上——否则 undo_turn 会把这一整轮
    // (激活+停用)一起退掉,测不出「单独退停用」这件事。
    session.begin_turn();
    let before = session.history_len();

    let _ = session.deactivate_skill(&root(), SkillId::new("alpha"));
    assert_eq!(session.history_len(), before + 1, "停用同样要留下一条 Entry");
    assert!(
        !session.active_skills().contains(&SkillId::new("alpha")),
        "停用之后不该再活跃,实际: {:?}",
        session.active_skills()
    );

    let report = session.undo_turn();
    assert!(matches!(report, UndoReport::Applied { .. }), "{report:?}");
    assert!(
        session.active_skills().contains(&SkillId::new("alpha")),
        "撤销停用之后该重新活跃,实际: {:?}",
        session.active_skills()
    );
}

/// 激活两个不同 skill,undo 一次只退最近那一步(同一个 turn 里发生的话是一起退,
/// 这里刻意用 `begin_turn` 隔开,验证「退的是我撤的那一步,不是全部」)。
#[test]
fn undo_only_reverts_the_activation_in_the_turn_being_undone() {
    let mut session = Session::new(root());
    let _ = session.activate_skill(&root(), SkillId::new("alpha"));
    session.begin_turn();
    let _ = session.activate_skill(&root(), SkillId::new("beta"));

    let _ = session.undo_turn();

    let active = session.active_skills();
    assert!(active.contains(&SkillId::new("alpha")), "更早那轮的激活不该被这次 undo 波及: {active:?}");
    assert!(!active.contains(&SkillId::new("beta")), "最近那轮的激活该被退掉: {active:?}");
}
