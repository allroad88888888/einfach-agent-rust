//! `/model <name>` 运行时切 provider（014 缺口 1）。
//!
//! 切换 = 换 adapter + endpoint + key + model（从已加载的 `providers.toml`
//! 里取，`RunnerCtx::switch_provider` 顺带清第 3 层滚动窗口——理由记在那份
//! 文档注释）+ 清 `Session` 的前缀镜像（跨家前缀无意义，不清会让第 1 层
//! 把「正常换家」误报成「前缀漂移」）。**消息历史保留**——跨家续聊是合法
//! 场景，这个函数不碰消息历史。
//!
//! 027：清前缀镜像换成 `Session::clear_prev_prefix()`——`TurnState` 时代
//! 直接赋值 `state.prev_prefix = None` 绕过 undo log，`Session` 的字段全私有，
//! 红线 2 逼着这条也走命令层（新增的 `Session::clear_prev_prefix`）。
//!
//! 名字对不上 `providers.toml` 任何一段，或者对应不上任何已知 adapter，都
//! 打一行错误、原样保留当前 provider，不 panic、不把 `ctx` 改到一半就中止
//! （先把 `provider_cfg`/`adapter`/`api_key` 三样都拿到手，验证全过才动
//! `ctx`——`switch_provider` 一旦被调用就不会失败）。

use std::sync::Arc;

use agent_core::Session;
use agent_runtime::RunnerCtx;
use agent_transport::config::RootConfig;

use crate::{print, provider};

/// 处理一句已经去掉 `/model ` 前缀、`trim` 过的名字。
pub fn switch(name: &str, ctx: &mut RunnerCtx, session: &mut Session, config: &RootConfig) {
    let Some(provider_cfg) = config.providers.get(name) else {
        let names: Vec<&str> = config.providers.keys().map(String::as_str).collect();
        print::model_switch_error(&format!(
            "未知 provider \"{name}\"。可选：{}",
            names.join(" / ")
        ));
        return;
    };
    let adapter = match provider::build_provider(name) {
        Ok(p) => p,
        Err(e) => {
            print::model_switch_error(&e);
            return;
        }
    };
    let Some(api_key) = provider_cfg.resolve_key() else {
        print::model_switch_error(&format!(
            "provider \"{name}\" 没配 key：检查 providers.toml 里的 api_key，或对应的 api_key_env 指向的环境变量"
        ));
        return;
    };

    ctx.switch_provider(
        Arc::from(adapter),
        provider_cfg.endpoint(),
        api_key,
        Arc::from(provider_cfg.model.as_str()),
    );
    // 跨家前缀镜像无意义：不清的话 024 第 1 层会拿新家这次请求的裸字节去比对
    // 旧家上一轮的 PrefixImage，两家的料单形状本来就不同，比出来的「漂移」
    // 只是切换本身造成的噪音，不是真的前缀坏了。
    session.clear_prev_prefix();
    agent_runtime::persist::sync(ctx, session);

    print::model_switched(name, &provider_cfg.model, &provider_cfg.endpoint());
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::seam::{PrefixImage, Segment, SegmentImage};
    use agent_core::{AgentId, ContentBlock, Event, StopReason, TokenUsage};
    use agent_providers::deepseek::DeepSeek;
    use agent_runtime::ToolTable;
    use agent_tools::ToolExecutor;
    use agent_transport::Client;

    /// 三段夹具刚好覆盖三条要走的路：`deepseek`（切换会成功）、`kimi`
    /// （切换会成功，用来证明真的换了家）、`glm`（存在配置段但没配 key，
    /// 用来覆盖「provider/adapter 都对，但 key 没配」这条错误路径）。
    const FIXTURE: &str = r#"
[providers.deepseek]
api_key = "deepseek-fixture-key"
base_url = "https://api.deepseek.com"
model = "deepseek-v4-pro"

[providers.kimi]
api_key = "kimi-fixture-key"
base_url = "https://api.moonshot.cn/v1"
model = "kimi-k3"

[providers.glm]
base_url = "https://open.bigmodel.cn/api/paas/v4"
model = "glm-5.2"

[default]
provider = "deepseek"
"#;

    fn fixture_config() -> RootConfig {
        toml::from_str(FIXTURE).unwrap()
    }

    fn build_ctx() -> RunnerCtx {
        let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
        RunnerCtx::new(
            Arc::new(DeepSeek),
            Arc::new(Client::new()),
            "https://api.deepseek.com/chat/completions".to_string(),
            "deepseek-fixture-key".to_string(),
            fs,
            ToolTable::builtin(),
            Vec::new(),
            agent_core::SessionConfig {
                model: Arc::from("deepseek-v4-pro"),
                temperature: None,
                max_tokens: None,
                context_window: None,
            },
            agent_runtime::open_backend(None, |_| {}),
            Box::new(|_ev| {}),
        )
    }

    /// 有历史、有前缀镜像的一份会话——模拟「已经跟 deepseek 聊过一轮」。
    /// 跟 `TurnState` 时代不同，`Session` 没有公开字段可以直接摆值，只能真的
    /// 跑一轮转移（`Event::UserInput` → `Event::ProviderDone`）。
    fn session_with_history_and_prefix() -> Session {
        let mut session = Session::new(AgentId::root());
        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: "之前问过的问题".into(),
            images: Vec::new(),
        });
        let _ = session.step(Event::ProviderDone {
            agent: AgentId::root(),
            epoch: session.epoch(),
            blocks: vec![ContentBlock::Text("之前的回答".into())],
            stop: StopReason::EndTurn,
            usage: TokenUsage {
                prompt: 800,
                completion: 10,
                cached: None,
            },
            prefix: PrefixImage {
                segments: vec![SegmentImage {
                    segment: Segment::Tools,
                    bytes: 512,
                    hash: 42,
                }],
                prompt_tokens: None, // 转移表会用 usage.prompt 回填
            },
            adjustments: Vec::new(),
        });
        session
    }

    /// 验收原文点名的三件事：切到 kimi 之后——历史保留、`prev_prefix` 被清。
    /// adapter/endpoint/model 真的换了这件事在 `agent-runtime::ctx` 的
    /// `switch_provider_encode_reflects_the_new_family_not_the_old` 测试里
    /// 已经用真实 `encode()` 验过，这里不重复。
    #[test]
    fn switching_to_kimi_keeps_history_and_clears_prev_prefix() {
        let config = fixture_config();
        let mut ctx = build_ctx();
        let mut session = session_with_history_and_prefix();

        switch("kimi", &mut ctx, &mut session, &config);

        assert_eq!(
            session.messages().len(),
            2,
            "跨家续聊是合法场景，历史不该被切换动过"
        );
        assert!(
            session.prev_prefix().is_none(),
            "跨家前缀镜像无意义，必须被清掉，否则第 1 层会把切换误报成漂移"
        );
    }

    /// 未知名字：不 panic，会话原样不动（没有半途改一半）——用 `primitives()`
    /// 逐值比对代替 `TurnState` 时代的 `assert_eq!(state, before)`。
    #[test]
    fn unknown_provider_name_leaves_state_untouched() {
        let config = fixture_config();
        let mut ctx = build_ctx();
        let mut session = session_with_history_and_prefix();
        let before = session.primitives();

        switch("not-a-real-provider", &mut ctx, &mut session, &config);

        assert_eq!(
            session.primitives(),
            before,
            "未知名字该原样保留当前状态，不是改到一半就中止"
        );
    }

    /// 配置段存在、adapter 也认得这个名字，但没配 key：同样不该动会话。
    #[test]
    fn provider_without_key_leaves_state_untouched() {
        let config = fixture_config();
        let mut ctx = build_ctx();
        let mut session = session_with_history_and_prefix();
        let before = session.primitives();

        switch("glm", &mut ctx, &mut session, &config);

        assert_eq!(
            session.primitives(),
            before,
            "没配 key 该报错保留原状态，不是切换到一半"
        );
    }
}
