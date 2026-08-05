//! 092-A 的终态回执留存边界黑盒验收。

use std::cell::RefCell;

use agent_core::{AgentId, Session, ToolCallId, TurnStatus};
use agent_runtime::{
    REMOTE_TOOL_RECEIPT_CAP, RemoteToolClaimDecision, RemoteToolClaimRequest,
    RemoteToolSubmitDecision, RemoteToolSubmitOutcome, RemoteToolSubmitRequest,
    RemoteToolTerminalStatus, ToolTable, claim_remote_tool, run_turn, submit_remote_tool_result,
};

use crate::support::{build_ctx_with, spawn_scripted_server, sse_text, sse_tool_call, temp_dir};

fn claim(call_id: &str) -> RemoteToolClaimRequest {
    RemoteToolClaimRequest {
        agent: AgentId::root(),
        call_id: ToolCallId::new(call_id),
        claim_id: "executor-a".into(),
    }
}

fn submit(
    session: &mut Session,
    ctx: &mut agent_runtime::RunnerCtx,
    call_id: &str,
    content: String,
) -> RemoteToolSubmitDecision {
    let acknowledgement = RefCell::new(None);
    submit_remote_tool_result(
        session,
        ctx,
        RemoteToolSubmitRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(call_id),
            claim_id: "executor-a".into(),
            submission_id: format!("submission-{call_id}"),
            outcome: RemoteToolSubmitOutcome::Succeeded { content },
        },
        |decision| *acknowledgement.borrow_mut() = Some(decision),
    );
    acknowledgement
        .into_inner()
        .expect("actor must acknowledge")
}

#[test]
fn terminal_ledger_is_capped_and_eviction_is_honest() {
    let mut responses = Vec::new();
    for index in 0..=REMOTE_TOOL_RECEIPT_CAP {
        responses.push(sse_tool_call(
            &format!("cap-{index}"),
            "browser_action",
            r#"{}"#,
        ));
        responses.push(sse_text("done"));
    }
    let dir = temp_dir("remote-tool-receipt-cap");
    let port = spawn_scripted_server(responses);
    let (mut ctx, _) = build_ctx_with(port, &dir, ToolTable::standard());
    let payload = "x".repeat(512);
    let mut latest_registered_at = None;
    for index in 0..=REMOTE_TOOL_RECEIPT_CAP {
        let call_id = format!("cap-{index}");
        let mut session = Session::new(AgentId::root());
        assert_eq!(
            run_turn(&mut session, &mut ctx, "render"),
            TurnStatus::ToolsPending
        );
        latest_registered_at = Some(ctx.remote_tool_status().active[0].registered_at);
        assert!(matches!(
            claim_remote_tool(&session, &mut ctx, claim(&call_id)),
            RemoteToolClaimDecision::Claimed(_)
        ));
        assert!(matches!(
            submit(&mut session, &mut ctx, &call_id, payload.clone()),
            RemoteToolSubmitDecision::Committed(ref receipt)
                if receipt.status == RemoteToolTerminalStatus::Succeeded
        ));
    }
    let snapshot = ctx.remote_tool_status();
    assert_eq!(snapshot.recent_terminal.len(), REMOTE_TOOL_RECEIPT_CAP);
    assert!(
        snapshot
            .recent_terminal
            .iter()
            .all(|receipt| receipt.payload_digest.is_some())
    );
    assert!(
        snapshot
            .recent_terminal
            .iter()
            .all(|receipt| receipt.payload_len == Some(payload.len() + 9))
    );
    assert_eq!(
        snapshot
            .recent_terminal
            .last()
            .expect("latest receipt")
            .created_at,
        latest_registered_at.expect("latest registration time")
    );
    assert!(snapshot.retention_floor_revision.is_some());
    let mut evicted_session = Session::new(AgentId::root());
    assert_eq!(
        claim_remote_tool(&evicted_session, &mut ctx, claim("cap-0")),
        RemoteToolClaimDecision::StatusNotRetained
    );
    assert_eq!(
        submit(&mut evicted_session, &mut ctx, "cap-0", payload),
        RemoteToolSubmitDecision::StatusNotRetained
    );
}
