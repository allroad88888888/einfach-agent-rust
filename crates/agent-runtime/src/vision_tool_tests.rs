use std::collections::BTreeMap;
use std::sync::Arc;

use agent_core::vision::{VISION_INSPECT_TOOL, vision_inspect_spec};
use agent_core::{
    AgentId, ChildConfig, Effect, Epoch, ErrorClass, Event, ExecutionProfileId, Failure,
    Reversibility, Session, SessionConfig, ToolCallId,
};
use agent_providers::deepseek::DeepSeek;
use agent_providers::kimi::Kimi;
use agent_tools::ToolExecutor;
use agent_transport::Client;
use serde_json::{Value, json};

use super::*;
use crate::dispatch;
use crate::execution_binding::ExecutionBinding;
use crate::subagent;
use crate::tool_table::ToolTable;

fn config(model: &str) -> SessionConfig {
    SessionConfig {
        model: Arc::from(model),
        temperature: None,
        max_tokens: None,
        context_window: None,
    }
}

fn context(with_vision: bool) -> RunnerCtx {
    // Deliberately inject the reserved name as a host tool. `subagent::tools_for` must
    // still hide it unless the trusted runtime binding exists, then use the canonical spec.
    let tools =
        ToolTable::builtin().with_host_tools(vec![(vision_inspect_spec(), Reversibility::Pure)]);
    let ctx = RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "https://api.deepseek.com/chat/completions".to_string(),
        "default-key".to_string(),
        ToolExecutor::new(std::env::temp_dir()).unwrap(),
        tools,
        Vec::new(),
        config("deepseek-test"),
        crate::persist::open_backend(None, |_| {}),
        Box::new(|_| {}),
    );
    if !with_vision {
        return ctx;
    }
    ctx.with_execution_bindings(BTreeMap::from([(
        ExecutionProfileId::new("vision"),
        ExecutionBinding::new(
            Arc::new(Kimi),
            Arc::new(Client::new()),
            "https://api.moonshot.cn/v1/chat/completions".to_string(),
            "vision-key".to_string(),
            config("kimi-vision-test"),
        ),
    )]))
}

fn vision_count(specs: &[agent_core::ToolSpec]) -> usize {
    specs
        .iter()
        .filter(|spec| &*spec.name == VISION_INSPECT_TOOL)
        .count()
}

#[test]
fn facade_is_absent_without_the_trusted_profile_and_root_only_with_it() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    assert_eq!(
        vision_count(&subagent::tools_for(&session, &context(false), &root)),
        0
    );

    let ctx = context(true);
    let root_specs = subagent::tools_for(&session, &ctx, &root);
    assert_eq!(vision_count(&root_specs), 1);
    assert_eq!(
        root_specs
            .iter()
            .find(|spec| &*spec.name == VISION_INSPECT_TOOL),
        Some(&vision_inspect_spec()),
        "the runtime-owned canonical declaration must replace host input"
    );

    let child = session
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: vec![Arc::from(VISION_INSPECT_TOOL)],
                ..ChildConfig::default()
            },
        )
        .unwrap();
    assert_eq!(
        vision_count(&subagent::tools_for(&session, &ctx, &child)),
        0
    );
    assert!(
        !subagent::allowed_names(&session, &ctx, &root)
            .iter()
            .any(|name| &**name == VISION_INSPECT_TOOL),
        "generic spawn must not delegate the reserved facade"
    );
}

#[test]
fn invalid_or_privileged_input_returns_only_the_stable_envelope() {
    for input in [
        json!({"images": ["attachment://img_1"], "question": "what?"}),
        json!({"images": ["img_1"], "question": "what?", "model": "attacker"}),
    ] {
        let mut session = Session::new(AgentId::root());
        let mut ctx = context(true);
        let mut subtree = Subtree::default();
        let dispatched = intercept(
            &mut session,
            &mut ctx,
            &mut subtree,
            &AgentId::root(),
            ToolCallId::new("vision-invalid"),
            &Arc::new(input),
            Epoch::START,
        );
        let Dispatched::Event(Event::ToolFailed { error, .. }) = dispatched else {
            panic!("invalid vision input must fail synchronously");
        };
        let body: Value = serde_json::from_str(&error).unwrap();
        assert_eq!(body["error"]["code"], "invalid_input");
        assert_eq!(body["error"]["retryable"], false);
        assert!(!error.contains("attacker"));
    }
}

#[test]
fn root_dispatch_without_profile_uses_stable_failure_and_child_cannot_use_host_spoof() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let mut ctx = context(false);
    let mut subtree = Subtree::default();
    // 117 接线：`run_effect` 的第四个参数从「泵 channel 的发送端」换成了整条
    // IO 总线（`crate::io_bus::IoBus`）。这两次调用都走不到起飞那一步（都在
    // dispatch 里就被截获/拒绝了），总线只是个必须存在的参数。
    let bus = crate::io_bus::IoBus::new(std::time::Duration::from_millis(20));
    let effect = Effect::ExecuteTool {
        agent: root.clone(),
        call_id: ToolCallId::new("vision-no-profile"),
        tool: Arc::from(VISION_INSPECT_TOOL),
        input: Arc::new(json!({"images": ["img_1"], "question": "what?"})),
        epoch: Epoch::START,
    };
    let Dispatched::Event(Event::ToolFailed { error, .. }) =
        dispatch::run_effect(&mut session, &mut ctx, &mut subtree, &bus, &root, effect)
    else {
        panic!("reserved root call without a profile must fail synchronously");
    };
    let body: Value = serde_json::from_str(&error).unwrap();
    assert_eq!(body["error"]["code"], "vision_profile_unavailable");

    let child = session
        .spawn_child(&root, ChildConfig::default())
        .expect("child");
    let effect = Effect::ExecuteTool {
        agent: child.clone(),
        call_id: ToolCallId::new("vision-child-spoof"),
        tool: Arc::from(VISION_INSPECT_TOOL),
        input: Arc::new(json!({"images": ["img_1"], "question": "what?"})),
        epoch: Epoch::START,
    };
    let Dispatched::Event(Event::ToolFailed { error, .. }) =
        dispatch::run_effect(&mut session, &mut ctx, &mut subtree, &bus, &child, effect)
    else {
        panic!("child must not execute a host-spoofed reserved facade");
    };
    assert_eq!(&*error, "[unknown_tool] unknown tool");
}

#[test]
fn launch_is_isolated_and_retryable_failure_never_repeats_the_paid_call() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let mut ctx = context(true);
    let mut subtree = Subtree::default();

    let dispatched = intercept(
        &mut session,
        &mut ctx,
        &mut subtree,
        &root,
        ToolCallId::new("vision-ok"),
        &Arc::new(json!({
            "images": ["img_9", "img_2"],
            "question": "  Compare only these images.  "
        })),
        Epoch::START,
    );
    let Dispatched::Event(Event::UserInput {
        agent: child,
        text,
        images,
    }) = dispatched
    else {
        panic!("valid vision input must launch one child");
    };

    assert_ne!(child, root);
    assert_eq!(&*text, "Compare only these images.");
    assert_eq!(
        images
            .iter()
            .map(|image| &*image.reference)
            .collect::<Vec<_>>(),
        ["attachment://img_9", "attachment://img_2"]
    );
    assert!(images.iter().all(|image| image.name.is_none()));
    assert!(
        session.messages_of(&child).is_empty(),
        "no parent history is copied"
    );
    assert_eq!(session.tools_allowed_of(&child), Some(Vec::new()));
    assert_eq!(
        session.execution_profile_of(&child),
        Some(ExecutionProfileId::new("vision"))
    );
    assert!(subtree.is_awaited(&child));
    assert!(subagent::tools_for(&session, &ctx, &child).is_empty());

    let first_effects = session.step(Event::UserInput {
        agent: child.clone(),
        text,
        images,
    });
    assert!(
        first_effects
            .iter()
            .any(|effect| matches!(effect, Effect::CallProvider { agent, .. } if agent == &child))
    );
    let terminal_effects = session.step(Event::ProviderFailed {
        agent: child.clone(),
        epoch: session.epoch(),
        class: ErrorClass::Retryable,
        message: Arc::from("transient failure must not trigger a second paid call"),
    });
    assert!(
        terminal_effects
            .iter()
            .all(|effect| !matches!(effect, Effect::CallProvider { .. })),
        "the dedicated vision child has a zero automatic-retry budget"
    );
    assert_eq!(
        session.status_of(&child),
        agent_core::TurnStatus::Failed(Failure::Provider(ErrorClass::Retryable))
    );
}
