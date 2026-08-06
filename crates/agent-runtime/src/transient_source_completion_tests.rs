use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use agent_core::{
    AgentId, ContentBlock, DriftVerdict, Epoch, Event, PrefixImage, SessionConfig, StopReason,
    ToolCallId,
};
use agent_providers::deepseek::DeepSeek;
use agent_tools::ToolExecutor;
use agent_transport::{Client, StreamOutcome, TransportError};
use serde_json::json;

use crate::RunnerCtx;
use crate::TransientSourceFailure;
use crate::event::RunnerEvent;
use crate::execution_binding::GuardScope;
use crate::tool_table::ToolTable;
use crate::transient_source_completion::{Metadata, finish};
use crate::transient_source_policy::{SAFE_CANDIDATE, SOURCE_READ};
use crate::transient_source_vault::CapturedSource;

const PRIVATE_INPUT: &str = "private-input-7b7d";
const PRIVATE_CANDIDATE: &str =
    "核心逻辑位于 src/private/auth.rs:42\nfn secret_impl() {}\nprivate-candidate-9f3a";
const PRIVATE_THINKING: &str = "private-thinking-e2c1";

fn test_ctx() -> (RunnerCtx, Rc<RefCell<Vec<RunnerEvent>>>) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&events);
    let ctx = RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "http://127.0.0.1:1/chat/completions".to_string(),
        "fake-key".to_string(),
        ToolExecutor::new(std::env::temp_dir()).unwrap(),
        ToolTable::builtin(),
        Vec::new(),
        SessionConfig {
            model: Arc::from("deepseek-v4-flash"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        crate::persist::open_backend(None, |_| {}),
        Box::new(move |event| observed.borrow_mut().push(event)),
    );
    (ctx, events)
}

fn metadata(agent: AgentId) -> Metadata {
    Metadata {
        agent,
        epoch: Epoch::START,
        guard_scope: GuardScope::INITIAL,
        drift: DriftVerdict::Clean,
        predicted_cache: 0,
        adjustments: Vec::new(),
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
    }
}

fn seed_private_source(ctx: &mut RunnerCtx, agent: &AgentId) -> ToolCallId {
    let call_id = ToolCallId::new("source-call");
    ctx.transient_sources
        .capture_batch(vec![CapturedSource {
            agent: agent.clone(),
            epoch: Epoch::START,
            call_id: call_id.clone(),
            tool: Arc::from(SOURCE_READ),
            input: Arc::new(json!({"opaque": PRIVATE_INPUT})),
            reasoning: Some(Arc::from(PRIVATE_THINKING)),
        }])
        .unwrap();
    assert!(
        ctx.transient_sources
            .raw_input(agent, Epoch::START, &call_id, SOURCE_READ)
            .is_some()
    );
    call_id
}

fn assert_purged(ctx: &RunnerCtx, agent: &AgentId, call_id: &ToolCallId) {
    assert!(
        ctx.transient_sources
            .raw_input(agent, Epoch::START, call_id, SOURCE_READ)
            .is_none()
    );
}

#[test]
fn sensitive_terminal_candidate_reaches_only_the_private_event_boundary() {
    let (mut ctx, events) = test_ctx();
    let agent = AgentId::root();
    let call_id = seed_private_source(&mut ctx, &agent);

    let event = finish(
        &mut ctx,
        metadata(agent.clone()),
        Ok(StreamOutcome::Finished),
        vec![
            ContentBlock::Thinking(Arc::from(PRIVATE_THINKING)),
            ContentBlock::Text(Arc::from(PRIVATE_CANDIDATE)),
        ],
        StopReason::EndTurn,
    )
    .expect("a valid transient-source completion should succeed");

    let Event::ProviderDone { blocks, stop, .. } = &event else {
        panic!("expected provider success: {event:?}");
    };
    assert_eq!(*stop, StopReason::EndTurn);
    assert!(matches!(
        blocks.as_slice(),
        [ContentBlock::Text(text)] if &**text == SAFE_CANDIDATE
    ));
    assert!(!format!("{event:?}").contains(PRIVATE_CANDIDATE));
    assert_purged(&ctx, &agent, &call_id);

    let events = events.borrow();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RunnerEvent::TextDelta(_)))
            .count(),
        1
    );
    assert!(events.iter().any(
        |event| matches!(event, RunnerEvent::TextDelta(text) if &**text == PRIVATE_CANDIDATE)
    ));
    assert!(!format!("{events:?}").contains(PRIVATE_THINKING));
}

#[test]
fn invalid_terminal_shape_fails_without_releasing_partial_text() {
    let (mut ctx, events) = test_ctx();
    let agent = AgentId::root();
    let call_id = seed_private_source(&mut ctx, &agent);
    let failure = finish(
        &mut ctx,
        metadata(agent.clone()),
        Ok(StreamOutcome::Finished),
        vec![
            ContentBlock::Text(Arc::from(PRIVATE_CANDIDATE)),
            ContentBlock::ToolResult {
                id: ToolCallId::new("invalid"),
                content: Arc::from(PRIVATE_INPUT),
                is_error: false,
            },
        ],
        StopReason::EndTurn,
    )
    .expect_err("an invalid transient-source completion must fail");

    assert!(matches!(
        failure,
        TransientSourceFailure::InvalidCompletion {
            agent: failure_agent,
            epoch: Epoch::START,
        } if failure_agent == agent
    ));
    assert_purged(&ctx, &agent, &call_id);
    let emitted = format!("{:?}", events.borrow());
    assert!(!emitted.contains(PRIVATE_CANDIDATE));
    assert!(!emitted.contains(PRIVATE_INPUT));
    assert!(events.borrow().is_empty());
}

#[test]
fn transport_failure_purges_source_and_preserves_provider_body() {
    let (mut ctx, events) = test_ctx();
    let agent = AgentId::root();
    let call_id = seed_private_source(&mut ctx, &agent);
    let failure = finish(
        &mut ctx,
        metadata(agent.clone()),
        Err(TransportError::Http {
            status: 500,
            body: PRIVATE_CANDIDATE.to_string(),
        }),
        vec![ContentBlock::Text(Arc::from(PRIVATE_CANDIDATE))],
        StopReason::EndTurn,
    )
    .expect_err("a transport failure must leave the runtime as the original error");

    assert!(matches!(
        failure,
        TransientSourceFailure::Transport {
            agent: failure_agent,
            epoch: Epoch::START,
            error: TransportError::Http { status: 500, body },
        } if failure_agent == agent && body == PRIVATE_CANDIDATE
    ));
    assert_purged(&ctx, &agent, &call_id);
    let emitted = format!("{:?}", events.borrow());
    assert!(!emitted.contains(PRIVATE_CANDIDATE));
    assert!(events.borrow().is_empty());
}

#[test]
fn cancellation_purges_source_without_emitting_the_candidate() {
    let (mut ctx, events) = test_ctx();
    let agent = AgentId::root();
    let call_id = seed_private_source(&mut ctx, &agent);
    let event = finish(
        &mut ctx,
        metadata(agent.clone()),
        Ok(StreamOutcome::Cancelled),
        vec![ContentBlock::Text(Arc::from(PRIVATE_CANDIDATE))],
        StopReason::EndTurn,
    )
    .expect("a cancellation is still a session event");

    assert!(matches!(event, Event::Cancel { .. }));
    assert_purged(&ctx, &agent, &call_id);
    assert!(events.borrow().is_empty());
}
