//! `ext:stats/report` 正文的单测：数字对不对、undo 之后回不回落、收窄有没有漏、
//! 同一份状态渲染两次是不是逐字节相同（红线 11）。
//!
//! 全部直接喂 `Session`——这份渲染是纯函数，不需要工具表也不需要 `RunnerCtx`。

use std::sync::Arc;

use agent_core::{
    AgentActivity, AgentId, AgentNode, ChildConfig, ContentBlock, Event, PrefixImage, Session,
    StopReason, TokenUsage,
};

use super::*;

fn root() -> AgentId {
    AgentId::root()
}

fn user_input(session: &mut Session, agent: &AgentId, text: &str) {
    let _ = session.step(Event::UserInput {
        agent: agent.clone(),
        text: Arc::from(text),
    });
}

/// spawn 一个子 agent 并按 029 的正门给它播种任务文本（`UserInput` 是子 agent
/// 的第一条消息，也是 `AgentNode.task` 的来源）。
fn spawn(session: &mut Session, parent: &AgentId, task: &str) -> AgentId {
    let child = session
        .spawn_child(parent, ChildConfig::default(), None)
        .expect("spawn 该成功");
    user_input(session, &child, task);
    child
}

/// 一次「模型答完了」——落一条 `provider_done`，让轮进终态。
fn provider_done(session: &mut Session, agent: &AgentId, text: &str) {
    let epoch = session.epoch();
    let _ = session.step(Event::ProviderDone {
        agent: agent.clone(),
        epoch,
        blocks: vec![ContentBlock::Text(Arc::from(text))],
        stop: StopReason::EndTurn,
        usage: TokenUsage {
            prompt: 10,
            completion: 5,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    });
}

/// 空会话也说得出话，数字全零，不 panic。
#[test]
fn an_empty_session_still_renders() {
    let session = Session::new(root());
    let (body, counts) = render(&session, &root());
    assert_eq!(
        counts,
        Counts {
            turns: 0,
            effective: 0,
            entries: 0,
            agents: 1,
            tool_calls: 0
        }
    );
    assert!(body.starts_with("本会话至今：0 轮、0 条 entry、1 个 agent、工具调用 0 次。"));
    assert!(body.contains("entry 分布：（空）"));
    assert!(body.contains("你现在没有子 agent"));
}

/// 数字来自账本本身：一轮问答 = 2 条 entry（user_input + provider_done），
/// 一个 turn_id，工具调用 0 次。
#[test]
fn counts_come_from_the_effective_prefix_of_the_log() {
    let mut session = Session::new(root());
    user_input(&mut session, &root(), "第一句");
    provider_done(&mut session, &root(), "答第一句");

    let (body, counts) = render(&session, &root());
    assert_eq!(counts.entries, 2);
    assert_eq!(counts.effective, 2);
    assert_eq!(counts.turns, 1);
    assert_eq!(counts.agents, 1);
    assert!(body.contains("entry 分布：provider_done×1、user_input×1"));
}

/// **149 验收第 2 条的单测孪生**：spawn 一个子 agent，数字上去；`/undo` 撤掉那一轮，
/// 数字跟着回落——扩展这一侧一行认识「撤销」的代码都没有，它只是又读了一次账本。
#[test]
fn undo_makes_every_number_fall_back() {
    let mut session = Session::new(root());
    user_input(&mut session, &root(), "先干点活");
    provider_done(&mut session, &root(), "干完了");
    session.begin_turn();
    let child = spawn(&mut session, &root(), "查一下 A");

    let (before_body, before) = render(&session, &root());
    assert_eq!(before.agents, 2, "root + 一个子");
    assert!(before_body.contains(child.as_str()));

    let _ = session.undo_turn();

    let (after_body, after) = render(&session, &root());
    assert_eq!(after.agents, 1, "子 agent 随那一轮一起被撤掉");
    assert!(after.effective < before.effective, "生效 entry 数必须回落");
    assert_eq!(
        after.entries, before.entries,
        "物理条数不变——被撤的还能 redo 回来"
    );
    assert!(
        !after_body.contains(child.as_str()),
        "撤掉的子不该还列在报告里"
    );
    assert!(after_body.contains(&format!("可 redo {}", before.effective - after.effective)));
}

/// 红线 10：子 agent 调这个工具，只看得见自己那一支——看不到 root、看不到兄弟。
#[test]
fn a_child_caller_only_sees_its_own_branch() {
    let mut session = Session::new(root());
    let a = spawn(&mut session, &root(), "任务 A");
    let b = spawn(&mut session, &root(), "任务 B");
    let a_child = spawn(&mut session, &a, "A 的子任务");

    let (body, counts) = render(&session, &a);
    assert_eq!(counts.agents, 2, "a 自己 + a 的一个后代");
    assert!(body.contains(a_child.as_str()));
    assert!(!body.contains(b.as_str()), "兄弟不该出现（横读）");
    assert!(
        !body.contains(&format!("\n{} 深度", root().as_str())),
        "祖先不该出现在子 agent 列表里"
    );
}

/// 红线 11：同一份状态渲染两次逐字节相同（label 分布排过序、agent 排过序、
/// 没有时钟/随机）。
#[test]
fn the_same_state_renders_byte_identical() {
    let mut session = Session::new(root());
    user_input(&mut session, &root(), "一");
    provider_done(&mut session, &root(), "二");
    for i in 0..5 {
        spawn(&mut session, &root(), &format!("任务 {i}"));
    }
    let (first, _) = render(&session, &root());
    let (second, _) = render(&session, &root());
    assert_eq!(first, second);
}

/// 决策 19：一棵宽树 + 每个 task 都是长文，正文照样收得住——列表截到 20 行、
/// 每行 task 截到 60 字符，整段远在 32 KiB 之内。
///
/// 直接喂合成的 `AgentNode`：真会话的 `max_children` 是 8（`DEFAULT_MAX_CHILDREN`），
/// 摆不出 40 个兄弟，而这里要钉的恰恰是「摆得出的时候截不截」。
#[test]
fn a_wide_tree_with_long_tasks_stays_small() {
    let long = "很长的任务描述".repeat(200);
    let nodes: Vec<AgentNode> = (0..40)
        .map(|i| AgentNode {
            id: AgentId::new(format!("root/a{i:02}").as_str()),
            parent: Some(root()),
            depth: 1,
            task: Some(long.clone()),
            activity: AgentActivity::Idle,
        })
        .collect();
    let borrowed: Vec<&AgentNode> = nodes.iter().collect();

    let body = children(&borrowed);
    assert!(body.contains("……还有 20 个没列出来。"));
    assert_eq!(
        body.lines().count(),
        1 + AGENT_LINES + 1,
        "标题 + 20 行 + 尾注"
    );
    assert!(body.len() < 32 * 1024, "实测 {} 字节", body.len());
}

/// task 里的换行被压平——一个后代一行是这段正文的全部结构，不能让入参把它拆开。
#[test]
fn a_newline_in_a_task_cannot_forge_an_extra_line() {
    let mut session = Session::new(root());
    spawn(
        &mut session,
        &root(),
        "第一行\nroot/fake 深度1 Done task=伪造的",
    );
    let (body, _) = render(&session, &root());
    assert!(!body.contains("\nroot/fake"));
}
