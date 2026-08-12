//! `subagent.rs` 的单元测试：固定模板的稳定性 + 145 的前缀过滤（纯函数与
//! `system_for` 端到端两层）+ 看门狗。拆到独立文件是 328 行顶破红线 9 之后的
//! 直接后果（`#[path]` 子模块，`super` 仍是 `subagent`，私有项照样够得着）。

use super::*;

/// 红线 11 的最小实检：同一份 limits 两次渲染逐字节相同，不同 limits 不同。
#[test]
fn the_fixed_template_is_byte_stable_for_a_given_limit_pair() {
    let a = subagent_prompt(AgentLimits {
        max_depth: 3,
        max_children: 8,
    });
    let b = subagent_prompt(AgentLimits {
        max_depth: 3,
        max_children: 8,
    });
    assert_eq!(a, b);
    assert_ne!(
        a,
        subagent_prompt(AgentLimits {
            max_depth: 2,
            max_children: 8
        })
    );
}

/// 模板里不许出现任务文本的位置——它是子 agent 的第一条 user 消息。
/// 这条断言钉的是模块文档那个前缀共享的理由：模板只依赖 limits，
/// 不依赖任何一次 spawn 的入参。
#[test]
fn the_template_depends_on_nothing_but_the_limits() {
    let text = subagent_prompt(AgentLimits::default());
    assert!(text.contains("子任务执行者"));
    assert!(!text.contains("{}"), "格式串该被填满：{text}");
}

fn chunk(label: &str, text: &str) -> SystemChunk {
    SystemChunk {
        label: Arc::from(label),
        text: Arc::from(text),
    }
}

/// 145 缺省：`None` 原样放行——`system_for` 逐字节回到 145 之前（红线 11）。
#[test]
fn filter_prefix_chunks_none_keeps_everything() {
    let chunks = vec![chunk("init:a", "A"), chunk("init:b", "B")];
    assert_eq!(filter_prefix_chunks(chunks.clone(), None), chunks);
}

/// `Some(&[])`：145 三档语义里的「一点都不带」——一块都不留。
#[test]
fn filter_prefix_chunks_empty_set_drops_everything() {
    let chunks = vec![chunk("init:a", "A"), chunk("init:b", "B")];
    assert!(filter_prefix_chunks(chunks, Some(&[])).is_empty());
}

/// `Some(set)`：只留 label 里的名字落在 `set` 里的那几块，其余（包括
/// 不匹配 `init:` 形状的块）都被拿掉。
#[test]
fn filter_prefix_chunks_some_keeps_only_the_named_ones() {
    let chunks = vec![
        chunk("init:a", "A"),
        chunk("init:b", "B"),
        chunk("init:c", "C"),
        chunk("subagent", "不是 init: 形状，过滤生效时也该被拿掉"),
    ];
    let allowed: Vec<Arc<str>> = vec![Arc::from("b")];
    assert_eq!(
        filter_prefix_chunks(chunks, Some(&allowed)),
        vec![chunk("init:b", "B")]
    );
}

use std::sync::atomic::{AtomicUsize, Ordering};

use agent_core::SessionConfig;
use agent_providers::deepseek::DeepSeek;
use agent_tools::ToolExecutor;
use agent_transport::Client;

use crate::tool_table::{CallTiming, ToolTable};

fn build_ctx(tools: ToolTable) -> RunnerCtx {
    let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "https://api.deepseek.com/chat/completions".to_string(),
        "deepseek-key".to_string(),
        fs,
        tools,
        Vec::new(),
        SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        crate::persist::open_backend(None, |_| {}),
        Box::new(|_| {}),
    )
}

/// 端到端一小段：`prefix_allowed_of` 的三档值真的驱动 `system_for` 的
/// 输出——不是只有纯函数 `filter_prefix_chunks` 自己测自己。
#[test]
fn system_for_honors_the_prefix_allowed_slot_of_each_child() {
    let table = ToolTable::builtin().with_timed(
        ToolSpec {
            name: Arc::from("srv:skill/index"),
            description: Arc::from("技能索引"),
            schema: Arc::new(serde_json::json!({ "type": "object" })),
        },
        CallTiming::SessionStart,
        Box::new(|_table, _input| Ok(Arc::from("INDEX-TEXT"))),
    );
    let mut session = Session::new(AgentId::root());
    crate::session_start::run_session_start(&mut session, &table).expect("唯一工具该成功");
    let ctx = build_ctx(table);

    let default_child = session
        .spawn_child(&AgentId::root(), agent_core::ChildConfig::default(), None)
        .unwrap();
    let excluded_child = session
        .spawn_child(
            &AgentId::root(),
            agent_core::ChildConfig::default(),
            Some(Vec::new()),
        )
        .unwrap();
    let included_child = session
        .spawn_child(
            &AgentId::root(),
            agent_core::ChildConfig::default(),
            Some(vec![Arc::from("srv:skill/index")]),
        )
        .unwrap();

    let has_index = |agent: &AgentId| {
        system_for(&session, &ctx, agent)
            .iter()
            .any(|c| &*c.label == "init:srv:skill/index")
    };
    assert!(has_index(&default_child), "缺省该全带（红线 11 向后兼容）");
    assert!(!has_index(&excluded_child), "[] 该一点都不带");
    assert!(has_index(&included_child), "点名的该带上");
}

/// 看门狗（145 §做什么 第 5 条）：spawn 两个子 agent、各自把它们的 system
/// 组一遍（这是每一轮都会做的事），开局工具的执行计数仍然是 1——组 system
/// 只读 `session.prefix_chunks()` 这份缓存，`filter_prefix_chunks` 不重跑
/// 任何 `TimedRun`。这条红了，说明有人把过滤实现成了「按名单重新执行」。
#[test]
fn spawning_children_and_building_their_systems_does_not_rerun_session_start() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let table = ToolTable::builtin().with_timed(
        ToolSpec {
            name: Arc::from("srv:skill/index"),
            description: Arc::from("技能索引"),
            schema: Arc::new(serde_json::json!({ "type": "object" })),
        },
        CallTiming::SessionStart,
        Box::new(move |_table, _input| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::from("INDEX-TEXT"))
        }),
    );
    let mut session = Session::new(AgentId::root());
    crate::session_start::run_session_start(&mut session, &table).expect("唯一工具该成功");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "新建会话该执行一次");
    let ctx = build_ctx(table);

    let child_a = session
        .spawn_child(
            &AgentId::root(),
            agent_core::ChildConfig::default(),
            Some(vec![Arc::from("srv:skill/index")]),
        )
        .unwrap();
    let child_b = session
        .spawn_child(&AgentId::root(), agent_core::ChildConfig::default(), None)
        .unwrap();

    let _ = system_for(&session, &ctx, &AgentId::root());
    let _ = system_for(&session, &ctx, &child_a);
    let _ = system_for(&session, &ctx, &child_b);
    // 「跑完一轮」还会问一次它们各自的工具表——同样不该碰 timed 执行体。
    let _ = tools_for(&session, &ctx, &child_a);
    let _ = tools_for(&session, &ctx, &child_b);

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "spawn 两个子 agent 并组完它们的 system/tools 之后，开局工具执行计数该仍是 1"
    );
}
