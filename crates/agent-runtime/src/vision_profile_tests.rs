use std::collections::BTreeMap;
use std::sync::Arc;

use agent_core::{AgentId, ChildConfig, ExecutionProfileId, Session, SessionConfig, SystemChunk};
use agent_providers::deepseek::DeepSeek;
use agent_providers::kimi::Kimi;
use agent_tools::ToolExecutor;
use agent_transport::Client;

use super::is_enabled;
use crate::ctx::RunnerCtx;
use crate::execution_binding::ExecutionBinding;
use crate::subagent;
use crate::tool_table::ToolTable;

fn config(model: &str) -> SessionConfig {
    SessionConfig {
        model: Arc::from(model),
        temperature: None,
        max_tokens: None,
        context_window: None,
    }
}

fn context(provider: Arc<dyn agent_providers::Provider>) -> RunnerCtx {
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "https://api.deepseek.com/chat/completions".to_string(),
        "default-key".to_string(),
        ToolExecutor::new(std::env::temp_dir()).unwrap(),
        ToolTable::builtin(),
        vec![SystemChunk {
            label: Arc::from("host"),
            text: Arc::from("HOST_SYSTEM_CANARY"),
        }],
        config("deepseek-test"),
        crate::persist::open_backend(None, |_| {}),
        Box::new(|_| {}),
    )
    .with_execution_bindings(BTreeMap::from([(
        ExecutionProfileId::new("vision"),
        ExecutionBinding::new(
            provider,
            Arc::new(Client::new()),
            "https://vision.example/chat/completions".to_string(),
            "vision-key".to_string(),
            config("vision-test"),
        ),
    )]))
}

#[test]
fn nonvisual_binding_cannot_enable_the_vision_facade() {
    assert!(!is_enabled(&context(Arc::new(DeepSeek))));
}

#[test]
fn vision_child_system_excludes_host_context() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let child = session
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: Vec::new(),
                execution_profile: Some(ExecutionProfileId::new("vision")),
                max_retries: Some(0),
            },
        )
        .unwrap();
    let ctx = context(Arc::new(Kimi));

    let system = subagent::system_for(&session, &ctx, &child);
    assert_eq!(system.len(), 1);
    assert!(!system[0].text.contains("HOST_SYSTEM_CANARY"));
    assert!(system[0].text.contains("isolated vision inspection worker"));
}
