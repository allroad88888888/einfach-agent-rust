use std::sync::Arc;

use agent_core::{
    AgentId, ChildConfig, ContentBlock, Epoch, ErrorClass, Event, Failure, PrefixImage, Session,
    StopReason, TokenUsage, TurnStatus,
};
use serde_json::Value;

use super::*;

fn child_in_thinking(session: &mut Session) -> AgentId {
    let root = AgentId::root();
    let child = session.spawn_child(&root, ChildConfig::default()).unwrap();
    let _ = session.step(Event::UserInput {
        agent: child.clone(),
        text: Arc::from("isolated visual question"),
        images: Vec::new(),
    });
    child
}

fn parsed(result: (String, bool)) -> (Value, bool) {
    (serde_json::from_str(&result.0).unwrap(), result.1)
}

#[test]
fn successful_child_returns_only_observation_and_stable_metadata() {
    let mut session = Session::new(AgentId::root());
    let child = child_in_thinking(&mut session);
    let _ = session.step(Event::ProviderDone {
        agent: child.clone(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::Text(Arc::from("There are two red circles."))],
        stop: StopReason::EndTurn,
        usage: TokenUsage {
            prompt: 8,
            completion: 5,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    });

    let (body, is_error) = parsed(outcome(
        &session,
        &child,
        &session.status_of(&child),
        2,
        false,
    ));
    assert!(!is_error);
    assert_eq!(body["observation"], "There are two red circles.");
    assert_eq!(body["metadata"]["images_inspected"], 2);
    assert_eq!(body["metadata"]["truncated"], false);
    assert_eq!(body.as_object().unwrap().len(), 2);
}

#[test]
fn rejected_failed_timed_out_and_cancelled_children_have_stable_safe_codes() {
    let mut session = Session::new(AgentId::root());
    let child = child_in_thinking(&mut session);
    let _ = session.step(Event::Cancel {
        agent: child.clone(),
    });

    let cases = [
        (
            TurnStatus::Failed(Failure::Provider(ErrorClass::BadRequest)),
            false,
            "vision_rejected",
            false,
        ),
        (
            TurnStatus::Failed(Failure::Provider(ErrorClass::Unknown)),
            false,
            "vision_child_failed",
            false,
        ),
        (
            TurnStatus::Failed(Failure::Provider(ErrorClass::Retryable)),
            false,
            "vision_child_failed",
            true,
        ),
        (
            TurnStatus::Failed(Failure::Provider(ErrorClass::Retryable)),
            true,
            "vision_timeout",
            true,
        ),
        (session.status_of(&child), false, "vision_cancelled", false),
    ];
    for (status, timed_out, code, retryable) in cases {
        let (body, is_error) = parsed(outcome(&session, &child, &status, 1, timed_out));
        assert!(is_error);
        assert_eq!(body["error"]["code"], code);
        assert_eq!(body["error"]["retryable"], retryable);
        assert_eq!(body["error"].as_object().unwrap().len(), 3);
        assert!(!body.to_string().contains("provider-secret-body"));
    }
}

#[test]
fn real_provider_rejection_does_not_leak_its_message() {
    let mut session = Session::new(AgentId::root());
    let child = child_in_thinking(&mut session);
    let _ = session.step(Event::ProviderFailed {
        agent: child.clone(),
        epoch: Epoch::START,
        class: ErrorClass::Auth,
        message: Arc::from("provider-secret-body"),
    });

    let (body, is_error) = parsed(outcome(
        &session,
        &child,
        &session.status_of(&child),
        1,
        false,
    ));
    assert!(is_error);
    assert_eq!(body["error"]["code"], "vision_rejected");
    assert!(!body.to_string().contains("provider-secret-body"));
}

#[test]
fn done_without_non_empty_assistant_text_is_not_reported_as_success() {
    let mut session = Session::new(AgentId::root());
    let child = child_in_thinking(&mut session);
    let _ = session.step(Event::ProviderDone {
        agent: child.clone(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::Text(Arc::from("  \n "))],
        stop: StopReason::EndTurn,
        usage: TokenUsage {
            prompt: 1,
            completion: 0,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    });

    let (body, is_error) = parsed(outcome(
        &session,
        &child,
        &session.status_of(&child),
        1,
        false,
    ));
    assert!(is_error);
    assert_eq!(body["error"]["code"], "vision_child_failed");
    assert_eq!(body["error"]["retryable"], false);
    assert!(body.get("observation").is_none());
}
