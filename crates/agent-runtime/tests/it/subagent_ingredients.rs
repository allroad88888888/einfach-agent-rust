//! 子 agent 拿到的**料**长什么样：工具表是宿主表按 `ToolsAllowed` 过滤后的子集，
//! system 多一段固定的「你是被分解出的子任务执行者」，task 是它的第一条 user
//! 消息（029 §注意）。
//!
//! 断言全部对着**假服务器收到的请求体**做——料对不对的唯一判据是模型看到了什么，
//! 不是我们内部某个 `Vec` 长什么样（跟 027 用请求体证明 `/undo` 是同一种证明）。

use crate::support;
use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::{run_turn, ToolTable};

use crate::support::routed::{Route, RoutedServer};

const USAGE_STOP: &str = r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#;

fn text_reply(needle: &'static str, content: &str) -> Route {
    Route::sse(
        needle,
        vec![
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{content}"}},"finish_reason":null}}]}}"#
            ),
            USAGE_STOP.to_string(),
            "data: [DONE]".to_string(),
        ],
    )
}

#[test]
fn a_child_sees_only_the_tools_it_was_given_plus_the_fixed_subagent_system() {
    let dir = support::temp_dir("subagent-ingredients");
    let server = RoutedServer::start(vec![
        text_reply("小结", "汇总完毕。"),
        text_reply("只许读文件", "小结：读完了。"),
        Route::sse(
            "",
            vec![
                r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"srv_3Aagent_2Fspawn","arguments":"{\"task\": \"只许读文件\", \"tools\": [\"srv:fs/read\"]}"}}]}}]}"#,
                r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#,
                "data: [DONE]",
            ],
        ),
    ]);
    let (mut ctx, _events) = support::build_ctx_agent_aware(
        server.port,
        &dir,
        ToolTable::builtin().with_spawn(AgentLimits::default()),
    );
    let mut session = Session::new(AgentId::root());

    assert_eq!(
        run_turn(&mut session, &mut ctx, "找个人去读文件"),
        TurnStatus::Done { truncated: false }
    );

    let root_hop1 = server.call("").expect("root 首跳该被服务过");
    let child = server.call("只许读文件").expect("子 agent 该发过一次请求");

    // —— 工具表：宿主整张表 vs 过滤后的子集 ——————————————————
    for tool in ["srv_3Afs_2Fread", "srv_3Afs_2Flist", "srv_3Aagent_2Fspawn"] {
        assert!(
            root_hop1.body.contains(tool),
            "root 看得见宿主整张表，缺了 {tool}"
        );
    }
    assert!(
        child.body.contains("srv_3Afs_2Fread"),
        "子 agent 该看得见它被分到的工具"
    );
    for denied in ["srv_3Afs_2Flist", "srv_3Aagent_2Fspawn"] {
        assert!(
            !child.body.contains(denied),
            "子 agent 不该看见没分给它的 {denied}——`ToolsAllowed` 是 spawn 当时的快照，不是「现查宿主表」"
        );
    }

    // —— system：固定模板只加在子 agent 头上 ————————————————
    assert!(
        child.body.contains("子任务执行者"),
        "子 agent 的 system 该带那段固定模板"
    );
    assert!(
        !root_hop1.body.contains("子任务执行者"),
        "root 不该带子 agent 的模板"
    );
    assert!(
        !child.body.contains("你是一个简洁"),
        "这条用例的宿主没配 system 分段——真配了的话它会原样在前面，见 subagent::system_for"
    );

    // —— task 走第一条 user 消息，不进 system（红线 11：模板不带任务文本，
    //     兄弟之间的 [Tools][System] 前缀才逐字节相同、缓存才共享）——————
    let system_end = child.body.find("子任务执行者").expect("上面已经断言过它在");
    let task_at = child
        .body
        .find("只许读文件")
        .expect("task 该出现在请求体里");
    assert!(
        task_at > system_end,
        "task 该排在 system 之后（它是一条 user 消息）：{}",
        child.body
    );

    let child_id = AgentId::new("root/a1");
    assert_eq!(
        session.tools_allowed_of(&child_id),
        Some(vec![std::sync::Arc::from("srv:fs/read")]),
        "spawn 当时的工具子集落进了 `Slot::ToolsAllowed`"
    );
    assert!(
        session.tools_allowed_of(&AgentId::root()).is_none(),
        "root 不受子集约束"
    );
}
