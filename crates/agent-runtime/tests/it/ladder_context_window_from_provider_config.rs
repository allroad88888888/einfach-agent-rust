//! 110 前置验收核心：`context_window` 从 `providers.toml`（`agent_transport::
//! config::ProviderConfig`）解出来，一路喂给 `SessionConfig::context_window`，
//! 用量过线时第 2 档真的开火——证明的不是「阶梯逻辑本身对不对」（那是
//! 095/096/108 的范围，`ladder_tier2_alone_suffices.rs` 等文件已经测得很细），
//! 证明的是**这条配置没有在五个宿主构造点的任何一个途中被悄悄换成 `None`**。
//!
//! 跟 `ladder_support::build_ctx` 的其余用例唯一的差别：那些测试的
//! `context_window` 是测试自己钦定的字面量（`Some(WINDOW)`），这里的
//! `context_window` 来自 `toml::from_str::<RootConfig>` 真解析出来的
//! `ProviderConfig::context_window`——跟 `agent_cli::main`/`agent_server::
//! bootstrap` 装配 `SessionConfig` 时读的是同一个字段、同一条路径。

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::ToolTable;
use agent_transport::config::{self, RootConfig};

use crate::ladder_support::{build_ctx, text_response, tool_call_response};
use crate::support;
use crate::support::ScriptedResponse;

const LOW: u32 = 400; // 40%，远低于 85% 触发线
const HIGH: u32 = 900; // 90%，冲过 85%

/// 跟真实 `providers.toml` 同一个形状——`context_window = 1000` 是这条测试
/// 唯一关心的键，其余字段凑够 `ProviderConfig`/`RootConfig` 能解析出来就够。
const PROVIDERS_TOML: &str = r#"
[providers.deepseek]
api_key = "fake-key"
base_url = "http://127.0.0.1:1"
model = "deepseek-v4-pro"
context_window = 1000

[default]
provider = "deepseek"
"#;

fn leak(lines: Vec<String>) -> Vec<&'static str> {
    lines
        .into_iter()
        .map(|l| -> &'static str { Box::leak(l.into_boxed_str()) })
        .collect()
}

#[test]
fn a_context_window_parsed_from_provider_config_makes_tier_two_fire() {
    let root: RootConfig = toml::from_str(PROVIDERS_TOML).expect("测试配置必须合法");
    let provider_cfg = config::default_provider(&root).expect("[default] 指的段必须存在");
    // 先钉住这条测试的前提：解析出来的就是配置文件里写的那个数，不是某个
    // 巧合相等的默认值。
    assert_eq!(provider_cfg.context_window, Some(1000));

    let dir = support::temp_dir("ladder-context-window-from-provider-config");
    std::fs::write(dir.join("seed.txt"), b"SEED-CONTENT").unwrap();

    let script = vec![
        // 第 1 轮：工具调用，建立一段之后可以被第 2 档清掉的历史。
        ScriptedResponse::Sse(leak(tool_call_response(
            "call_r1",
            "srv_3Afs_2Fread",
            r#"{"path": "seed.txt"}"#,
            LOW,
        ))),
        ScriptedResponse::Sse(leak(text_response("读完了", LOW))),
        // 第 2、3、4 轮：把第 1 轮的工具结果挤出「最近 3 轮」保护区。
        ScriptedResponse::Sse(leak(text_response("继续", LOW))),
        ScriptedResponse::Sse(leak(text_response("继续", LOW))),
        ScriptedResponse::Sse(leak(text_response("继续", LOW))),
        // 第 5 轮：usage 冲过阈值——第 2 档该在这一轮末开火。
        ScriptedResponse::Sse(leak(text_response("继续", HIGH))),
    ];
    let (port, _bodies) = support::spawn_recording_server(script);
    // 这里是全条测试的关键一行：`context_window` 不是这条测试自己写的字面量，
    // 是刚从 TOML 解析出来的 `provider_cfg.context_window`——跟五个真实宿主
    // 构造 `SessionConfig` 时读的是同一个字段。
    let (mut ctx, _events) = build_ctx(
        port,
        &dir,
        ToolTable::builtin(),
        provider_cfg.context_window,
    );
    let mut session = Session::new(AgentId::root());
    let root_agent = AgentId::root();

    for (i, text) in [
        "读一下 seed.txt",
        "继续聊",
        "继续聊",
        "继续聊",
        "继续聊", // 这一轮末 usage 冲线，第 2 档该开火
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            session.begin_turn();
        }
        let status = agent_runtime::run_turn(&mut session, &mut ctx, text)
            .unwrap_or_else(|e| panic!("{text} 不该是 source failure：{e:?}"));
        assert_eq!(status, TurnStatus::Done { truncated: false }, "{text}");
    }

    let plan = session.send_plan_of(&root_agent);
    assert!(
        !plan.cleared().is_empty(),
        "配置里的 context_window 该已经传到触发判断，第 2 档该清过东西——\
         为空说明这条配置在某个构造点上丢了，又静默退化回「不触发」"
    );
}
