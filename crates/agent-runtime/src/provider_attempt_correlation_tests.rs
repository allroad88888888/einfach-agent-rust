use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use agent_core::{DriftVerdict, PrefixImage, SessionConfig, StopReason, TokenUsage};
use agent_providers::deepseek::DeepSeek;
use agent_tools::ToolExecutor;
use agent_transport::{Client, StreamOutcome};

use super::*;
use crate::event::RunnerEvent;
use crate::image_preparation_failure::ImagePreparationFailure;
use crate::provider_attempt::ProviderAttemptId;
use crate::provider_message::{self, ProviderMessage};
use crate::tool_table::ToolTable;

fn config() -> SessionConfig {
    SessionConfig {
        model: Arc::from("test-model"),
        temperature: None,
        max_tokens: None,
        context_window: None,
    }
}

fn build() -> (RunnerCtx, Rc<RefCell<Vec<RunnerEvent>>>) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&events);
    let ctx = RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "http://127.0.0.1:1/chat/completions".to_string(),
        "test-key".to_string(),
        ToolExecutor::new(std::env::temp_dir()).unwrap(),
        ToolTable::builtin(),
        Vec::new(),
        config(),
        crate::persist::open_backend(None, |_| {}),
        Box::new(move |event| observed.borrow_mut().push(event)),
    );
    (ctx, events)
}

fn call(ctx: &RunnerCtx, attempt: ProviderAttemptId) -> ProviderCall {
    let selection = ctx.execution_binding_for(None).unwrap();
    ProviderCall {
        agent: AgentId::root(),
        attempt,
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
        replay_sanitized_deltas: false,
        redact_provider_errors: false,
        cancel_token: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn stale_messages_cannot_touch_same_agent_retry_in_same_epoch() {
    let (mut ctx, events) = build();
    let stale = ProviderAttemptId::allocate();
    let current = ProviderAttemptId::allocate();
    let agent = AgentId::root();
    let mut calls = vec![call(&ctx, current)];
    let mut pending = VecDeque::new();

    let messages = [
        ProviderMessage::delta(
            agent.clone(),
            stale,
            RunnerEvent::TextDelta(Arc::from("late")),
        ),
        ProviderMessage::done(
            agent.clone(),
            stale,
            Ok(StreamOutcome::Cancelled),
            Vec::new(),
            StopReason::EndTurn,
            TokenUsage {
                prompt: 0,
                completion: 0,
                cached: None,
            },
            Vec::new(),
        ),
        ProviderMessage::preparation_failed(
            agent.clone(),
            stale,
            ImagePreparationFailure::Cancelled,
        ),
        ProviderMessage::gone(agent, stale),
    ];

    for message in messages {
        provider_message::land(&mut ctx, &mut calls, &mut pending, message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].attempt, current);
        assert!(pending.is_empty());
        assert!(events.borrow().is_empty());
    }
    assert!(
        ctx.take_image_preparation_failure(&AgentId::root())
            .is_none()
    );
}
