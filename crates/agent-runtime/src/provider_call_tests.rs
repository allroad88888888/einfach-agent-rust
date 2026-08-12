use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agent_core::{
    AgentId, ChildConfig, DriftVerdict, ErrorClass, ExecutionProfileId, PrefixImage, Session,
    SessionConfig, StopReason, TokenUsage,
};
use agent_providers::deepseek::DeepSeek;
use agent_providers::{Decoded, Encoded, Ingredients, Provider, StreamAccumulator};
use agent_tools::ToolExecutor;
use agent_transport::{Client, TransportError};
use serde_json::Value;
// 114b：`ProviderCall.deadline` 的字段类型已经是 `web_time::Instant`（见
// `provider_call.rs`），这里显式跟着改来源，避免和 `use super::*` 隐式带进来
// 的同名类型只是「刚好同一个」而非「本来就该一个」。
use web_time::Instant;

use super::*;
use crate::tool_table::ToolTable;

struct ClassifiedProvider(ErrorClass);

impl Provider for ClassifiedProvider {
    fn encode(&self, ingredients: &Ingredients<'_>) -> Encoded {
        DeepSeek.encode(ingredients)
    }

    fn decode(&self, body: &Value) -> Decoded {
        DeepSeek.decode(body)
    }

    fn accumulator(&self) -> StreamAccumulator {
        DeepSeek.accumulator()
    }

    fn classify(&self, _: u16, _: &str) -> ErrorClass {
        self.0.clone()
    }
}

fn config(model: &str) -> SessionConfig {
    SessionConfig {
        model: Arc::from(model),
        temperature: None,
        max_tokens: None,
        context_window: None,
    }
}

fn build(provider: Arc<dyn Provider>) -> RunnerCtx {
    RunnerCtx::new(
        provider,
        Arc::new(Client::new()),
        "http://127.0.0.1:1/chat/completions".to_string(),
        "test-key".to_string(),
        ToolExecutor::new(std::env::temp_dir()).unwrap(),
        ToolTable::builtin(),
        Vec::new(),
        config("test-model"),
        crate::persist::open_backend(None, |_| {}),
        Box::new(|_| {}),
    )
}

#[test]
fn missing_named_profile_fails_before_provider_start() {
    let root = AgentId::root();
    let profile = ExecutionProfileId::new("unconfigured");
    let mut session = Session::new(root.clone());
    let child = session
        .spawn_child(
            &root,
            ChildConfig {
                execution_profile: Some(profile),
                ..ChildConfig::default()
            },
            None,
        )
        .unwrap();
    let mut ctx = build(Arc::new(DeepSeek));
    // 117 接线：`start` 的第三个参数从「泵 channel 的发送端」换成了 IO 总线。
    let bus = crate::io_bus::IoBus::new(Duration::from_millis(20));

    let failure = match start(&session, &mut ctx, &bus, child.clone(), Epoch::START) {
        Err(failure) => failure,
        Ok(_) => panic!("unconfigured profile must not start an HTTP call"),
    };

    assert!(matches!(
        failure,
        StartFailure::Event(Event::ProviderFailed {
            agent,
            epoch: Epoch::START,
            class: ErrorClass::Unknown,
            ..
        }) if agent == child
    ));
}

#[test]
fn call_cancel_latch_survives_a_later_session_flag_reset() {
    let session_cancel = AtomicBool::new(false);
    let call_cancel = Arc::new(AtomicBool::new(session_cancel.load(Ordering::Relaxed)));
    let ctx = build(Arc::new(DeepSeek));
    let selection = ctx.execution_binding_for(None).unwrap();
    let call = ProviderCall {
        agent: AgentId::root(),
        attempt: ProviderAttemptId::allocate(),
        epoch: Epoch::START,
        deadline: Instant::now() + Duration::from_secs(1),
        binding: selection.binding,
        guard_scope: selection.guard_scope,
        drift: DriftVerdict::Clean,
        predicted_cache: 0,
        adjustments: Vec::new(),
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        one_shot: false,
        hold_deltas: false,
        cancel_token: Arc::clone(&call_cancel),
    };

    session_cancel.store(true, Ordering::Relaxed);
    call.cancel();
    session_cancel.store(false, Ordering::Relaxed);

    assert!(call_cancel.load(Ordering::Relaxed));
}

#[test]
fn finish_uses_start_binding_after_default_switch() {
    let root = AgentId::root();
    let mut ctx = build(Arc::new(ClassifiedProvider(ErrorClass::Auth)));
    let selection = ctx.execution_binding_for(None).unwrap();
    let call = ProviderCall {
        agent: root.clone(),
        attempt: ProviderAttemptId::allocate(),
        epoch: Epoch::START,
        deadline: Instant::now() + Duration::from_secs(1),
        binding: selection.binding,
        guard_scope: selection.guard_scope,
        drift: DriftVerdict::Clean,
        predicted_cache: 0,
        adjustments: Vec::new(),
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        one_shot: false,
        hold_deltas: false,
        cancel_token: Arc::new(AtomicBool::new(false)),
    };
    ctx.switch_provider(
        Arc::new(ClassifiedProvider(ErrorClass::BadRequest)),
        "http://127.0.0.1:2/chat/completions".to_string(),
        "new-key".to_string(),
        Arc::from("new-model"),
        None,
    );

    let event = finish(
        &mut ctx,
        call,
        Err(TransportError::Http {
            status: 418,
            body: "old-flight".to_string(),
        }),
        Vec::new(),
        StopReason::EndTurn,
        TokenUsage {
            prompt: 0,
            completion: 0,
            cached: None,
        },
    )
    .expect("an ordinary provider failure should remain a session event");

    assert!(matches!(
        event,
        Event::ProviderFailed {
            agent,
            class: ErrorClass::Auth,
            ..
        } if agent == root
    ));
}

/// 一趟默认 provider 请求起飞后，即使 `/model` 切走、它才成功落地，也只能给
/// 旧 binding 的窗口记账；新默认 binding 从一张干净窗口开始。
#[test]
fn old_default_finish_does_not_contaminate_new_default_guard_scope() {
    let root = AgentId::root();
    let mut ctx = build(Arc::new(DeepSeek));
    let selection = ctx.execution_binding_for(None).unwrap();
    let old_scope = selection.guard_scope;
    let call = ProviderCall {
        agent: root.clone(),
        attempt: ProviderAttemptId::allocate(),
        epoch: Epoch::START,
        deadline: Instant::now() + Duration::from_secs(1),
        binding: selection.binding,
        guard_scope: old_scope,
        drift: DriftVerdict::Clean,
        predicted_cache: 0,
        adjustments: Vec::new(),
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        one_shot: false,
        hold_deltas: false,
        cancel_token: Arc::new(AtomicBool::new(false)),
    };

    ctx.switch_provider(
        Arc::new(ClassifiedProvider(ErrorClass::BadRequest)),
        "http://127.0.0.1:2/chat/completions".to_string(),
        "new-key".to_string(),
        Arc::from("new-model"),
        None,
    );
    let new_scope = ctx.execution_binding_for(None).unwrap().guard_scope;

    let event = finish(
        &mut ctx,
        call,
        Ok(agent_transport::StreamOutcome::Finished),
        Vec::new(),
        StopReason::EndTurn,
        TokenUsage {
            prompt: 100,
            completion: 10,
            cached: Some(64),
        },
    )
    .expect("an ordinary provider completion should remain a session event");

    assert!(matches!(event, Event::ProviderDone { agent, .. } if agent == root));
    assert_ne!(old_scope, new_scope);
    assert_eq!(ctx.guard_history_for(old_scope).len(), 1);
    assert!(ctx.guard_history_for(new_scope).is_empty());
}
