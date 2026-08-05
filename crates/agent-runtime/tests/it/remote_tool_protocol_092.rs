//! 092-A 的黑盒验收：远端工具认领、终态回执与 epoch 闸（只经公开 runtime 接缝）。

use std::cell::RefCell;
use std::time::Duration;

use agent_core::{AgentId, ContentBlock, Session, ToolCallId, TurnStatus};
use agent_runtime::{
    RemoteToolActiveState, RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolFailure,
    RemoteToolSubmitDecision, RemoteToolSubmitOutcome, RemoteToolSubmitRequest,
    RemoteToolTerminalStatus, RunnerCtx, RunnerEvent, ToolTable, claim_remote_tool, run_turn,
    submit_remote_tool_result, sweep_remote_tool_deadlines,
};

use crate::support::{build_ctx_with, spawn_scripted_server, sse_text, sse_tool_call, temp_dir};

fn parked_call(
    call_id: &str,
    timeout: Option<Duration>,
) -> (Session, RunnerCtx, std::rc::Rc<RefCell<Vec<RunnerEvent>>>) {
    let dir = temp_dir("remote-tool-protocol-092");
    let mut responses = vec![sse_tool_call(
        call_id,
        "browser_action",
        r#"{\"action\":\"render_card\"}"#,
    )];
    // A successful submit continues the model once; timeouts do as well.
    responses.push(sse_text("remote result observed"));
    let port = spawn_scripted_server(responses);
    let (ctx, events) = build_ctx_with(port, &dir, ToolTable::standard());
    let mut ctx = match timeout {
        Some(timeout) => ctx.with_remote_tool_timeout(timeout),
        None => ctx,
    };
    let mut session = Session::new(AgentId::root());
    assert_eq!(
        run_turn(&mut session, &mut ctx, "render a card"),
        TurnStatus::ToolsPending,
        "precondition: the declared web tool must occupy a waiting slot"
    );
    (session, ctx, events)
}

fn claim(call_id: &str, claim_id: &str) -> RemoteToolClaimRequest {
    RemoteToolClaimRequest {
        agent: AgentId::root(),
        call_id: ToolCallId::new(call_id),
        claim_id: claim_id.into(),
    }
}

fn submit(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    call_id: &str,
    claim_id: &str,
    submission_id: &str,
    outcome: RemoteToolSubmitOutcome,
) -> (Option<TurnStatus>, RemoteToolSubmitDecision) {
    let acknowledgement = RefCell::new(None);
    let status = submit_remote_tool_result(
        session,
        ctx,
        RemoteToolSubmitRequest {
            agent: AgentId::root(),
            call_id: ToolCallId::new(call_id),
            claim_id: claim_id.into(),
            submission_id: submission_id.into(),
            outcome,
        },
        |decision| *acknowledgement.borrow_mut() = Some(decision),
    );
    (
        status,
        acknowledgement
            .into_inner()
            .expect("actor must explicitly acknowledge"),
    )
}

#[test]
fn claim_is_exclusive_and_the_same_claim_id_is_idempotent() {
    let (session, mut ctx, _) = parked_call("claim-1", None);

    assert!(matches!(
        claim_remote_tool(&session, &mut ctx, claim("claim-1", "executor-a")),
        RemoteToolClaimDecision::Claimed(_)
    ));
    assert!(
        matches!(
            claim_remote_tool(&session, &mut ctx, claim("claim-1", "executor-a")),
            RemoteToolClaimDecision::AlreadyClaimedByYou(_)
        ),
        "a retry after a lost claim response must not make the executor run twice"
    );
    assert!(matches!(
        claim_remote_tool(&session, &mut ctx, claim("claim-1", "executor-b")),
        RemoteToolClaimDecision::ClaimedByOther(_)
    ));
    assert!(matches!(
        ctx.remote_tool_status().active.as_slice(),
        [active] if matches!(&active.state, RemoteToolActiveState::Claimed { claim_id } if claim_id == "executor-a")
    ));
}

#[test]
fn committed_submission_is_acknowledged_once_and_hides_failure_details_from_the_model() {
    let (mut session, mut ctx, events) = parked_call("submit-1", None);
    assert!(matches!(
        claim_remote_tool(&session, &mut ctx, claim("submit-1", "executor-a")),
        RemoteToolClaimDecision::Claimed(_)
    ));
    let failed = RemoteToolSubmitOutcome::Failed {
        error: RemoteToolFailure {
            code: "inventory_shortage".into(),
            message: "库存不足".into(),
            retryable: false,
            details: Some(serde_json::json!({"private_trace":"DO_NOT_PROMPT"})),
        },
    };
    let (status, decision) = submit(
        &mut session,
        &mut ctx,
        "submit-1",
        "executor-a",
        "submission-1",
        failed.clone(),
    );
    assert!(matches!(
        decision,
        RemoteToolSubmitDecision::Committed(ref receipt)
            if receipt.status == RemoteToolTerminalStatus::Failed
    ));
    assert_eq!(status, Some(TurnStatus::Done { truncated: false }));
    assert!(matches!(
        submit(
            &mut session,
            &mut ctx,
            "submit-1",
            "executor-a",
            "submission-1",
            failed.clone()
        )
        .1,
        RemoteToolSubmitDecision::Duplicate(ref receipt)
            if receipt.status == RemoteToolTerminalStatus::Failed
    ));
    assert!(matches!(
        submit(
            &mut session,
            &mut ctx,
            "submit-1",
            "executor-a",
            "submission-1",
            RemoteToolSubmitOutcome::Succeeded {
                content: "different payload".into()
            },
        )
        .1,
        RemoteToolSubmitDecision::Conflict(ref receipt)
            if receipt.status == RemoteToolTerminalStatus::Failed
    ));
    assert!(matches!(
        submit(
            &mut session,
            &mut ctx,
            "submit-1",
            "executor-a",
            "submission-2",
            failed
        )
        .1,
        RemoteToolSubmitDecision::Conflict(ref receipt)
            if receipt.status == RemoteToolTerminalStatus::Failed
    ));
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|event| matches!(event, RunnerEvent::ToolExecuted { .. }))
            .count(),
        1,
        "duplicate/conflict must not advance core or emit a second ToolExecuted"
    );
    let messages = session.messages();
    let prompt_body = messages
        .iter()
        .flat_map(|message| &message.blocks)
        .find_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => Some(content.as_ref()),
            _ => None,
        })
        .expect("the committed outcome must already be in core history before acknowledgement");
    assert!(prompt_body.contains("[remote_tool_failed] code=inventory_shortage retryable=false"));
    assert!(
        !prompt_body.contains("DO_NOT_PROMPT"),
        "host-only failure details leaked into the model prompt"
    );
}

#[test]
fn cancelled_submission_records_a_cancelled_terminal() {
    let (mut session, mut ctx, _) = parked_call("cancel-1", None);
    assert!(matches!(
        claim_remote_tool(&session, &mut ctx, claim("cancel-1", "executor-a")),
        RemoteToolClaimDecision::Claimed(_)
    ));
    assert!(matches!(
        submit(
            &mut session,
            &mut ctx,
            "cancel-1",
            "executor-a",
            "cancel-submission",
            RemoteToolSubmitOutcome::Cancelled {
                reason: "operator cancelled".into()
            },
        )
        .1,
        RemoteToolSubmitDecision::Committed(ref receipt)
            if receipt.status == RemoteToolTerminalStatus::Cancelled
    ));
    assert!(matches!(
        ctx.remote_tool_status().recent_terminal.as_slice(),
        [receipt] if receipt.status == RemoteToolTerminalStatus::Cancelled
    ));
}

#[test]
fn deadlines_distinguish_unclaimed_from_claimed_outcome_unknown() {
    for (call_id, claim_id, expected) in [
        (
            "unclaimed",
            None,
            RemoteToolTerminalStatus::UnclaimedTimeout,
        ),
        (
            "claimed",
            Some("executor-a"),
            RemoteToolTerminalStatus::OutcomeUnknown,
        ),
    ] {
        let (mut session, mut ctx, _) = parked_call(call_id, Some(Duration::from_millis(15)));
        if let Some(claim_id) = claim_id {
            assert!(matches!(
                claim_remote_tool(&session, &mut ctx, claim(call_id, claim_id)),
                RemoteToolClaimDecision::Claimed(_)
            ));
        }
        std::thread::sleep(Duration::from_millis(30));
        assert!(sweep_remote_tool_deadlines(&mut session, &mut ctx).is_some());
        assert!(matches!(
            ctx.remote_tool_status().recent_terminal.as_slice(),
            [receipt] if receipt.status == expected && receipt.submission_id.is_none() && receipt.payload_digest.is_none() && receipt.payload_len.is_none()
        ));
    }
}

#[test]
fn undo_makes_a_late_submission_terminal_without_a_ghost_write() {
    let (mut session, mut ctx, events) = parked_call("undo-late", None);
    assert!(matches!(
        claim_remote_tool(&session, &mut ctx, claim("undo-late", "executor-a")),
        RemoteToolClaimDecision::Claimed(_)
    ));
    let _ = session.undo_turn();
    assert!(matches!(
        submit(
            &mut session,
            &mut ctx,
            "undo-late",
            "executor-a",
            "late",
            RemoteToolSubmitOutcome::Succeeded {
                content: "GHOST_RESULT".into()
            },
        )
        .1,
        RemoteToolSubmitDecision::Terminal(ref receipt)
            if receipt.status == RemoteToolTerminalStatus::Cancelled
    ));
    assert!(!session.messages().iter().flat_map(|message| &message.blocks).any(|block| matches!(block, ContentBlock::ToolResult { content, .. } if content.contains("GHOST_RESULT"))));
    assert!(
        events
            .borrow()
            .iter()
            .all(|event| !matches!(event, RunnerEvent::ToolExecuted { .. }))
    );
}
