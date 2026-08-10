use std::sync::Arc;

use agent_core::{AgentId, ContentBlock, Epoch, Message, MessageId, Role, ToolCallId};
use serde_json::json;

use crate::remote_tool_protocol::{RemoteToolFailure, RemoteToolSubmitOutcome};
use crate::transient_source_policy::{
    SAFE_ERROR, SAFE_RESULT, is_placeholder_input, is_transient_source, placeholder_input,
};
use crate::transient_source_prompt::prepare;
use crate::transient_source_vault::{CapturedSource, TransientSourceVault};

const SOURCE_PULL: &str = "web:source/pull";
const SOURCE_SEARCH: &str = "web:source/search";
const SOURCE_READ: &str = "web:source/read";

fn captured(id: &str, tool: &str, secret: serde_json::Value) -> CapturedSource {
    captured_with_reasoning(id, tool, secret, None)
}

fn captured_with_reasoning(
    id: &str,
    tool: &str,
    secret: serde_json::Value,
    reasoning: Option<&str>,
) -> CapturedSource {
    CapturedSource {
        agent: AgentId::root(),
        epoch: Epoch::START,
        call_id: ToolCallId::new(id),
        tool: Arc::from(tool),
        input: Arc::new(secret),
        reasoning: reasoning.map(Arc::from),
    }
}

fn source_use(id: &str, tool: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: ToolCallId::new(id),
        name: Arc::from(tool),
        input: placeholder_input(),
    }
}

fn tool_result(id: &str, content: &str, is_error: bool) -> ContentBlock {
    ContentBlock::ToolResult {
        id: ToolCallId::new(id),
        content: Arc::from(content),
        is_error,
    }
}

#[test]
fn source_tool_namespace_is_transient() {
    for name in [
        SOURCE_PULL,
        SOURCE_SEARCH,
        SOURCE_READ,
        "web:source/log",
        "web:source/tags",
        "web:source/diff",
        "web:source/future-operation",
    ] {
        assert!(is_transient_source(name));
    }
    for name in [
        "web:source",
        "web:source/",
        "web_source_read",
        "WEB:SOURCE/READ",
        "web:diagnostic/read",
    ] {
        assert!(!is_transient_source(name));
    }
}

#[test]
fn vault_capture_is_atomic_and_outcomes_are_canonical() {
    let mut vault = TransientSourceVault::default();
    vault
        .capture_batch(vec![captured("existing", SOURCE_PULL, json!({"a": 1}))])
        .unwrap();

    assert!(
        vault
            .capture_batch(vec![
                captured("new", SOURCE_SEARCH, json!({"b": 2})),
                captured("new", SOURCE_READ, json!({"c": 3})),
            ])
            .is_err()
    );
    assert!(
        vault
            .raw_input(
                &AgentId::root(),
                Epoch::START,
                &ToolCallId::new("new"),
                SOURCE_SEARCH,
            )
            .is_none()
    );

    let outcome = RemoteToolSubmitOutcome::Failed {
        error: RemoteToolFailure {
            code: "private-code".into(),
            message: "private-message".into(),
            retryable: true,
            details: Some(json!({"z": 1, "a": {"y": 2, "b": 3}})),
        },
    };
    vault
        .record_outcome(
            &AgentId::root(),
            Epoch::START,
            &ToolCallId::new("existing"),
            &outcome,
        )
        .unwrap();
    let expected = [ToolCallId::new("existing")];
    let ready = vault
        .take_ready_hop(&AgentId::root(), Epoch::START, &expected)
        .unwrap()
        .unwrap();
    assert_eq!(ready.current.len(), 1);
    assert_eq!(ready.reasoning.len(), 1);
    assert!(ready.current[0].is_error);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ready.current[0].outcome).unwrap(),
        json!({
            "status": "failed",
            "error": {
                "code": "private-code",
                "message": "private-message",
                "retryable": true,
                "details": {"a": {"b": 3, "y": 2}, "z": 1},
            }
        })
    );
    assert!(
        vault
            .take_ready_hop(&AgentId::root(), Epoch::START, &expected)
            .is_err()
    );
}

#[test]
fn prompt_overlay_is_one_shot_and_preserves_normal_tools() {
    let source_secret = json!({"query": "raw-request-canary"});
    let normal_input = Arc::new(json!({"path": "visible"}));
    let durable = vec![
        Message {
            id: MessageId(1),
            role: Role::Assistant,
            blocks: vec![
                ContentBlock::Text(Arc::from("before")),
                source_use("source", SOURCE_SEARCH),
                ContentBlock::ToolUse {
                    id: ToolCallId::new("normal"),
                    name: Arc::from("fs/read"),
                    input: Arc::clone(&normal_input),
                },
            ],
        },
        Message {
            id: MessageId(2),
            role: Role::Assistant,
            blocks: vec![
                tool_result("source", SAFE_RESULT, false),
                tool_result("normal", "visible-result", false),
            ],
        },
    ];
    let mut vault = TransientSourceVault::default();
    vault
        .capture_batch(vec![captured_with_reasoning(
            "source",
            SOURCE_SEARCH,
            source_secret.clone(),
            Some("provider-native-reasoning"),
        )])
        .unwrap();
    vault
        .record_outcome(
            &AgentId::root(),
            Epoch::START,
            &ToolCallId::new("source"),
            &RemoteToolSubmitOutcome::Succeeded {
                content: "raw-result-canary".into(),
            },
        )
        .unwrap();

    let prepared = prepare(&durable, &mut vault, &AgentId::root(), Epoch::START).unwrap();
    assert!(prepared.one_shot);
    assert_eq!(prepared.messages.len(), durable.len());
    assert!(matches!(
        &prepared.messages[0].blocks[0],
        ContentBlock::Thinking(reasoning) if &**reasoning == "provider-native-reasoning"
    ));
    match &prepared.messages[0].blocks[2] {
        ContentBlock::ToolUse { input, .. } => assert_eq!(input.as_ref(), &source_secret),
        other => panic!("unexpected source block: {other:?}"),
    }
    match &prepared.messages[0].blocks[3] {
        ContentBlock::ToolUse { input, .. } => assert_eq!(input, &normal_input),
        other => panic!("unexpected normal block: {other:?}"),
    }
    assert!(matches!(
        &prepared.messages[1].blocks[0],
        ContentBlock::ToolResult { content, is_error: false, .. }
            if &**content == "raw-result-canary"
    ));
    assert_eq!(durable[0].blocks[1], source_use("source", SOURCE_SEARCH));
    assert!(durable.iter().all(|message| {
        message
            .blocks
            .iter()
            .all(|block| !matches!(block, ContentBlock::Thinking(_)))
    }));
    assert_eq!(
        durable[1].blocks[0],
        tool_result("source", SAFE_RESULT, false)
    );

    assert!(prepare(&durable, &mut vault, &AgentId::root(), Epoch::START).is_err());
}

#[test]
fn prompt_fails_closed_when_marker_and_vault_do_not_pair() {
    let messages = vec![
        Message {
            id: MessageId(1),
            role: Role::Assistant,
            blocks: vec![source_use("source", SOURCE_READ)],
        },
        Message {
            id: MessageId(2),
            role: Role::Assistant,
            blocks: vec![tool_result("source", SAFE_ERROR, true)],
        },
    ];
    let mut empty = TransientSourceVault::default();
    assert!(prepare(&messages, &mut empty, &AgentId::root(), Epoch::START).is_err());

    let mut unmatched = TransientSourceVault::default();
    unmatched
        .capture_batch(vec![captured("other", SOURCE_PULL, json!({"raw": true}))])
        .unwrap();
    unmatched
        .record_outcome(
            &AgentId::root(),
            Epoch::START,
            &ToolCallId::new("other"),
            &RemoteToolSubmitOutcome::Cancelled {
                reason: "private-reason".into(),
            },
        )
        .unwrap();
    assert!(prepare(&messages, &mut unmatched, &AgentId::root(), Epoch::START).is_err());

    assert!(is_placeholder_input(match &messages[0].blocks[0] {
        ContentBlock::ToolUse { input, .. } => input,
        _ => unreachable!(),
    }));
}
