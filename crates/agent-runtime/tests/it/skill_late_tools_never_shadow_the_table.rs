//! 064 §验收「跨路径撞名（069）」，**端到端、断在假上游收到的请求体上**。
//!
//! 表里有 `web:crm/close`（宿主注入的），某个 skill 也带一个同名的 →
//! **激活它那一轮的请求体里，这个名字只出现一次，而且是表里那一份的说明书**；
//! 那个 skill 的正文一个字节不少。
//!
//! 断在请求体上而不是断在 `skill_injection` 的返回值上，是因为 069 那条红线说的
//! 就是**进 prompt 的那张表**：
//!
//! > 一个名字在进 prompt 的那张表里只能出现一次，且它的描述/schema 必须就是
//! > dispatch 真会执行的那一份。
//!
//! 「只出现一次」和「是哪一份」都只有在模型真正收到的字节上才判得了。
//! （`skill_injection` 那一层的单测在 `src/tool_table_skill_tests.rs`。）
//!
//! # 会红的那一行
//!
//! `ToolTable::skill_injection` 里那句 `late_tools.retain(|spec| !self.declares(..))`。
//! 删掉它，`the_name_the_table_already_has_appears_exactly_once` 当场红：请求体里
//! `web:crm/close` 出现两次，模型按哪一份的 schema 出参完全看它自己——而只有一份对
//! 得上真正会跑的那件事（069 §拍板：**这正是本仓最怕的那类静默错值，只不过发生在
//! prompt 里而不是 store 里**）。

use crate::support;
use std::sync::Arc;
use std::time::Duration;

use agent_core::{
    AgentId, HostSkill, Reversibility, Session, SessionConfig, SkillId, ToolSpec, TurnStatus,
};
use agent_providers::deepseek::DeepSeek;
use agent_runtime::{run_turn, RunnerCtx, SkillRegistry, ToolTable};
use agent_tools::ToolExecutor;
use agent_transport::{Backoff, Client};
use serde_json::{json, Value};

use crate::support::routed::{Route, RoutedServer};

const USAGE_STOP: &str = r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#;

/// 撞名的那个工具。`web:` 前缀 = 061 允许宿主声明的两种之一。
const CLASH: &str = "web:crm/close";
/// skill 自带、**表里没有**的那个：正对照。没有它，一个「`late_tools` 一律清空」的
/// 实现同样会让主断言绿。
const ONLY_FROM_SKILL: &str = "web:crm/extra";

const TABLE_DESC: &str = "宿主注册的那一份说明书 TABLE_SIDE_MARKER";
const SKILL_DESC: &str = "skill 自带的那一份说明书 SKILL_SIDE_MARKER";
const BODY_MARKER: &str = "CRMFLOW_BODY_MARKER_ZX91";

fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 宿主声明的一个 skill：正文 + 两个自带工具，其中一个跟宿主注入的工具**同名**。
fn declared_skill() -> HostSkill {
    HostSkill {
        id: SkillId::new("crm-flow"),
        description: Arc::from("处理客户工单的标准流程"),
        body: Arc::from(format!(
            "这是 crm-flow 的正文，激活后整段进 late_system。{BODY_MARKER}"
        )),
        tools: vec![
            spec(CLASH, SKILL_DESC),
            spec(ONLY_FROM_SKILL, "只有 skill 带的"),
        ],
        tool_reversibility: [(Arc::from(CLASH), Reversibility::Pure)]
            .into_iter()
            .collect(),
    }
}

#[test]
fn dispatch_uses_the_table_declaration_when_an_active_skill_has_the_same_name() {
    let fs_root = support::temp_dir("skill-clash-dispatch-fs");
    let server = RoutedServer::start(vec![Route::sse(
        "",
        vec![
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_clash","type":"function","function":{"name":"web_3Acrm_2Fclose","arguments":"{\"ticket\":\"T-7\"}"}}]}}]}"#.to_string(),
            r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}]}"#.to_string(),
            "data: [DONE]".to_string(),
        ],
    )]);
    let mut ctx = build_ctx(server.port, &fs_root);
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    session
        .activate_skill(&root, SkillId::new("crm-flow"))
        .unwrap();

    assert_eq!(
        run_turn(&mut session, &mut ctx, "关闭 T-7")
            .expect("remote dispatch should not be a source failure"),
        TurnStatus::ToolsPending
    );

    let pending = ctx.pending_remote_tools();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].agent, root);
    assert_eq!(&*pending[0].call_id.0, "call_clash");
    assert_eq!(&*pending[0].request.tool, CLASH);
    assert_eq!(*pending[0].request.input, json!({ "ticket": "T-7" }));
    assert_eq!(
        pending[0].request.reversibility,
        Reversibility::Irreversible,
        "ToolTable 的同名声明必须赢；skill 那份故意标成 Pure，用不同值钉住 dispatch 优先级"
    );
}

/// 装配形状照 `agent-server` 的 `actor::capabilities::assemble`：
/// 部署期那一档 → `with_skills`（registry 非空才接）→ `with_host_tools`（表尾）。
fn build_ctx(port: u16, fs_root: &std::path::Path) -> RunnerCtx {
    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_millis(50),
        Backoff {
            base: Duration::from_millis(10),
            max_attempts: 1,
        },
    );
    let registry = SkillRegistry::from_host_skills(vec![declared_skill()]);
    let index = registry.skill_index_chunk();
    let tools = ToolTable::builtin()
        .with_skills(registry)
        .with_host_tools(vec![(spec(CLASH, TABLE_DESC), Reversibility::Irreversible)]);

    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(client),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        ToolExecutor::new(fs_root).unwrap(),
        tools,
        vec![index],
        SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        agent_runtime::open_backend(None, |_| {}),
        Box::new(|_| {}),
    )
}

#[test]
fn the_name_the_table_already_has_appears_exactly_once_and_the_skill_body_is_intact() {
    let fs_root = support::temp_dir("skill-clash-fs");
    let server = RoutedServer::start(vec![
        // 第二跳：请求体里回显了第一跳那个 tool_call 的 id。
        Route::sse(
            "call_a",
            vec![
                r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"激活完毕"},"finish_reason":null}]}"#.to_string(),
                USAGE_STOP.to_string(),
                "data: [DONE]".to_string(),
            ],
        ),
        // 第一跳：模型调 srv:skill/activate({"skill":"crm-flow"})。
        Route::sse(
            "",
            vec![
                r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"srv_3Askill_2Factivate","arguments":"{\"skill\": \"crm-flow\"}"}}]}}]}"#.to_string(),
                r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":80,"completion_tokens":15,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":80}}"#.to_string(),
                "data: [DONE]".to_string(),
            ],
        ),
    ]);
    let mut ctx = build_ctx(server.port, &fs_root);
    let mut session = Session::new(AgentId::root());

    assert_eq!(
        run_turn(&mut session, &mut ctx, "帮我激活 crm-flow")
            .expect("skill activation should not be a source failure"),
        TurnStatus::Done { truncated: false }
    );

    let calls = server.calls();
    assert_eq!(
        calls.len(),
        2,
        "一次工具调用 + 一次收敛，该正好两跳请求，实际 {}",
        calls.len()
    );
    let after = &calls[1];
    let tools = tools_of(&after.body);

    // ── 069 那条红线：一个名字在进 prompt 的那张表里只能出现一次。
    let clashing: Vec<&Value> = tools.iter().filter(|t| name_of(t) == CLASH).collect();
    assert_eq!(
        clashing.len(),
        1,
        "{CLASH} 在请求体里出现了 {} 次——两份同名说明书一起进 prompt，模型按哪一份出参完全看它自己，而只有一份对得上真正会跑的那件事（069）\n{}",
        clashing.len(),
        after.body
    );

    // ── 活下来的必须是**表里那一份**：赢家不是选出来的，是 dispatch 早就定死的
    //    （`declares()` 为真是因为表里有它，远端第五路派给的就是宿主注册的那份）。
    assert_eq!(
        clashing[0]["function"]["description"],
        json!(TABLE_DESC),
        "留下来的得是表里那一份说明书——留下 skill 那份等于给模型看一份它影响不了的 schema"
    );

    // ── 正对照：skill 自带、表里没有的那个，原样进这一轮。
    assert!(
        tools.iter().any(|t| name_of(t) == ONLY_FROM_SKILL),
        "只有 skill 带的那个工具必须还在——滤的是撞名那一份，不是把 late_tools 一并清空：{}",
        after.body
    );

    // ── 滤的是**工具**不是 skill：正文一个字节不少。
    assert!(
        after.body.contains(BODY_MARKER),
        "撞名是工具名的事，跟这个 skill 的正文该不该注入没有关系（late_system 一个字节不能少）：{}",
        after.body
    );
    assert!(
        !after.body.contains("SKILL_SIDE_MARKER"),
        "skill 那一份说明书整个都不该出现在请求体里（它连自己的执行路径都没有）：{}",
        after.body
    );
}

/// 请求体里的 `tools` 数组。
fn tools_of(body: &str) -> Vec<Value> {
    let parsed: Value =
        serde_json::from_str(body).unwrap_or_else(|e| panic!("请求体不是 JSON：{e}\n{body}"));
    parsed["tools"].as_array().cloned().unwrap_or_default()
}

/// wire 上的 `function.name` 是转义过的（050），用 provider 自己那把解码器还原——
/// 转义规则不在测试里抄第二遍。
fn name_of(tool: &Value) -> String {
    agent_providers::wire_name::from_wire(tool["function"]["name"].as_str().unwrap_or_default())
        .to_string()
}
