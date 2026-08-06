//! Source-tool capability alone must not disable ordinary provider streaming.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_core::{AgentId, Reversibility, Session, ToolSpec, TurnStatus};
use agent_runtime::{RunnerEvent, ToolTable, run_turn};
use serde_json::json;

use crate::support::{
    ScriptedResponse, build_ctx_with_observer, spawn_scripted_server, sse_text, temp_dir,
};

const TEXT: &str = "ordinary live text";

#[test]
fn ordinary_delta_arrives_before_done_when_source_tools_are_available() {
    let terminal_started = Arc::new(AtomicBool::new(false));
    let saw_early = Arc::new(AtomicBool::new(false));
    let ScriptedResponse::Sse(mut before) = sse_text(TEXT) else {
        panic!("expected SSE response");
    };
    let after = before.split_off(1);
    let port = spawn_scripted_server(vec![ScriptedResponse::SseSplit {
        before,
        after,
        after_started: Arc::clone(&terminal_started),
    }]);
    let tools = ToolTable::empty().with_host_tools(vec![(
        ToolSpec {
            name: Arc::from("web:source/read"),
            description: Arc::from("transient source read"),
            schema: Arc::new(json!({"type":"object"})),
        },
        Reversibility::Pure,
    )]);
    let terminal_for_observer = Arc::clone(&terminal_started);
    let early_for_observer = Arc::clone(&saw_early);
    let observer = Box::new(move |event: &RunnerEvent| {
        if matches!(event, RunnerEvent::TextDelta(text) if &**text == TEXT)
            && !terminal_for_observer.load(Ordering::Acquire)
        {
            early_for_observer.store(true, Ordering::Release);
        }
    });
    let dir = temp_dir("transient-source-ordinary-stream");
    let (mut ctx, _) = build_ctx_with_observer(port, &dir, tools, None, Some(observer));
    let mut session = Session::new(AgentId::root());

    assert_eq!(
        run_turn(&mut session, &mut ctx, "hello").expect("ordinary stream is not a source failure"),
        TurnStatus::Done { truncated: false }
    );
    assert!(terminal_started.load(Ordering::Acquire));
    assert!(saw_early.load(Ordering::Acquire));
}
