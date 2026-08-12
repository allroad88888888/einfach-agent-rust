//! 崩溃/重启时，旧进程没来得及回传的远端工具必须直接收敛为取消终态，绝不重放。

use std::time::Duration;

use agent_core::{AgentId, TurnStatus};
use agent_server::{Command, SessionEvent, ToolTableSpec};

use crate::support;
use crate::support::server::{FakeServer, Script};

fn browser_action_reply() -> String {
    [
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_restart_1","type":"function","function":{"name":"browser_action","arguments":"{\"action\":\"restart-canary\"}"}}]},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}]}"#,
        "data: [DONE]",
        "",
    ]
    .join("\n\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_terminally_cancels_recovered_pending_tools_without_a_new_command() {
    let upstream = FakeServer::start(vec![Script::Immediate(browser_action_reply())]);
    let store_path = support::temp_dir("recovered-pending-tools").join("session.jsonl");
    let registry = agent_server::SessionRegistry::new();
    let mut spec = support::open_spec("recovered", upstream.endpoint(), Some(store_path.clone()));
    spec.tools = ToolTableSpec::Standard;
    let handle = registry.open(spec).unwrap();
    let mut events = handle.subscribe();
    handle
        .send(Command::Input {
            text: "调用浏览器".to_owned(),
        })
        .unwrap();

    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("应收到远端工具事件")
            .expect("actor 不能提前退出");
        if matches!(frame.event, SessionEvent::ToolExecuting { .. }) {
            break;
        }
    }
    assert_eq!(handle.pending_remote_tools().len(), 1);
    registry
        .close(&agent_server::SessionId::from("recovered"))
        .unwrap();

    // open() 本身就是恢复触发点；此后没有发送任何 HTTP/actor 命令。
    let handle = registry
        .open(support::open_spec(
            "recovered",
            upstream.endpoint(),
            Some(store_path.clone()),
        ))
        .unwrap();
    assert!(handle.pending_remote_tools().is_empty());
    registry
        .close(&agent_server::SessionId::from("recovered"))
        .unwrap();

    let backend = agent_runtime::open_backend(Some(store_path), |_| {});
    let recovered = agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        agent_core::AgentLimits::default(),
        &mut |_| panic!("不应出现未知持久化键"),
    )
    .unwrap()
    .expect("重启后的 journal 应保留会话");
    assert!(matches!(recovered.status(), TurnStatus::Failed(_)));
    assert!(!agent_runtime::has_unresolved_tool_calls(&recovered));
    assert_eq!(
        upstream.request_count(),
        1,
        "恢复不能重放远端工具或 provider 调用"
    );
}
