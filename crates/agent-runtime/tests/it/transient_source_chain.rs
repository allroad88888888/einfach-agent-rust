//! A consumed source result may drive more private source hops before its terminal candidate.

use std::cell::RefCell;
use std::sync::Arc;

use agent_core::{AgentId, ContentBlock, Reversibility, Session, ToolCallId, ToolSpec, TurnStatus};
use agent_runtime::{
    RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolSubmitDecision,
    RemoteToolSubmitOutcome, RemoteToolSubmitRequest, RunnerCtx, RunnerEvent, ToolTable,
    claim_remote_tool, run_turn, submit_remote_tool_result,
};
use serde_json::{Value, json};

use crate::support::{ScriptedResponse, build_ctx_with_store, spawn_recording_server, temp_dir};

const PULL: &str = "web:source/pull";
const SEARCH: &str = "web:source/search";
const READ: &str = "web:source/read";
const PULL_CALL: &str = "private-pull";
const SEARCH_CALL: &str = "private-search";
const READ_CALL: &str = "private-read";

const PULL_INPUT: &str = "SYNTH_PULL_INPUT_c4f0";
const PULL_RESULT: &str = "SYNTH_PULL_RESULT_8a21";
const SEARCH_INPUT: &str = "SYNTH_SEARCH_INPUT_b709";
const SEARCH_RESULT: &str = "SYNTH_SEARCH_RESULT_f653";
const READ_INPUT: &str = "SYNTH_READ_INPUT_77dd";
const READ_RESULT: &str = "SYNTH_READ_RESULT_2e16";
const PULL_REASONING: &str = "SYNTH_PULL_REASONING_19a7";
const SEARCH_REASONING: &str = "SYNTH_SEARCH_REASONING_414a";
const READ_REASONING: &str = "SYNTH_READ_REASONING_732c";
const FINAL_REASONING: &str = "SYNTH_FINAL_REASONING_eb90";
const FINAL_MARKER: &str = "SYNTH_FINAL_PRIVATE_2a5e";
const PRIVATE_CANDIDATE: &str =
    "核心逻辑位于 src/private/engine.rs:73\nfn internal_engine() {}\nSYNTH_FINAL_PRIVATE_2a5e";
const SAFE_CANDIDATE: &str = "[transient_source_candidate_redacted]";

fn source_response(
    call_id: &str,
    wire_name: &str,
    input: &str,
    thinking: &str,
) -> ScriptedResponse {
    let delta = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": Value::Null,
                "reasoning_content": thinking,
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": wire_name,
                        "arguments": json!({"opaque": input}).to_string()
                    }
                }]
            }
        }]
    });
    scripted(delta, "tool_calls")
}

fn final_response() -> ScriptedResponse {
    let delta = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "reasoning_content": FINAL_REASONING,
                "content": PRIVATE_CANDIDATE
            }
        }]
    });
    scripted(delta, "stop")
}

fn public_response() -> ScriptedResponse {
    let delta = json!({
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant", "content": "fresh public answer"}
        }]
    });
    scripted(delta, "stop")
}

fn scripted(delta: Value, finish: &str) -> ScriptedResponse {
    let terminal = json!({
        "choices": [{
            "index": 0,
            "delta": {"content": ""},
            "finish_reason": finish
        }],
        "usage": {
            "prompt_tokens": 31,
            "completion_tokens": 17,
            "prompt_cache_hit_tokens": 0,
            "prompt_cache_miss_tokens": 31
        }
    });
    ScriptedResponse::Sse(vec![
        Box::leak(format!("data: {delta}").into_boxed_str()),
        Box::leak(format!("data: {terminal}").into_boxed_str()),
        "data: [DONE]",
    ])
}

fn reasoning_values(body: &str) -> Vec<String> {
    let request: Value = serde_json::from_str(body).unwrap();
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["reasoning_content"].as_str().map(str::to_owned))
        .collect()
}

fn source_tools() -> ToolTable {
    let tools = [PULL, SEARCH, READ].into_iter().map(|name| {
        (
            ToolSpec {
                name: Arc::from(name),
                description: Arc::from("synthetic transient source"),
                schema: Arc::new(json!({"type":"object"})),
            },
            Reversibility::Pure,
        )
    });
    ToolTable::empty().with_host_tools(tools.collect())
}

fn submit(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    call_id: &str,
    expected_input: &str,
    result: &str,
) -> TurnStatus {
    let claim_id = format!("claim-{call_id}");
    let claim = claim_remote_tool(
        session,
        ctx,
        RemoteToolClaimRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(call_id),
            claim_id: claim_id.clone(),
        },
    );
    let RemoteToolClaimDecision::Claimed(grant) = claim else {
        panic!("source claim was not granted: {claim:?}");
    };
    assert_eq!(grant.request.input["opaque"], expected_input);

    let ack = RefCell::new(None);
    let status = submit_remote_tool_result(
        session,
        ctx,
        RemoteToolSubmitRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(call_id),
            claim_id,
            submission_id: format!("submission-{call_id}"),
            outcome: RemoteToolSubmitOutcome::Succeeded {
                content: result.into(),
            },
        },
        |decision| *ack.borrow_mut() = Some(decision),
    )
    .expect("new source result must resume the turn");
    assert!(matches!(
        ack.into_inner(),
        Some(RemoteToolSubmitDecision::Committed(_))
    ));
    status.expect("successful source result must finish the turn")
}

fn assert_private_surfaces(session: &Session, events: &[RunnerEvent]) {
    let private = [
        PULL_INPUT,
        PULL_RESULT,
        SEARCH_INPUT,
        SEARCH_RESULT,
        READ_INPUT,
        READ_RESULT,
        SEARCH_REASONING,
        READ_REASONING,
        FINAL_REASONING,
    ];
    let durable = serde_json::to_string(&session.primitives()).unwrap();
    let history = format!("{:#?}", session.history());
    let emitted = format!("{events:#?}");
    for marker in private {
        assert!(!durable.contains(marker));
        assert!(!history.contains(marker));
        assert!(!emitted.contains(marker));
    }
    assert!(emitted.contains(PULL_REASONING));
    assert!(events.iter().any(
        |event| matches!(event, RunnerEvent::TextDelta(text) if &**text == PRIVATE_CANDIDATE)
    ));
    assert!(!durable.contains(PULL_REASONING));
    assert!(!history.contains(PULL_REASONING));
    assert!(!durable.contains(FINAL_MARKER));
    assert!(!history.contains(FINAL_MARKER));
}

#[test]
fn pull_search_read_then_terminal_candidate_stays_private() {
    let dir = temp_dir("transient-source-chain");
    let (port, bodies) = spawn_recording_server(vec![
        source_response(PULL_CALL, "web_3Asource_2Fpull", PULL_INPUT, PULL_REASONING),
        source_response(
            SEARCH_CALL,
            "web_3Asource_2Fsearch",
            SEARCH_INPUT,
            SEARCH_REASONING,
        ),
        source_response(READ_CALL, "web_3Asource_2Fread", READ_INPUT, READ_REASONING),
        final_response(),
        public_response(),
    ]);
    let (mut ctx, events) = build_ctx_with_store(port, &dir, source_tools(), None);
    let mut session = Session::new(AgentId::root());

    assert_eq!(
        run_turn(&mut session, &mut ctx, "synthetic diagnosis")
            .expect("initial source request is not a terminal source failure"),
        TurnStatus::ToolsPending
    );
    assert_eq!(
        submit(&mut session, &mut ctx, PULL_CALL, PULL_INPUT, PULL_RESULT),
        TurnStatus::ToolsPending
    );
    assert_eq!(
        submit(
            &mut session,
            &mut ctx,
            SEARCH_CALL,
            SEARCH_INPUT,
            SEARCH_RESULT,
        ),
        TurnStatus::ToolsPending
    );
    assert_eq!(
        submit(&mut session, &mut ctx, READ_CALL, READ_INPUT, READ_RESULT),
        TurnStatus::Done { truncated: false }
    );

    assert!(session.messages().iter().any(|message| {
        matches!(message.blocks.as_slice(), [ContentBlock::Text(text)] if &**text == SAFE_CANDIDATE)
    }));
    assert_private_surfaces(&session, &events.borrow());

    session.begin_turn();
    assert_eq!(
        run_turn(&mut session, &mut ctx, "fresh public turn")
            .expect("fresh public turn is not a source failure"),
        TurnStatus::Done { truncated: false }
    );
    assert_private_surfaces(&session, &events.borrow());

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 5);
    assert!(reasoning_values(&bodies[0]).is_empty());
    assert!(!bodies[0].contains(PULL_INPUT));
    assert!(bodies[1].contains(PULL_INPUT) && bodies[1].contains(PULL_RESULT));
    assert_eq!(reasoning_values(&bodies[1]), [PULL_REASONING]);
    assert!(!bodies[2].contains(PULL_INPUT) && !bodies[2].contains(PULL_RESULT));
    assert!(bodies[2].contains(SEARCH_INPUT) && bodies[2].contains(SEARCH_RESULT));
    assert_eq!(
        reasoning_values(&bodies[2]),
        [PULL_REASONING, SEARCH_REASONING]
    );
    for marker in [PULL_INPUT, PULL_RESULT, SEARCH_INPUT, SEARCH_RESULT] {
        assert!(!bodies[3].contains(marker));
    }
    assert!(bodies[3].contains(READ_INPUT) && bodies[3].contains(READ_RESULT));
    assert_eq!(
        reasoning_values(&bodies[3]),
        [PULL_REASONING, SEARCH_REASONING, READ_REASONING]
    );
    assert!(reasoning_values(&bodies[4]).is_empty());
    for marker in [PULL_REASONING, SEARCH_REASONING, READ_REASONING] {
        assert!(!bodies[4].contains(marker));
    }
    assert!(!bodies[4].contains(FINAL_MARKER));
}
