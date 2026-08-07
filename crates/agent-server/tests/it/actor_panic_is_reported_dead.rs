//! 验收清单第六条：actor 内 panic → 进程活着、事件流收到终态、registry 报
//! dead。用一个故意在 `encode` 里 panic 的假 `Provider` 触发——`encode` 是
//! `provider_call::execute` 拿到 effect 后第一个调用的东西，不需要真的连上
//! 任何网络就能可靠地把 panic 打穿 `run_turn` → actor 的命令循环。
//!
//! 这条测试本身能跑到断言、能正常退出，就是「进程没被拖垮」最直接的证明
//! ——如果 `catch_unwind` 没接住，这个测试进程会直接被这个 panic 干掉，
//! 连断言都不会跑到。

use crate::support;
use std::sync::Arc;
use std::time::Duration;

use agent_core::ErrorClass;
use agent_providers::{Decoded, Encoded, Ingredients, Provider, StreamAccumulator};
use agent_server::{Command, SessionEvent, SessionId, SessionQuery};
use agent_transport::Client;
use serde_json::Value;

/// `encode` 一被调用就 panic——够触发这个测试要的「actor 线程真的挂了」，
/// 不需要构造任何真实的模型响应。其余方法永远不会被走到，`unreachable!()`
/// 占位即可（`!` 能强转成任何返回类型）。
struct PanicProvider;

impl Provider for PanicProvider {
    fn encode(&self, _ing: &Ingredients<'_>) -> Encoded {
        panic!("boom-from-test-provider")
    }
    fn decode(&self, _body: &Value) -> Decoded {
        unreachable!()
    }
    fn accumulator(&self) -> StreamAccumulator {
        unreachable!()
    }
    fn classify(&self, _status: u16, _body: &str) -> ErrorClass {
        unreachable!()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panicking_provider_kills_only_the_actor_thread_and_registry_reports_it_dead() {
    let id = SessionId::from("doomed");
    let spec = agent_server::OpenSpec {
        id: id.clone(),
        store_path: None,
        provider: Arc::new(PanicProvider),
        endpoint: "http://127.0.0.1:1/unused".to_string(), // 永远不会真的连——`encode` 先 panic。
        api_key: "fake-key".to_string(),
        model: Arc::from("deepseek-v4-pro"),
        tools: agent_server::ToolTableSpec::Builtin,
        tools_root: support::temp_dir("panic-tools"),
        system: Vec::new(),
        client: Arc::new(Client::new()),
        history_cap: None,
        snapshot_every: Some(0),
        provider_timeout: Some(Duration::from_secs(5)),
        remote_tool_timeout: None,
        vision: None,
        host_tools: Vec::new(),
        host_skills: Vec::new(),
        disable_builtin: Vec::new(),
    };

    let registry = agent_server::SessionRegistry::new();
    let handle = registry
        .open(spec)
        .expect("open 阶段还没碰到 provider，不该失败");

    let mut sub = handle.subscribe();
    handle
        .send(Command::Input {
            text: "trigger the panic".to_string(),
        })
        .unwrap();

    let died_reason = loop {
        let frame = tokio::time::timeout(Duration::from_secs(3), sub.recv())
            .await
            .expect("该在几秒内收到 SessionDied")
            .expect("事件流不该在收到 SessionDied 之前结束");
        if let SessionEvent::SessionDied { reason } = frame.event {
            // 034：SessionDied 是 actor/连接级的事实，标 root（`crate::event::
            // frame` 模块文档同一条判据）——这里顺带钉住这个归属，不只是拆包。
            assert_eq!(frame.agent.as_str(), "root", "SessionDied 该标 root");
            break reason;
        }
    };
    assert!(
        died_reason.contains("boom-from-test-provider"),
        "{died_reason}"
    );

    match registry.get(&id) {
        Some(SessionQuery::Dead { reason }) => {
            assert!(reason.contains("boom-from-test-provider"), "{reason}")
        }
        other => panic!(
            "registry 该报 dead，不是静默移除或者说它还活着：{}",
            matches!(other, Some(SessionQuery::Alive(_)))
        ),
    }

    match registry.close(&id) {
        Err(agent_server::CloseError::WasDead { reason }) => {
            assert!(reason.contains("boom-from-test-provider"), "{reason}")
        }
        other => panic!(
            "close 该如实报告『它已经死了』，而不是假装关闭成功：{:?}",
            other.is_ok()
        ),
    }
}
