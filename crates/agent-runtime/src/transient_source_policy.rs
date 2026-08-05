//! Exact-name policy and durable placeholders for transient source tools.

use std::sync::Arc;

use agent_core::ToolCallRequest;
use serde_json::{Value, json};

pub(crate) const SOURCE_PULL: &str = "web:source/pull";
pub(crate) const SOURCE_SEARCH: &str = "web:source/search";
pub(crate) const SOURCE_READ: &str = "web:source/read";

pub(crate) const SAFE_RESULT: &str = "[transient_source_result_redacted]";
pub(crate) const SAFE_ERROR: &str = "[transient_source_error_redacted]";
pub(crate) const SAFE_CANDIDATE: &str = "[transient_source_candidate_redacted]";
pub(crate) const SAFE_PROVIDER_ERROR: &str = "transient source provider call failed";
pub(crate) const SAFE_INGRESS_ERROR: &str = "invalid transient source tool batch";

pub(crate) fn is_transient_source(name: &str) -> bool {
    matches!(name, SOURCE_PULL | SOURCE_SEARCH | SOURCE_READ)
}

pub(crate) fn placeholder_input() -> Arc<Value> {
    Arc::new(json!({"transient_source": "redacted"}))
}

pub(crate) fn is_placeholder_input(input: &Value) -> bool {
    input == placeholder_input().as_ref()
}

pub(crate) fn sanitize_request(request: &ToolCallRequest) -> ToolCallRequest {
    ToolCallRequest {
        tool: Arc::clone(&request.tool),
        input: placeholder_input(),
        location: request.location,
        reversibility: request.reversibility,
    }
}
