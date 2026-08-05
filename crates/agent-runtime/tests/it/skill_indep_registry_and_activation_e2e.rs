//! 039 独立测试(agent-runtime 层,mock provider·零网络):skill 从磁盘
//! `SKILL.md` 到模型请求体的全链路——宿主装载 → 常驻索引进 system → 模型经
//! `srv:skill/activate` 激活 → 同一 turn 的下一跳请求体带上它的正文
//! (`late_system`)和它声明的工具(`late_tools`)→ `undo_turn` 连激活一起退掉。
//!
//! 断言全部对着**假服务器收到的请求体**做——料对不对的唯一判据是模型看到了
//! 什么,不是内部某个 `Vec` 长什么样(跟 `subagent_ingredients.rs` 同一种证明
//! 手法)。假 SSE 服务器复用 `support::routed::RoutedServer`(029 并发假服务器
//! 同款,按请求体内容路由)。
//!
//! 独立测试 agent 规则:只依据 `docs/issues/039-skills-loading.md`、
//! `docs/TOOLS.md` §Skills、`agent-runtime/src/lib.rs` 的公开签名写成,**不看**
//! `crates/agent-runtime/src/` 里 039 新增的 skill registry / 工具实现体。
//!
//! # 假定的公开签名(全篇风险最高的一段,未见实现体)
//!
//! ```ignore
//! pub struct SkillRegistry { .. }
//! impl SkillRegistry {
//!     pub fn load(dirs: &[PathBuf]) -> Result<Self, _>;  // 合并内置+项目多个来源目录
//!     pub fn skill_index_chunk(&self) -> SystemChunk;    // label = "skill-index"
//! }
//! impl ToolTable {
//!     // 声明 srv:skill/activate + srv:skill/deactivate,并让 dispatch 在
//!     // 截获到这两个工具名时能查到 registry 里的正文/工具(跟 with_spawn 同款
//!     // consuming-builder,registry 被 ToolTable 拥有,供 dispatch 在 run_turn
//!     // 期间随时查——不是只在建表那一刻用一次)。
//!     pub fn with_skills(self, registry: SkillRegistry) -> Self;
//! }
//! ```
//!
//! `RunnerCtx` 不走共享的 `support::build_ctx*`——那条路径的 `system` 参数被
//! 硬编码成 `Vec::new()`,而这份测试需要把常驻索引塞进 `system`,所以本文件自己
//! 按 `ctx.rs` 的公开签名装一份(`RunnerCtx::new` 的参数表是 026/027 定型的既有
//! 签名,不属于 039 新增,零新增猜测面)。
//!
//! `srv:skill/activate` 的入参形状按它自己的命名空间猜成 `{"skill": "<id>"}`
//! ——这是第二大风险点,猜错的表现是「工具调用返回 is_error,后续断言全部落空」,
//! 会在独立测试报告里单独点出来。

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent_core::{AgentId, Session, SessionConfig, SkillId, TurnStatus, UndoReport};
use agent_providers::deepseek::DeepSeek;
use agent_runtime::{RunnerCtx, SkillRegistry, ToolTable, run_turn};
use agent_tools::ToolExecutor;
use agent_transport::{Backoff, Client};

use support::routed::{Route, RoutedServer};

const USAGE_STOP: &str = r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#;

const SKILL_BODY_MARKER: &str = "TESTSKILL_BODY_MARKER_ZX91";
const SKILL_INDEX_MARKER: &str = "TESTSKILL_INDEX_MARKER_ZX91";

/// 落一份最小可用的 `skills/testskill/SKILL.md`:frontmatter(name/description/
/// 一个工具)+ 正文。frontmatter 形状照抄本仓自己 `.claude/skills/*/SKILL.md`
/// 的 `---\nname: ..\ndescription: ..\n---` 惯例(`tools:` 列表是额外猜测,
/// issue 原文只说"可选 tools 声明")。
fn write_test_skill(skills_root: &std::path::Path) {
    let dir = skills_root.join("testskill");
    std::fs::create_dir_all(&dir).unwrap();
    // 逐行拼接,不用带换行续行的字符串字面量——反斜杠续行会连带吃掉下一行开头
    // 的空白,YAML 的两格缩进会被悄悄吃掉,坑比省下的几行代码贵。
    let lines = [
        "---".to_string(),
        "name: testskill".to_string(),
        format!("description: 独立测试用的技能,{SKILL_INDEX_MARKER}。"),
        "tools:".to_string(),
        "  - name: srv:testskill/ping".to_string(),
        "    description: 独立测试用的 ping 工具。".to_string(),
        "    schema:".to_string(),
        "      type: object".to_string(),
        "      properties: {}".to_string(),
        "---".to_string(),
        format!("这是 testskill 的正文,激活后整段应该进 late_system。{SKILL_BODY_MARKER}"),
    ];
    std::fs::write(dir.join("SKILL.md"), lines.join("\n") + "\n").unwrap();
}

fn load_registry(skills_root: &std::path::Path) -> SkillRegistry {
    SkillRegistry::load(&[skills_root.to_path_buf()]).expect("加载测试用 skill 目录不该失败")
}

fn build_ctx(port: u16, fs_root: &std::path::Path, registry: SkillRegistry) -> RunnerCtx {
    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_millis(50),
        Backoff { base: Duration::from_millis(10), max_attempts: 1 },
    );
    let fs = ToolExecutor::new(fs_root).unwrap();
    let session_config = SessionConfig {
        model: Arc::from("deepseek-v4-pro"),
        temperature: None,
        max_tokens: None,
        context_window: None,
    };
    let index = registry.skill_index_chunk();
    let tools = ToolTable::builtin().with_skills(registry);

    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(client),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        fs,
        tools,
        vec![index],
        session_config,
        agent_runtime::open_backend(None, |_| {}),
        Box::new(|_| {}),
    )
}

#[test]
fn the_resident_index_is_in_the_very_first_request_before_any_activation() {
    let skills_root = support::temp_dir("skill-e2e-index-skills");
    write_test_skill(&skills_root);
    let fs_root = support::temp_dir("skill-e2e-index-fs");

    let server = RoutedServer::start(vec![Route::sse(
        "",
        vec![
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"你好"},"finish_reason":null}]}"#.to_string(),
            USAGE_STOP.to_string(),
            "data: [DONE]".to_string(),
        ],
    )]);
    let mut ctx = build_ctx(server.port, &fs_root, load_registry(&skills_root));
    let mut session = Session::new(AgentId::root());

    assert_eq!(run_turn(&mut session, &mut ctx, "你好"), TurnStatus::Done { truncated: false });

    let call = server.call("").expect("该有且只有一次请求");
    assert!(
        call.body.contains(SKILL_INDEX_MARKER),
        "常驻索引必须在**第一轮、激活之前**就在请求体里——它跟工具表一样是随时都在的: {}",
        call.body
    );
    assert!(
        !call.body.contains(SKILL_BODY_MARKER),
        "激活之前不该看到正文——索引只是一行摘要,不是把所有 skill 正文都预先塞进去"
    );
}

#[test]
fn activating_mid_turn_injects_body_and_tool_into_the_next_hop_then_undo_removes_both() {
    let skills_root = support::temp_dir("skill-e2e-activate-skills");
    write_test_skill(&skills_root);
    let fs_root = support::temp_dir("skill-e2e-activate-fs");

    let server = RoutedServer::start(vec![
        // 第二跳(路由检查在前,更具体的 needle 先判):请求体里回显了第一跳
        // 那个 tool_call 的 id "call_a",借它精确路由到"收尾"响应。
        Route::sse(
            "call_a",
            vec![
                r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"激活完毕"},"finish_reason":null}]}"#.to_string(),
                USAGE_STOP.to_string(),
                "data: [DONE]".to_string(),
            ],
        ),
        // 第一跳(兜底:第一次请求还没有任何 tool_call 痕迹)——模型直接调用
        // srv:skill/activate({"skill":"testskill"})。
        Route::sse(
            "",
            vec![
                r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"srv_3Askill_2Factivate","arguments":"{\"skill\": \"testskill\"}"}}]}}]}"#.to_string(),
                r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":80,"completion_tokens":15,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":80}}"#.to_string(),
                "data: [DONE]".to_string(),
            ],
        ),
    ]);
    let registry = load_registry(&skills_root);
    let mut ctx = build_ctx(server.port, &fs_root, registry);
    let mut session = Session::new(AgentId::root());

    assert_eq!(run_turn(&mut session, &mut ctx, "帮我激活 testskill"), TurnStatus::Done { truncated: false });

    let calls = server.calls();
    assert_eq!(calls.len(), 2, "一次工具调用 + 一次收敛,该正好两跳请求,实际: {}", calls.len());
    let after_activation = &calls[1];
    assert!(
        after_activation.body.contains(SKILL_BODY_MARKER),
        "激活之后的下一跳请求体该带上 skill 正文(late_system): {}",
        after_activation.body
    );
    assert!(
        after_activation.body.contains("testskill_2Fping") || after_activation.body.contains("testskill/ping"),
        "激活之后的下一跳请求体该带上 skill 声明的工具(late_tools): {}",
        after_activation.body
    );

    assert!(
        session.active_skills().contains(&SkillId::new("testskill")),
        "run_turn 收尾时 testskill 该在 active_skills 里,实际: {:?}",
        session.active_skills()
    );

    let report = session.undo_turn();
    assert!(matches!(report, UndoReport::Applied { .. }), "{report:?}");
    assert!(
        session.active_skills().is_empty(),
        "undo 一整轮该连这轮里发生的激活一起退掉——不需要给 skill 写专门的 undo 代码,实际: {:?}",
        session.active_skills()
    );
}
