//! 视觉工具终态信封与稳定错误码的单元面。

use serde_json::Value;

use super::*;

fn json(outcome: &VisionToolOutcome) -> Value {
    serde_json::from_str(&outcome.content).unwrap()
}

#[test]
fn success_contains_observation_and_only_non_secret_metadata() {
    let outcome = vision_child_outcome(
        VisionChildTerminal::Succeeded {
            observation: Arc::from("The screen shows error E42."),
            truncated: false,
        },
        2,
    );
    assert!(!outcome.is_error);
    let body = json(&outcome);
    assert_eq!(body["observation"], "The screen shows error E42.");
    assert_eq!(body["metadata"]["images_inspected"], 2);
    assert_eq!(body["metadata"]["truncated"], false);
    assert!(body.get("provider").is_none());
    assert!(body.get("model").is_none());
}

#[test]
fn child_terminal_variants_map_to_stable_failure_envelopes() {
    let cases = [
        (
            VisionChildTerminal::TimedOut,
            VisionFailureCode::VisionTimeout,
            true,
        ),
        (
            VisionChildTerminal::Rejected,
            VisionFailureCode::VisionRejected,
            false,
        ),
        (
            VisionChildTerminal::Failed { retryable: true },
            VisionFailureCode::VisionChildFailed,
            true,
        ),
        (
            VisionChildTerminal::Cancelled,
            VisionFailureCode::VisionCancelled,
            false,
        ),
    ];

    for (terminal, code, retryable) in cases {
        let outcome = vision_child_outcome(terminal, 1);
        assert!(outcome.is_error);
        let body = json(&outcome);
        assert_eq!(body["error"]["code"], serde_json::to_value(code).unwrap());
        assert_eq!(body["error"]["retryable"], retryable);
    }
}

#[test]
fn every_preflight_failure_has_the_contract_retryability() {
    let cases = [
        (VisionFailure::attachment_not_found(), false),
        (VisionFailure::attachment_unavailable(), false),
        (VisionFailure::image_unsupported(), false),
        (VisionFailure::profile_unavailable(), true),
        (VisionFailure::upload_failed(), true),
    ];
    for (failure, retryable) in cases {
        let outcome = VisionToolOutcome::failure(failure);
        assert_eq!(json(&outcome)["error"]["retryable"], retryable);
    }
}

#[test]
fn failure_messages_do_not_accept_provider_details() {
    let outcome = VisionToolOutcome::failure(VisionFailure::child_failed(false));
    let content = &*outcome.content;
    assert!(!content.contains("endpoint"));
    assert!(!content.contains("api_key"));
    assert!(!content.contains("provider body"));
}
