//! 027 验收第一条：假 SSE 服务器走两轮（第一轮含一次工具调用），`/undo`
//! （这里直接调 `Session::undo_turn`，CLI 层的 `/undo` 就是这一行）之后——
//! 上一轮消息消失、`primitives()` 逐值回到那一轮开始之前、下一轮真的
//! `encode` 一次，body 字节里不含被退轮次的内容（缓存兜底第 1 层顺带验证
//! 前缀回退正确——`prev_prefix` 也是 primitive，undo 会把它带回上一轮末尾
//! 那份镜像）。

mod support;

use agent_core::{AgentId, RequestIntent, Session, SessionConfig, TurnStatus, UndoReport};
use agent_providers::{Ingredients, Provider, deepseek::DeepSeek};

use support::ScriptedResponse;

fn hop1_tool_use() -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"srv_3Afs_2Fread","arguments":"{\"path\": \"hello.txt\"}"}}]}}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#,
        "data: [DONE]",
    ])
}

fn hop2_end_turn() -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"第一轮的回答"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":150,"completion_tokens":10,"prompt_cache_hit_tokens":64,"prompt_cache_miss_tokens":86}}"#,
        "data: [DONE]",
    ])
}

fn second_turn_reply() -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"第二轮独有的秘密回答"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":200,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":200}}"#,
        "data: [DONE]",
    ])
}

#[test]
fn undo_after_two_turns_erases_the_second_and_the_next_request_does_not_carry_it() {
    let dir = support::temp_dir("undo-after-turns");
    std::fs::write(dir.join("hello.txt"), b"hello world").unwrap();

    let port = support::spawn_scripted_server(vec![hop1_tool_use(), hop2_end_turn(), second_turn_reply()]);
    let (mut ctx, _events) = support::build_ctx(port, &dir);
    let mut session = Session::new(AgentId::root());

    // 第一轮：一次工具调用 + 收尾。四条消息。
    let status = agent_runtime::run_turn(&mut session, &mut ctx, "读一下 hello.txt，秘密指令 ALPHA");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert_eq!(session.messages().len(), 4);
    let snapshot_after_turn_1 = session.primitives();

    // 第二轮：显式 begin_turn（`run_turn` 不替调用方决定，见它的文档）。
    session.begin_turn();
    let status = agent_runtime::run_turn(&mut session, &mut ctx, "第二轮独有的问题 BETA");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert_eq!(session.messages().len(), 6, "两轮各两条/四条消息叠加");

    // /undo：CLI 就是直接调这一行。
    let report = session.undo_turn();
    assert!(matches!(report, UndoReport::Applied { turn_id: 2, .. }), "{report:?}");

    // 上一轮消失：消息数、每一份 primitive 都跟第一轮结束时逐值相等。
    assert_eq!(session.messages().len(), 4, "第二轮的两条消息该被退掉");
    assert_eq!(session.primitives(), snapshot_after_turn_1, "undo 一整轮后所有 primitive 逐值回退");
    assert_eq!(session.status(), TurnStatus::Done { truncated: false }, "退回到第一轮收尾时的状态");

    // 下一轮 prompt 不含被退内容：真的 encode 一次，断言 body 字节。
    // provider/model 跟 `support::build_ctx` 建 `RunnerCtx` 时用的是同一家
    // （DeepSeek / deepseek-v4-pro）——`ctx` 的字段是 crate 内私有的，测试
    // 拿不到，这里直接复刻同一份 `SessionConfig` 现造一次 `encode`。
    let messages: Vec<agent_core::Message> = session.messages().iter().cloned().collect();
    let prev_prefix = session.prev_prefix();
    let config = SessionConfig { model: std::sync::Arc::from("deepseek-v4-pro"), temperature: None, max_tokens: None, context_window: None };
    let ing = Ingredients {
        system: &[],
        messages: &messages,
        tools: &[],
        late_tools: &[],
        late_system: &[],
        config: &config,
        intent: RequestIntent::Free,
        prev_prefix: prev_prefix.as_ref(),
    };
    let encoded = DeepSeek.encode(&ing);
    let body = String::from_utf8(encoded.body).unwrap();
    assert!(body.contains("ALPHA"), "第一轮的内容该还在下一次请求里：{body}");
    assert!(!body.contains("BETA"), "被退掉的第二轮用户提问不该出现在下一次请求里：{body}");
    assert!(!body.contains("秘密回答"), "被退掉的第二轮助手回复不该出现在下一次请求里：{body}");
}
