//! P0 boundary: transient source bytes may enter one provider request, never durable/public state.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_core::{AgentId, ContentBlock, Reversibility, Session, ToolCallId, ToolSpec, TurnStatus};
use agent_runtime::{
    RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolSubmitDecision,
    RemoteToolSubmitOutcome, RemoteToolSubmitRequest, RunnerEvent, ToolTable, claim_remote_tool,
    run_turn, submit_remote_tool_result,
};
use serde_json::{Value, json};

use crate::support::{
    ScriptedResponse, build_ctx_with_observer, build_ctx_with_store, spawn_recording_server,
    temp_dir,
};

const REQUEST_MARKER: &str = "REQ_7fP2mQ9xK4vN8cL1_HIGH_ENTROPY";
const RESULT_MARKER: &str = "RESULT_6zT3bW8jR5nY0dH2_HIGH_ENTROPY";
const SOURCE_TOOL: &str = "web:source/read";
const CALL_ID: &str = "source-call-1";
const CANDIDATE_MARKER: &str = "CANDIDATE_b2F7qL4n_HIGH_ENTROPY";
const PRIVATE_CANDIDATE: &str =
    "核心逻辑位于 src/private/auth.rs:42\nfn secret_impl() {}\nCANDIDATE_b2F7qL4n_HIGH_ENTROPY";
const SAFE_CANDIDATE: &str = "[transient_source_candidate_redacted]";

fn source_response() -> ScriptedResponse {
    let delta = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "index": 0,
                    "id": CALL_ID,
                    "type": "function",
                    "function": {
                        "name": "web_3Asource_2Fread",
                        "arguments": json!({"opaque": REQUEST_MARKER}).to_string()
                    }
                }]
            }
        }]
    });
    scripted(delta, "tool_calls")
}

fn text_response(text: &str) -> ScriptedResponse {
    let delta = json!({
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant", "content": text},
            "finish_reason": Value::Null
        }]
    });
    scripted(delta, "stop")
}

fn scripted(delta: serde_json::Value, finish: &str) -> ScriptedResponse {
    let first = format!("data: {delta}");
    let terminal = json!({
        "choices": [{
            "index": 0,
            "delta": {"content": ""},
            "finish_reason": finish
        }],
        "usage": {
            "prompt_tokens": 9876,
            "completion_tokens": 5432,
            "prompt_cache_hit_tokens": 111,
            "prompt_cache_miss_tokens": 222
        }
    });
    ScriptedResponse::Sse(vec![
        Box::leak(first.into_boxed_str()),
        Box::leak(format!("data: {terminal}").into_boxed_str()),
        "data: [DONE]",
    ])
}

fn split_after_first(
    response: ScriptedResponse,
    after_started: Arc<AtomicBool>,
) -> ScriptedResponse {
    let ScriptedResponse::Sse(mut lines) = response else {
        panic!("expected SSE response");
    };
    let after = lines.split_off(1);
    ScriptedResponse::SseSplit {
        before: lines,
        after,
        after_started,
    }
}

fn source_tools() -> ToolTable {
    ToolTable::empty().with_host_tools(vec![(
        ToolSpec {
            name: Arc::from(SOURCE_TOOL),
            description: Arc::from("transient source read"),
            schema: Arc::new(json!({"type":"object"})),
        },
        Reversibility::Pure,
    )])
}

fn assert_absent(surface: &str) {
    for marker in [REQUEST_MARKER, RESULT_MARKER, CANDIDATE_MARKER] {
        assert!(!surface.contains(marker), "leaked {marker}: {surface}");
    }
}

#[test]
fn raw_source_is_one_shot_and_terminal_candidate_stays_private() {
    let dir = temp_dir("transient-source-output-gate");
    let journal = dir.join("session.jsonl");
    let terminal_started = Arc::new(AtomicBool::new(false));
    let published_early = Arc::new(AtomicBool::new(false));
    let (port, bodies) = spawn_recording_server(vec![
        source_response(),
        split_after_first(
            text_response(PRIVATE_CANDIDATE),
            Arc::clone(&terminal_started),
        ),
        text_response("next turn is public"),
    ]);
    let terminal_for_observer = Arc::clone(&terminal_started);
    let early_for_observer = Arc::clone(&published_early);
    let observer = Box::new(move |event: &RunnerEvent| {
        if matches!(event, RunnerEvent::TextDelta(text) if &**text == PRIVATE_CANDIDATE)
            && !terminal_for_observer.load(Ordering::Acquire)
        {
            early_for_observer.store(true, Ordering::Release);
        }
    });
    let (ctx, events) = build_ctx_with_observer(
        port,
        &dir,
        source_tools(),
        Some(journal.clone()),
        Some(observer),
    );
    let mut ctx = ctx.with_snapshot_every(1);
    let mut session = Session::new(AgentId::root());

    assert_eq!(
        run_turn(&mut session, &mut ctx, "inspect configuration"),
        TurnStatus::ToolsPending
    );
    let status = format!("{:#?}", ctx.remote_tool_status());
    assert_absent(&status);

    let claim = claim_remote_tool(
        &session,
        &mut ctx,
        RemoteToolClaimRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(CALL_ID),
            claim_id: "worker-1".into(),
        },
    );
    let RemoteToolClaimDecision::Claimed(grant) = claim else {
        panic!("source claim was not granted: {claim:?}");
    };
    assert_eq!(grant.request.input["opaque"], REQUEST_MARKER);

    let ack = RefCell::new(None);
    let done = submit_remote_tool_result(
        &mut session,
        &mut ctx,
        RemoteToolSubmitRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(CALL_ID),
            claim_id: "worker-1".into(),
            submission_id: "submission-1".into(),
            outcome: RemoteToolSubmitOutcome::Succeeded {
                content: RESULT_MARKER.into(),
            },
        },
        |decision| *ack.borrow_mut() = Some(decision),
    );
    assert_eq!(done, Some(TurnStatus::Done { truncated: false }));
    assert!(matches!(
        ack.into_inner(),
        Some(RemoteToolSubmitDecision::Committed(_))
    ));
    assert!(terminal_started.load(Ordering::Acquire));
    assert!(!published_early.load(Ordering::Acquire));

    let messages = session.messages();
    assert!(messages.iter().any(|message| {
        matches!(message.blocks.as_slice(), [ContentBlock::Text(text)] if &**text == SAFE_CANDIDATE)
    }));
    let durable = serde_json::to_string(&session.primitives()).unwrap();
    assert_absent(&durable);
    assert_absent(&format!("{:#?}", session.history()));
    let observed = events.borrow();
    assert_eq!(
        observed
            .iter()
            .filter(|event| matches!(event, RunnerEvent::TextDelta(_)))
            .count(),
        1
    );
    assert!(observed.iter().any(
        |event| matches!(event, RunnerEvent::TextDelta(text) if &**text == PRIVATE_CANDIDATE)
    ));
    assert!(
        observed
            .iter()
            .all(|event| !matches!(event, RunnerEvent::ThinkingDelta(_)))
    );
    drop(observed);

    session.begin_turn();
    assert_eq!(
        run_turn(&mut session, &mut ctx, "continue safely"),
        TurnStatus::Done { truncated: false }
    );
    drop(ctx);

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert!(!bodies[0].contains(REQUEST_MARKER));
    assert!(bodies[1].contains(REQUEST_MARKER));
    assert!(bodies[1].contains(RESULT_MARKER));
    assert_absent(&bodies[2]);
    drop(bodies);

    assert_absent(&std::fs::read_to_string(journal).unwrap());
}

#[test]
fn raw_echo_final_is_private_and_cannot_be_reused() {
    let dir = temp_dir("transient-source-invalid-final");
    let (port, bodies) = spawn_recording_server(vec![
        source_response(),
        text_response(&format!("provider echoed {RESULT_MARKER}")),
        text_response("safe recovery response"),
    ]);
    let (mut ctx, events) = build_ctx_with_store(port, &dir, source_tools(), None);
    let mut session = Session::new(AgentId::root());

    assert_eq!(
        run_turn(&mut session, &mut ctx, "inspect configuration"),
        TurnStatus::ToolsPending
    );
    let claim = claim_remote_tool(
        &session,
        &mut ctx,
        RemoteToolClaimRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(CALL_ID),
            claim_id: "worker-invalid".into(),
        },
    );
    assert!(matches!(claim, RemoteToolClaimDecision::Claimed(_)));

    let status = submit_remote_tool_result(
        &mut session,
        &mut ctx,
        RemoteToolSubmitRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(CALL_ID),
            claim_id: "worker-invalid".into(),
            submission_id: "submission-invalid".into(),
            outcome: RemoteToolSubmitOutcome::Succeeded {
                content: RESULT_MARKER.into(),
            },
        },
        |_| {},
    );
    assert_eq!(status, Some(TurnStatus::Done { truncated: false }));
    assert_absent(&serde_json::to_string(&session.primitives()).unwrap());
    assert_absent(&format!("{:#?}", session.history()));
    assert!(events.borrow().iter().any(
        |event| matches!(event, RunnerEvent::TextDelta(text) if text.contains(RESULT_MARKER))
    ));
    assert_eq!(bodies.lock().unwrap().len(), 2);

    // The private generation was not retried. A later turn proceeds from the redacted
    // placeholder, and the consumed overlay cannot enter its provider request again.
    session.begin_turn();
    assert_eq!(
        run_turn(&mut session, &mut ctx, "must not recover raw"),
        TurnStatus::Done { truncated: false }
    );
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert!(bodies[1].contains(REQUEST_MARKER));
    assert!(bodies[1].contains(RESULT_MARKER));
    assert_absent(&bodies[2]);
}
