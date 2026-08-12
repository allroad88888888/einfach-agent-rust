//! 141 独立测试（线级）：老会话（M13 期真被激活过、正文里带一个哨兵串）恢复之后，
//! **新一轮请求体不含一个正文字节**——issue 141 §验收「老会话数据恢复不 panic；
//! 恢复后下一轮 encode body 不含任何 skill 正文」的直接落点。
//!
//! 断言的落点是**假服务器真的收到的请求体**（跟 `skill_switch_wire_indep.rs`
//! 同一种证明手法：料对不对的唯一判据是模型看到了什么，不是内部某个函数存不
//! 存在）——这比只断言「那个曾经把激活集展开成注入料的方法不存在」更硬：即便
//! 有人在 141 之后又从别的角落悄悄把正文塞回 system/messages/tools，这条测试
//! 也会红。
//!
//! # 怎么造「老数据」
//!
//! `Session::activate_skill` 这个写入命令本身已经随 141 删除（`agent-core` 的
//! `command/skill.rs` 只剩只读口），没有任何公开 API 能再产出一条
//! `activate_skill` journal entry。于是这里跟
//! `tool_table_skill_assembly_tests.rs::a_restored_session_with_a_journaled_activation_no_longer_has_any_injection_path`
//! 同一个手法：先用公开的 `declare_host_skills` 得到一份正确编码的快照，
//! 再手改 `Slot::SkillsActive` 那一项——写的是 `value::str_set` 的既有编码形状
//! （排序去重的字符串数组），不猜测别的形状。恢复之后，宿主重建 registry 的
//! 方式也照抄生产代码的路径：`SkillRegistry::from_host_skills(session.host_skills())`
//! （`agent-server` 的 `actor::capabilities` 与 `agent-cli` 的恢复路都是这么接的）。

use std::sync::Arc;

use agent_core::{AgentId, AgentValue, AtomKey, HostSkill, Session, SkillId, Slot, TurnStatus};
use agent_runtime::{SkillRegistry, ToolTable, run_turn};

use crate::support;

const SKILL_ID: &str = "legacy-crm-flow";
const SENTINEL: &str = "LEGACY_SKILL_BODY_SENTINEL_KP73";

fn old_declaration() -> Vec<HostSkill> {
    vec![HostSkill {
        id: SkillId::new(SKILL_ID),
        description: Arc::from("M13 期声明的客户工单流程"),
        body: Arc::from(format!("先查档案再关单。{SENTINEL}")),
        tools: Vec::new(),
        tool_reversibility: Default::default(),
    }]
}

/// 一份「已声明 + 已激活」的老快照：`declare_host_skills` 走公开命令得到正确
/// 编码，`SkillsActive` 手改成已激活——模拟 M13 期真实存在过、141 之前才可能
/// 产出的状态组合（今天已经没有命令能再写出这个组合）。
fn old_session_snapshot() -> Vec<(AtomKey, AgentValue)> {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    session.declare_host_skills(old_declaration());

    let active_key = AtomKey::Agent(root, Slot::SkillsActive);
    let mut values = session.primitives();
    for (key, value) in values.iter_mut() {
        if *key == active_key {
            *value = AgentValue::Json(Arc::new(serde_json::json!([SKILL_ID])));
        }
    }
    values
}

#[test]
fn a_restored_sessions_next_turn_body_carries_zero_bytes_of_the_old_activated_skills_body() {
    let snapshot = old_session_snapshot();

    let mut unknown = Vec::new();
    let mut restored = Session::restore(
        AgentId::root(),
        Some(snapshot),
        Vec::new(),
        0,
        0,
        100,
        agent_core::AgentLimits::default(),
        &mut |k| unknown.push(k.clone()),
    )
    .expect("含 SkillsActive + HostSkills 的老快照必须能被今天的代码重放，不 panic、不拒绝");
    assert!(
        unknown.is_empty(),
        "SkillsActive/HostSkills 都是留壳的既有槽位，不该报进 on_unknown_key：{unknown:?}"
    );
    assert_eq!(
        restored.active_skills(),
        vec![SkillId::new(SKILL_ID)],
        "恢复出来的激活集必须原样带回这个 id——状态还在（红线 4 的留壳承诺）"
    );

    // 宿主重启后重建 registry 的既有路径：从恢复出来的 `HostSkills` 声明现造。
    let registry = SkillRegistry::from_host_skills(restored.host_skills());
    assert_eq!(
        registry.body_of(SKILL_ID).as_deref(),
        Some(&*format!("先查档案再关单。{SENTINEL}")),
        "registry 里那份正文必须原样带着哨兵串——这条测试要证的是『取得到但没人拿去注入』，\
         不是『正文本身丢了』"
    );
    let table = ToolTable::builtin().with_skills(registry);

    let dir = support::temp_dir("skill-old-activation-no-injection-fs");
    let (port, bodies) = support::spawn_recording_server(vec![support::sse_text("好的，继续吧。")]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, table);

    let status = run_turn(&mut restored, &mut ctx, "老会话恢复后的第一句新话")
        .expect("这一轮不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let bodies = bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        1,
        "该正好录到一条请求体，实际: {}",
        bodies.len()
    );
    assert!(
        !bodies[0].contains(SENTINEL),
        "恢复出来的老会话曾经激活过这个 skill，但 141 删了那条把激活集展开成注入料\
         的机制——新一轮的请求体（system/messages/tools 任何位置）都不该出现它的\
         正文一个字节：{}",
        bodies[0]
    );
}
