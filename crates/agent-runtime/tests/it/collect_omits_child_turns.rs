//! 097 验收第 4 条：M8（后台 spawn + collect）这条路径**同样**只吃子的结论，不吃
//! 子的中间轮次——`subtree.rs` 的模块文档点名 collect 复用的是同一个
//! `child_outcome::outcome`，这里把那句话变成一条会红的断言，外加把红线 10 那道
//! 「父读不到子的正文槽位」的闸从注释钉成一条结构断言。
//!
//! # 为什么 spawn(bg) 和 collect 绑在同一跳
//!
//! 见 `harvest_omits_child_turns_support` 模块文档：这样父在子答完之前不会再发
//! 任何请求，三个子因此严格串行，`spawn_recording_server`（按连接顺序应答）就够
//! 用，不需要按内容路由的并发假服务器——029/052 那批测试要处理的「父的下一跳和
//! 子的第一跳谁先谁后」的竞态，在这个夹具里结构性地不存在。
//!
//! # 三个子的轮数只用来证明「无关」
//!
//! 3、10、20 这三个数字本身不参与任何断言——根消息数的期望值是一个定值：
//! 1 条 kickoff 消息，每个子各贡献一对请求/收敛消息（三个子共六条），
//! 再加 1 条父的收尾，公式里没有 3、10、20 中的任何一个。这正是「根历史的消息
//! 条数与三个子的轮数无关」这句话的操作化：如果哪天父的取料口悄悄拼上了子的
//! 中间轮次，这个定值会随子的轮数变化，断言立刻红。

use crate::harvest_omits_child_turns_support::{n_round_script, spawn_bg_and_collect_call, text_end};
use crate::support;
use agent_core::{AgentId, AgentLimits, ContentBlock, Session, Slot, TurnStatus};
use agent_runtime::{ToolTable, run_turn};

const TASK_A: &str = "TASKA 三轮就答完";
const ANSWER_A: &str = "ANSWERA 三轮的结论";
const TASK_B: &str = "TASKB 十轮才答完";
const ANSWER_B: &str = "ANSWERB 十轮的结论";
const TASK_C: &str = "TASKC 二十轮才答完";
const ANSWER_C: &str = "ANSWERC 二十轮的结论";

#[test]
fn three_background_children_collect_into_a_fixed_shape_no_matter_their_round_counts() {
    let dir = support::temp_dir("collect-omits-child-turns");
    std::fs::write(dir.join("step.txt"), b"step content").unwrap();

    let mut script = vec![spawn_bg_and_collect_call(
        "call_spawn_a",
        "call_collect_a",
        TASK_A,
        "root/a1",
    )];
    script.extend(n_round_script(3, ANSWER_A));
    script.push(spawn_bg_and_collect_call(
        "call_spawn_b",
        "call_collect_b",
        TASK_B,
        "root/a2",
    ));
    script.extend(n_round_script(10, ANSWER_B));
    script.push(spawn_bg_and_collect_call(
        "call_spawn_c",
        "call_collect_c",
        TASK_C,
        "root/a3",
    ));
    script.extend(n_round_script(20, ANSWER_C));
    script.push(text_end("HARVESTALL 三个都收到了"));

    let (port, _bodies) = support::spawn_recording_server(script);
    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_collect();
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff 三个后台子，轮数不同")
        .expect("background collect should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let root_messages = session.messages_of(&root);

    // --- 恰好 3 条「子的结论」ToolResult，内容逐字节等于对应子的固定终答 ---
    //
    // 父这一跳的 ToolResult 总数其实是 6：spawn 那半只回 `{"agent_id":...}`，
    // collect 那半才是子的结论，过滤条件就是「内容是三个固定终答之一」。这三个
    // 字符串是本夹具里唯一能让子答出来的文本（每个子的最后一轮就是这句话，前面
    // 全是工具调用轮次），所以它逐字节等于 `child_outcome::final_text` 会给出的值
    // ——那个函数本身「只捞最后一条 assistant 文本」的断言由 097 的单元测试钉在
    // `child_outcome.rs` 的 `#[cfg(test)]` 里；这里验证的是它确实原样传到了父这
    // 一侧，不多不少。
    let answers = [ANSWER_A, ANSWER_B, ANSWER_C];
    let child_answer_results: Vec<(String, bool)> = root_messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                let text = content.to_string();
                if answers.contains(&text.as_str()) {
                    Some((text, *is_error))
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        child_answer_results.len(),
        3,
        "父历史里该恰好有 3 条子的结论 ToolResult，不随三个子各自的轮数变化：{child_answer_results:#?}"
    );
    for (content, is_error) in &child_answer_results {
        assert!(!is_error, "三个子都成功，不该有 is_error：{content}");
    }
    for answer in answers {
        assert!(
            child_answer_results
                .iter()
                .any(|(content, _)| content == answer),
            "该找到子的结论 {answer:?}：{child_answer_results:#?}"
        );
    }

    // --- 根历史的消息条数是一个不依赖轮数的定值 ---
    //
    // 结构：1 条 kickoff user 消息 + 每个子一对（ToolUse×2 的请求消息 +
    // ToolResult×2 的收敛消息）× 3 个子 + 1 条父的收尾文本 = 1 + 3*2 + 1 = 8。
    assert_eq!(
        root_messages.len(),
        8,
        "根历史消息数该是跟三个子的轮数无关的定值：{root_messages:#?}"
    );

    // --- 子确实说过话，而那些话确实没进父的历史 ---
    //
    // **决策 35 之前这里断言的是相反的事**：`Slot::Messages` 曾是 Upward-only，
    // 父读子的正文在 core 那层被 `NotVisible` 结构性拒绝，这里断言那道闸没被破坏。
    // 横读全开之后 `Messages` 是 `Shared`，父读得到了——所以这段改成断言**真正
    // 重要的那件事**：子的正文非空（它真的跑过好几轮），而父的历史仍然是那个定值。
    //
    // 这比原来那条强：原来只证明「core 不让父读」，现在证明「就算读得到，
    // 也没有一个字漏进父的历史」——而后者才是 `child_outcome.rs` 那条运行时侧
    // 读路要保证的东西（见它的模块文档）。
    let child_a = AgentId::new("root/a1");
    let child_messages = session
        .read_agent(&child_a, Slot::Messages)
        .expect("决策 35 起 Messages 是 Shared，跨 agent 读得到")
        .as_messages()
        .expect("Messages 槽位持 Messages")
        .clone();
    assert!(
        child_messages.len() > 1,
        "子该真的跑过好几轮，否则下面那条「没漏进来」就是空断言：{child_messages:#?}"
    );
    assert_eq!(
        root_messages.len(),
        8,
        "子的轮次一条都不该漏进父的历史"
    );
}
