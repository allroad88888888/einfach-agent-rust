//! 097 验收 1/2：父 agent 出料单时取的是子 agent 的**结论**，不是它的完整
//! history——前台（阻塞）spawn 这条路径上把这条性质锁死。
//!
//! 核查记录（issue 097）：父那一跳的取料口 `session.messages_of(&agent)` 按
//! `AgentId` 参数化，物理上取不到子的那一格；子的产出经 `child_outcome::final_text`
//! 只捞最后一条 assistant 消息里的 `Text` 块，回写成父历史里**一条** tool_result。
//! 这个文件不改实现，只把这条性质变成两条会红的断言：
//!
//! 1. 子固定终答（`ANSWER`）分别跑 5 轮和 20 轮工具调用，父 harvest 之后那一跳
//!    的请求体逐字节相同——父的取料口一旦悄悄拼上子的中间轮次，两条路的请求体
//!    会随轮数飘走，这条立刻红。「子的终答固定」这个前提由 `ANSWER` 常量保证，
//!    两次跑用的是同一个字符串，不是各自现造。
//! 2. 子的 20 轮各打一个唯一标记，父那一跳的请求体一个都不该看到，且终答只
//!    出现一次——直接盯住「中间轮次泄漏」本身，不是只看字节数变没变。

use crate::harvest_omits_child_turns_support::{marker_for, n_round_script, spawn_call, text_end};
use crate::support;
use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::{ToolTable, run_turn};

const TASK: &str = "TASKFIXED 拆给子的固定任务";
const ANSWER: &str = "ANSWERFIXED 子的固定终答";
const HARVEST_REPLY: &str = "HARVESTFIXED 父收到结论后的固定收尾";

/// 跑一次「父阻塞 spawn 一个要做 `rounds` 轮工具调用的子」，返回**父 harvest 之后
/// 那一跳**（也是整条链路的最后一跳）的请求体。
fn harvest_body_after(rounds: u32, tag: &str) -> String {
    let dir = support::temp_dir(tag);
    std::fs::write(dir.join("step.txt"), b"step content").unwrap();

    let mut script = vec![spawn_call(TASK)];
    script.extend(n_round_script(rounds, ANSWER));
    script.push(text_end(HARVEST_REPLY));

    let (port, bodies) = support::spawn_recording_server(script);
    let tools = ToolTable::builtin().with_spawn(AgentLimits::default());
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff 拆给子一个任务")
        .expect("blocking spawn should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    bodies
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("父的 harvest 跳该发生过")
}

/// 验收第 1 条：子的终答固定，5 轮和 20 轮跑出来的父 harvest 请求体逐字节相同。
#[test]
fn five_and_twenty_round_children_yield_byte_identical_harvest_bodies_given_the_same_final_answer()
 {
    let body_5 = harvest_body_after(5, "harvest-5-rounds");
    let body_20 = harvest_body_after(20, "harvest-20-rounds");
    assert_eq!(
        body_5, body_20,
        "子做 5 轮还是 20 轮工具调用，只要终答一样，父 harvest 之后那一跳的请求体\
         就该逐字节相同——飘了就说明父的取料口拼上了子的中间轮次"
    );
}

/// 验收第 2 条：20 个中间标记一个都不泄漏进父 harvest 请求体，终答只出现一次。
#[test]
fn none_of_the_twenty_intermediate_markers_leak_into_the_harvest_body() {
    let body = harvest_body_after(20, "harvest-markers");

    for i in 0..20 {
        let marker = marker_for(i);
        assert!(
            !body.contains(&marker),
            "子第 {i} 轮的标记 {marker} 不该出现在父 harvest 之后那一跳的请求体里：{body}"
        );
    }
    assert_eq!(
        body.matches(ANSWER).count(),
        1,
        "父只该看到子的终答一次（经 tool_result 那一条），不是子每一轮都在场：{body}"
    );
}
