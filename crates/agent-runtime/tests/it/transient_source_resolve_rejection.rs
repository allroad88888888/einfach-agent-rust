//! 124 native coverage: `resolve_remote_tool` (the simple, no-claim-needed door)
//! must refuse to complete a transient-source (`web:source/*`) pending call —
//! only `claim_remote_tool` + `submit_remote_tool_result` may, since that is the
//! only path that redacts real input/output before it durably lands. A rejected
//! `resolve_remote_tool` call must also leave the pending slot untouched, not
//! silently consume it.
//!
//! Reverse lock: an ordinary `web:`-prefixed but non-`web:source/`-prefixed call
//! is not swept into the same policy — `resolve_remote_tool` is the right door
//! for it, and its real input/output land in history verbatim.

use std::cell::RefCell;
use std::sync::Arc;

use agent_core::{AgentId, Reversibility, Session, ToolCallId, ToolSpec, TurnStatus};
use agent_runtime::{
    RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolOutput, RemoteToolSubmitDecision,
    RemoteToolSubmitOutcome, RemoteToolSubmitRequest, ResolveRemoteToolError, ToolTable,
    claim_remote_tool, resolve_remote_tool, run_turn, submit_remote_tool_result,
};
use serde_json::json;

use crate::support::{
    build_ctx_with_store, spawn_recording_server, sse_text, sse_tool_call, temp_dir,
};

const SOURCE_TOOL: &str = "web:source/peek";
const SOURCE_WIRE: &str = "web_3Asource_2Fpeek";
const ORDINARY_TOOL: &str = "web:page/title";
const ORDINARY_WIRE: &str = "web_3Apage_2Ftitle";

const SOURCE_CALL: &str = "resolve-rejection-source";
const ORDINARY_CALL: &str = "resolve-ordinary-call";

const SOURCE_RESULT: &str = "SYNTH_RESOLVE_SOURCE_RESULT_9c04";
const ORDINARY_INPUT: &str = "SYNTH_RESOLVE_ORDINARY_INPUT_44de";
const ORDINARY_RESULT: &str = "SYNTH_RESOLVE_ORDINARY_RESULT_71bb";

fn tool_table() -> ToolTable {
    let specs = [SOURCE_TOOL, ORDINARY_TOOL].into_iter().map(|name| {
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

#[test]
fn resolve_remote_tool_rejects_a_transient_source_call_and_leaves_it_claimable() {
    let dir = temp_dir("transient-source-resolve-rejection");
    let (port, bodies) = spawn_recording_server(vec![
        sse_tool_call(SOURCE_CALL, SOURCE_WIRE, r#"{\"opaque\":\"probe\"}"#),
        sse_text("acknowledged"),
    ]);
    let (mut ctx, _events) = build_ctx_with_store(port, &dir, tool_table(), None);
    let mut session = Session::new(AgentId::root());

    assert_eq!(
        run_turn(&mut session, &mut ctx, "peek a private source")
            .expect("initial source request is not a terminal source failure"),
        TurnStatus::ToolsPending
    );
    assert_eq!(ctx.pending_remote_tool_count(), 1);

    // (b) `resolve_remote_tool` is the wrong door for a `web:source/*` call.
    let rejected = resolve_remote_tool(
        &mut session,
        &mut ctx,
        AgentId::root(),
        ToolCallId::new(SOURCE_CALL),
        RemoteToolOutput::Success(SOURCE_RESULT.into()),
    );
    assert!(
        matches!(rejected, Err(ResolveRemoteToolError::InvalidResult(_))),
        "resolve_remote_tool must reject a web:source/* call, got {rejected:?}"
    );

    // (c) the pending slot is untouched by the rejection -- still exactly one,
    // and still claimable through the correct door.
    assert_eq!(
        ctx.pending_remote_tool_count(),
        1,
        "a rejected resolve_remote_tool must not consume the pending slot"
    );
    let pending = ctx.pending_remote_tools();
    assert_eq!(pending.len(), 1);
    assert_eq!(&*pending[0].call_id.0, SOURCE_CALL);

    let claim = claim_remote_tool(
        &session,
        &mut ctx,
        RemoteToolClaimRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(SOURCE_CALL),
            claim_id: "resolve-rejection-worker".into(),
        },
    );
    let RemoteToolClaimDecision::Claimed(grant) = claim else {
        panic!("the still-pending source call must remain claimable after rejection: {claim:?}");
    };
    assert_eq!(grant.request.input["opaque"], "probe");

    let acknowledgement = RefCell::new(None);
    let status = submit_remote_tool_result(
        &mut session,
        &mut ctx,
        RemoteToolSubmitRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(SOURCE_CALL),
            claim_id: "resolve-rejection-worker".into(),
            submission_id: "resolve-rejection-submission".into(),
            outcome: RemoteToolSubmitOutcome::Succeeded {
                content: SOURCE_RESULT.into(),
            },
        },
        |decision| *acknowledgement.borrow_mut() = Some(decision),
    )
    .expect("claim_remote_tool + submit_remote_tool_result is the correct door for a source call");
    assert_eq!(status, Some(TurnStatus::Done { truncated: false }));
    assert!(matches!(
        acknowledgement.into_inner(),
        Some(RemoteToolSubmitDecision::Committed(_))
    ));
    assert_eq!(bodies.lock().unwrap().len(), 2);
}

#[test]
fn resolve_remote_tool_completes_an_ordinary_web_tool_with_real_content_in_history() {
    let dir = temp_dir("transient-source-resolve-ordinary");
    let (port, bodies) = spawn_recording_server(vec![
        sse_tool_call(
            ORDINARY_CALL,
            ORDINARY_WIRE,
            r#"{\"opaque\":\"SYNTH_RESOLVE_ORDINARY_INPUT_44de\"}"#,
        ),
        sse_text("looked it up"),
    ]);
    let (mut ctx, _events) = build_ctx_with_store(port, &dir, tool_table(), None);
    let mut session = Session::new(AgentId::root());

    assert_eq!(
        run_turn(&mut session, &mut ctx, "read an ordinary web tool")
            .expect("initial ordinary request is not a terminal source failure"),
        TurnStatus::ToolsPending
    );

    let status = resolve_remote_tool(
        &mut session,
        &mut ctx,
        AgentId::root(),
        ToolCallId::new(ORDINARY_CALL),
        RemoteToolOutput::Success(ORDINARY_RESULT.into()),
    )
    .expect("resolve_remote_tool is the correct door for an ordinary web: tool");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let history = format!("{:#?}", session.history());
    assert!(
        history.contains(ORDINARY_INPUT),
        "an ordinary web: tool's real input must land in history, not be redacted: {history}"
    );
    assert!(
        history.contains(ORDINARY_RESULT),
        "an ordinary web: tool's real result must land in history, not be redacted: {history}"
    );
    assert!(
        !history.contains("transient_source"),
        "an ordinary web: tool must never be treated as transient-source: {history}"
    );
    assert_eq!(bodies.lock().unwrap().len(), 2);
}
