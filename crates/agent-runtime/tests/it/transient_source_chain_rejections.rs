//! A private source continuation scrubs text and rejects non-source tool calls.

use std::cell::RefCell;
use std::sync::Arc;

use agent_core::{AgentId, Reversibility, Session, ToolCallId, ToolSpec, TurnStatus};
use agent_runtime::{
    RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolSubmitDecision,
    RemoteToolSubmitOutcome,
    RemoteToolSubmitRequest, RunnerEvent, ToolTable, TransientSourceFailure, claim_remote_tool,
    run_turn, submit_remote_tool_result,
};
use serde_json::{Value, json};

use crate::support::{
    ScriptedResponse, build_ctx_with_store, spawn_recording_server, sse_tool_call, temp_dir,
};

const SOURCE: &str = "web:source/read";
const OTHER: &str = "web:client/inspect";
const INITIAL_CALL: &str = "rejection-source";
const PRIVATE_INPUT: &str = "SYNTH_REJECT_INPUT_90d1";
const PRIVATE_RESULT: &str = "SYNTH_REJECT_RESULT_ab72";
const PRIVATE_TEXT: &str = "SYNTH_REJECT_TEXT_e301";
const PRIVATE_THINKING: &str = "SYNTH_REJECT_THINKING_6f28";

fn tool_table() -> ToolTable {
    let specs = [SOURCE, OTHER].into_iter().map(|name| {
        (
            ToolSpec {
                name: Arc::from(name),
                description: Arc::from("synthetic test tool"),
                schema: Arc::new(json!({"type":"object"})),
            },
            Reversibility::Pure,
        )
    });
    ToolTable::empty().with_host_tools(specs.collect())
}

fn mixed_text_source_response() -> ScriptedResponse {
    let delta = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "reasoning_content": PRIVATE_THINKING,
                "content": PRIVATE_TEXT,
                "tool_calls": [{
                    "index": 0,
                    "id": "mixed-source",
                    "type": "function",
                    "function": {
                        "name": "web_3Asource_2Fread",
                        "arguments": json!({"opaque":"next"}).to_string()
                    }
                }]
            }
        }]
    });
    scripted(delta)
}

fn scripted(delta: Value) -> ScriptedResponse {
    let terminal = json!({
        "choices": [{
            "index": 0,
            "delta": {"content": ""},
            "finish_reason": "tool_calls"
        }],
        "usage": {
            "prompt_tokens": 23,
            "completion_tokens": 11,
            "prompt_cache_hit_tokens": 0,
            "prompt_cache_miss_tokens": 23
        }
    });
    ScriptedResponse::Sse(vec![
        Box::leak(format!("data: {delta}").into_boxed_str()),
        Box::leak(format!("data: {terminal}").into_boxed_str()),
        "data: [DONE]",
    ])
}

fn assert_text_mixed_source_call_is_scrubbed() {
    let dir = temp_dir("transient-source-mixed-text");
    let (port, bodies) = spawn_recording_server(vec![
        sse_tool_call(
            INITIAL_CALL,
            "web_3Asource_2Fread",
            r#"{\"opaque\":\"SYNTH_REJECT_INPUT_90d1\"}"#,
        ),
        mixed_text_source_response(),
    ]);
    let (mut ctx, events) = build_ctx_with_store(port, &dir, tool_table(), None);
    let mut session = Session::new(AgentId::root());
    assert_eq!(
        run_turn(&mut session, &mut ctx, "synthetic text-scrub")
            .expect("initial source request is not a terminal source failure"),
        TurnStatus::ToolsPending
    );

    let claim = claim_remote_tool(
        &session,
        &mut ctx,
        RemoteToolClaimRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(INITIAL_CALL),
            claim_id: "text-scrub-worker".into(),
        },
    );
    assert!(matches!(claim, RemoteToolClaimDecision::Claimed(_)));

    let acknowledgement = RefCell::new(None);
    let status = submit_remote_tool_result(
        &mut session,
        &mut ctx,
        RemoteToolSubmitRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(INITIAL_CALL),
            claim_id: "text-scrub-worker".into(),
            submission_id: "text-scrub-submission".into(),
            outcome: RemoteToolSubmitOutcome::Succeeded {
                content: PRIVATE_RESULT.into(),
            },
        },
        |decision| *acknowledgement.borrow_mut() = Some(decision),
    )
    .expect("a source continuation with text must stay private and continue");
    assert_eq!(status, Some(TurnStatus::ToolsPending));
    assert!(matches!(
        acknowledgement.into_inner(),
        Some(RemoteToolSubmitDecision::Committed(_))
    ));

    let next_claim = claim_remote_tool(
        &session,
        &mut ctx,
        RemoteToolClaimRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new("mixed-source"),
            claim_id: "text-scrub-next-worker".into(),
        },
    );
    let RemoteToolClaimDecision::Claimed(next_grant) = next_claim else {
        panic!("text-scrubbed continuation was not claimed: {next_claim:?}");
    };
    assert_eq!(next_grant.request.input["opaque"], "next");
    assert_eq!(bodies.lock().unwrap().len(), 2);

    let durable = serde_json::to_string(&session.primitives()).unwrap();
    let history = format!("{:#?}", session.history());
    let emitted = format!("{:#?}", events.borrow());
    for marker in [
        PRIVATE_INPUT,
        PRIVATE_RESULT,
        PRIVATE_TEXT,
        PRIVATE_THINKING,
    ] {
        assert!(!durable.contains(marker));
        assert!(!history.contains(marker));
        assert!(!emitted.contains(marker));
    }
    assert!(events.borrow().iter().all(|event| !matches!(
        event,
        RunnerEvent::TextDelta(_) | RunnerEvent::ThinkingDelta(_)
    )));
}

fn assert_rejected(name: &str, response: ScriptedResponse) {
    let dir = temp_dir(name);
    let (port, bodies) = spawn_recording_server(vec![
        sse_tool_call(
            INITIAL_CALL,
            "web_3Asource_2Fread",
            r#"{\"opaque\":\"SYNTH_REJECT_INPUT_90d1\"}"#,
        ),
        response,
    ]);
    let (mut ctx, events) = build_ctx_with_store(port, &dir, tool_table(), None);
    let mut session = Session::new(AgentId::root());
    assert_eq!(
        run_turn(&mut session, &mut ctx, "synthetic rejection")
            .expect("initial source request is not a terminal source failure"),
        TurnStatus::ToolsPending
    );

    let claim = claim_remote_tool(
        &session,
        &mut ctx,
        RemoteToolClaimRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(INITIAL_CALL),
            claim_id: "rejection-worker".into(),
        },
    );
    let RemoteToolClaimDecision::Claimed(grant) = claim else {
        panic!("source claim was not granted: {claim:?}");
    };
    assert_eq!(grant.request.input["opaque"], PRIVATE_INPUT);

    let failure = submit_remote_tool_result(
        &mut session,
        &mut ctx,
        RemoteToolSubmitRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(INITIAL_CALL),
            claim_id: "rejection-worker".into(),
            submission_id: "rejection-submission".into(),
            outcome: RemoteToolSubmitOutcome::Succeeded {
                content: PRIVATE_RESULT.into(),
            },
        },
        |_| {},
    )
    .expect_err("invalid source continuation must escape the core unclassified");
    assert!(matches!(
        failure,
        TransientSourceFailure::InvalidCompletion { agent, .. } if agent == AgentId::root()
    ));
    assert_eq!(bodies.lock().unwrap().len(), 2);

    let durable = serde_json::to_string(&session.primitives()).unwrap();
    let history = format!("{:#?}", session.history());
    let emitted = format!("{:#?}", events.borrow());
    for marker in [
        PRIVATE_INPUT,
        PRIVATE_RESULT,
        PRIVATE_TEXT,
        PRIVATE_THINKING,
    ] {
        assert!(!durable.contains(marker));
        assert!(!history.contains(marker));
        assert!(!emitted.contains(marker));
    }
    assert!(events.borrow().iter().all(|event| !matches!(
        event,
        RunnerEvent::TextDelta(_) | RunnerEvent::ThinkingDelta(_)
    )));
}

#[test]
fn text_mixed_with_a_source_call_is_safely_scrubbed() {
    assert_text_mixed_source_call_is_scrubbed();
}

#[test]
fn declared_non_source_call_fails_closed() {
    assert_rejected(
        "transient-source-non-source-call",
        sse_tool_call(
            "non-source",
            "web_3Aclient_2Finspect",
            r#"{\"opaque\":\"next\"}"#,
        ),
    );
}
