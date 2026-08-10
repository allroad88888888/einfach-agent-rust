//! 103 验收核心（反向锁）：压缩轮之后**紧接着的下一轮**，没有再压缩，
//! `PrefixIntent` 必须回到 `Reuse`；这一轮如果仍然漂移，第 1 层必须判
//! `Unexpected` 并告警——不能因为「上一轮刚压缩过」就继续放行。
//!
//! 只测「压缩轮不告警」（`prefix_intent_tier4_boundary_advance.rs`）盖不住
//! 这条：一个永远传 `Intentional` 的实现，压缩轮那条测试照样全绿，只是把第 1
//! 层永久关掉了而已。要拆穿它，必须在**压缩轮之后**的一轮里制造一次真实漂移，
//! 看它被判成 `Expected`（永久关掉的表现）还是 `Unexpected`（正确表现）。
//!
//! # 怎么在「没有再压缩」的前提下制造真实漂移
//!
//! 103 的 `PrefixIntent` 只看 `send_plan_of` 跟 `prev_send_plan_of`——`drift.rs`
//! 模块文档自己列了压缩之外**另一条**会改前缀、但本 issue 明确不接的来源：
//! 「换 skill 集……都是后面才出现的 `Intentional` 来源」。039 的
//! `Session::activate_skill` 是纯状态命令，不摸 `SendPlan`；`agent-runtime`
//! 拿到激活集之后会把这个 skill 的正文并进下一跳的 `late_system`
//! （`skill_indep_registry_and_activation_e2e.rs` 已经钉死这条链路）。
//!
//! 于是：压缩轮之后，不碰 `SendPlan`、只直接激活一个 skill，下一轮 System 段
//! 就会真的漂——而且是一次跟压缩完全无关的漂移，`send_plan_of` 与
//! `prev_send_plan_of` 全程相等，intent 该读出 `Reuse`。这一轮如果被判成
//! `Expected`，只有一种解释：intent 卡在了压缩轮那次的 `Intentional` 没退回来。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use agent_core::{AgentId, DriftVerdict, Segment, Session, SessionConfig, SkillId};
use agent_providers::deepseek::DeepSeek;
use agent_runtime::{RunnerCtx, RunnerEvent, SkillRegistry, ToolTable, run_turn};
use agent_tools::ToolExecutor;
use agent_transport::{Backoff, Client};

use crate::support;

/// 落一份最小可用的 `skills/testskill/SKILL.md`——frontmatter 形状照抄
/// `skill_indep_registry_and_activation_e2e.rs`（已验证过的已知格式），不带
/// `tools:`（可选字段，本测试只关心 System 段，不需要 late_tools 掺进来把
/// 断言变得两可）。
fn write_test_skill(skills_root: &std::path::Path) {
    let dir = skills_root.join("testskill");
    std::fs::create_dir_all(&dir).unwrap();
    let lines = [
        "---".to_string(),
        "name: testskill".to_string(),
        "description: 反向锁测试用的技能，REVERSELOCK_INDEX_MARKER。".to_string(),
        "---".to_string(),
        "这是 testskill 的正文，激活后应该整段进 late_system。REVERSELOCK_BODY_MARKER。"
            .to_string(),
    ];
    std::fs::write(dir.join("SKILL.md"), lines.join("\n") + "\n").unwrap();
}

/// 跟 `support::build_ctx` 一样返回一份事件收集器——这里不能复用那个共用
/// 助手，因为它的 `system` 参数硬编码成空（跟
/// `skill_indep_registry_and_activation_e2e.rs` 同样的理由：常驻索引要塞
/// 进 `system`），所以按 `RunnerCtx::new` 的公开签名自己装一份。
fn build_ctx(
    port: u16,
    fs_root: &std::path::Path,
    registry: SkillRegistry,
) -> (RunnerCtx, Rc<RefCell<Vec<RunnerEvent>>>) {
    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_millis(50),
        Backoff {
            base: Duration::from_millis(10),
            max_attempts: 1,
        },
    );
    let fs = ToolExecutor::new(fs_root).unwrap();
    let session_config = SessionConfig {
        model: Arc::from("deepseek-v4-pro"),
        temperature: None,
        max_tokens: None,
        context_window: None,
    };
    let index = registry.skill_index_chunk();
    let tools = ToolTable::builtin().with_skills(registry);

    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let ctx = RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(client),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        fs,
        tools,
        vec![index],
        session_config,
        agent_runtime::open_backend(None, |_| {}),
        Box::new(move |ev| sink.borrow_mut().push(ev)),
    );
    (ctx, events)
}

#[test]
fn round_after_compaction_with_no_further_plan_change_flags_real_drift_as_unexpected() {
    let skills_root = support::temp_dir("prefix-intent-reverse-lock-skills");
    write_test_skill(&skills_root);
    let fs_root = support::temp_dir("prefix-intent-reverse-lock-fs");
    let registry = SkillRegistry::load(&[skills_root]).expect("加载测试用 skill 目录不该失败");

    let port = support::spawn_scripted_server(vec![
        support::sse_text("第一轮回复"),
        support::sse_text("压缩轮回复"),
        support::sse_text("激活 skill 之后的一轮"),
    ]);
    let (mut ctx, events) = build_ctx(port, &fs_root, registry);
    let (mut session, root) = (Session::new(AgentId::root()), AgentId::root());

    // 轮 1：建立一份 PrevPrefix 镜像。
    run_turn(&mut session, &mut ctx, "第一句话").expect("第一轮不该是 source failure");

    // 轮 2：压缩开火（第 4 档）。这一轮的判读已经在
    // `prefix_intent_tier4_boundary_advance.rs` 单独钉过，这里只是把「上一轮
    // 刚压缩过」这个前提搭出来。
    let history_len = session.messages_of(&root).len();
    session.begin_turn();
    session
        .advance_boundary(&root, history_len, None)
        .expect("边界从 0 推到 history_len 该被接受");
    run_turn(&mut session, &mut ctx, "继续").expect("压缩轮不该是 source failure");

    // 轮 3（反向锁那一轮）：**不碰 SendPlan**——直接激活一个 skill，制造一次跟
    // 压缩无关的真实 System 段漂移。
    session.begin_turn();
    session
        .activate_skill(&root, SkillId::new("testskill"))
        .expect("激活一个从没激活过的 skill 不该被拒");
    run_turn(&mut session, &mut ctx, "激活之后的一轮").expect("第三轮不该是 source failure");

    let events = events.borrow();
    let last_guard = events
        .iter()
        .filter_map(|e| match e {
            RunnerEvent::TurnGuard { report, .. } => Some(report),
            _ => None,
        })
        .last()
        .unwrap_or_else(|| panic!("激活 skill 之后那一轮该有一份 GuardReport：{events:#?}"));

    assert_eq!(
        last_guard.drift,
        DriftVerdict::Unexpected {
            segment: Segment::System
        },
        "这一轮 send_plan_of 跟 prev_send_plan_of 全程没变过（没有再压缩），intent \
         该读出 Reuse；激活 skill 让 System 段真的漂了，Reuse + 漂移必须判 \
         Unexpected，不能因为上一轮刚压缩过就继续放行成 Expected：{events:#?}"
    );
}
