//! 097 两份集成测试共用的假响应构造件：子 agent 连续做 N 轮「文本 + 工具调用」，
//! 每轮打一个唯一标记（`CHILD_STEP_NN`），最后一轮换成纯文本收尾。
//!
//! 复用点：`blocking_spawn_omits_child_turns.rs`（前台阻塞 spawn 路径）和
//! `collect_omits_child_turns.rs`（M8 后台 spawn+collect 路径）都需要「子做 N 轮
//! 才答」这同一个夹具，只是外面套的 spawn 方式不同。
//!
//! 全程用 `support::spawn_recording_server`（**严格按连接顺序**应答，不按内容
//! 路由）：整条链路里同一时刻只有一个 agent 在打请求——前台 spawn 天然阻塞；
//! 后台这边把 spawn(bg) 和 collect 绑进同一跳（见
//! [`spawn_bg_and_collect_call`] 的文档），父在子答完之前不会再发下一跳。
//! 029/052 那批测试要按内容路由的并发假服务器，是因为那边父故意不等，两边会抢
//! 连接——这里的夹具结构性地不需要。
#![allow(dead_code)]

use crate::support::ScriptedResponse;

/// 第 `i` 轮的唯一标记。**两位补零**：这样 `CHILD_STEP_04` 和 `CHILD_STEP_19`
/// 长度相同，「终答长度跟轮数无关」这条断言（`child_outcome.rs` 的单元测试同款）
/// 才有意义。
pub fn marker_for(i: u32) -> String {
    format!("CHILD_STEP_{i:02}")
}

/// 一轮「文本 + 工具调用」：同一条 assistant 消息里**同时**带 `content` 文本和
/// `tool_calls`——DeepSeek 录制帧证明这是合法形状（`agent-providers` 的
/// `recorded_parallel_tool_calls` 测试：思考+文本+两次调用同一条消息）。标记各
/// 出现一次在文本里、一次在工具入参里，对应验收第 2 条要断言的两个位置。
pub fn tool_round(marker: &str, call_id: &str) -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        Box::leak(
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{marker} 继续处理","tool_calls":[{{"index":0,"id":"{call_id}","type":"function","function":{{"name":"srv_3Afs_2Fread","arguments":"{{\"path\": \"step.txt\", \"marker\": \"{marker}\"}}"}}}}]}}}}]}}"#
            )
            .into_boxed_str(),
        ),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":10}}"#,
        "data: [DONE]",
    ])
}

/// 一条纯文本收尾（`StopReason::EndTurn`）：子的终答，或者父的收尾。
pub fn text_end(text: &str) -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        Box::leak(
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{text}"}},"finish_reason":null}}]}}"#
            )
            .into_boxed_str(),
        ),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":10}}"#,
        "data: [DONE]",
    ])
}

/// 子 agent 完整的一段脚本：`rounds` 轮工具调用（各带唯一标记）+ 1 轮纯文本收尾
/// （固定为 `answer`）。**收尾文本不随轮数变化**——这正是验收第 1 条要压的前提：
/// 5 轮和 20 轮跑出来的 `answer` 必须一字不差，两次跑的父历史才有比较的意义。
pub fn n_round_script(rounds: u32, answer: &str) -> Vec<ScriptedResponse> {
    let mut script: Vec<ScriptedResponse> = (0..rounds)
        .map(|i| {
            let marker = marker_for(i);
            let call_id = format!("call_r{i:02}");
            tool_round(&marker, &call_id)
        })
        .collect();
    script.push(text_end(answer));
    script
}

/// 前台（阻塞）spawn 的第一跳：一次 `srv:agent/spawn` 调用，`task` 固定、
/// 不带 `background`（缺省 = 阻塞，等子干完）。
pub fn spawn_call(task: &str) -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        Box::leak(
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":null,"tool_calls":[{{"index":0,"id":"call_spawn","type":"function","function":{{"name":"srv_3Aagent_2Fspawn","arguments":"{{\"task\": \"{task}\"}}"}}}}]}}}}]}}"#
            )
            .into_boxed_str(),
        ),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":10}}"#,
        "data: [DONE]",
    ])
}

/// 后台 spawn + collect **绑在同一跳**：两个 `tool_calls` 一次给全。
///
/// 这不是凑巧的省事写法——它是让整条链路保持严格串行、从而能用
/// `spawn_recording_server`（按连接顺序应答）的关键：spawn(bg) 当场收敛，但
/// collect 绑住了父的另一个槽，父要等子答完才能两个槽都收敛、才能发下一跳；
/// 于是父在子回来之前不会再发任何请求——同一时刻只有子在打请求，没有「父的下一
/// 跳」和「子的第一跳」谁先谁后的竞态（对照 `spawn_bg_two_children_no_block.rs`
/// 需要按内容路由的并发假服务器，正是因为那边父不等，两边会抢连接）。
pub fn spawn_bg_and_collect_call(
    spawn_call_id: &str,
    collect_call_id: &str,
    task: &str,
    collect_id: &str,
) -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        Box::leak(
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":null,"tool_calls":[{{"index":0,"id":"{spawn_call_id}","type":"function","function":{{"name":"srv_3Aagent_2Fspawn","arguments":"{{\"task\": \"{task}\", \"background\": true}}"}}}},{{"index":1,"id":"{collect_call_id}","type":"function","function":{{"name":"srv_3Aagent_2Fcollect","arguments":"{{\"id\": \"{collect_id}\"}}"}}}}]}}}}]}}"#
            )
            .into_boxed_str(),
        ),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":10}}"#,
        "data: [DONE]",
    ])
}
