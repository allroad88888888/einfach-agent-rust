//! 最小 CLI 壳（issue 022，012 把它接到真实 loop 上，027 换接 `Session` +
//! 持久化 + `/undo` 家族）：读一行 → 组装 → `agent_runtime::run_turn` 驱动
//! loop → 流式打印。**纯接线**——loop 转移表在 `agent-core`、encode/decode/
//! stream 判断在 `agent-providers`、IO 编排在 `agent-runtime`，这里只装配。
//!
//! # Ctrl-C 方案：选了 `ctrlc` 依赖，不是纯 std 方案
//!
//! 沿用 022 的裁决（见那份 issue 与本文件历史版本的记录）：验收标准原话是
//! 「`Ctrl-C` 能中断正在流的响应，进程不退出」，只有真信号捕捉能做到，纯
//! std 轮询方案会把这句验收标准打折扣成「/cancel 能中断」。
//!
//! 012 把取消标志从「CLI 自己造一个 `AtomicBool`」换成
//! `RunnerCtx::cancel_flag()`——同一份标志现在也是 `run_turn` 内部判断
//! Ctrl-C-during-CallProvider 的那个标志，语义没变，只是标志的所有权挪进了
//! `agent-runtime`。014 验过这条线到今天还接着：这里装的 handler 翻的就是
//! `ctx.cancel_flag()` 本身，没有第二份标志。
//!
//! # 按配置分发 adapter（023），运行时切换（014）
//!
//! 初始 provider 由 [`agent_cli::provider::build_provider`] 按
//! `[default] provider` 的名字查表；`/model <name>` 运行时切换复用同一张表
//! （`agent_cli::model_switch`），两处不各自维护一份容易分叉的名字集合。
//!
//! # 027：会话文件与崩溃恢复
//!
//! [`agent_cli::session_path::resolve`] 决定这次跑的是持久会话（`--session
//! <path>` 或 `AGENT_SESSION_PATH`）还是临时会话（两者都没有，`Memory`
//! 后端，进程退出即丢）。有会话文件且里面有货 → `agent_runtime::recover`
//! 重建 `Session`；翻译/重建失败（`RecoverError`）直接拒绝启动，不硬凑一个
//! 能跑但是错的状态（`docs/issues/011-session-store.md` 的诚实原则）。
//!
//! `mod` 声明搬去了 `lib.rs`（原因见那份文件顶部注释），这个文件只剩「装配」
//! 本身。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use agent_cli::{
    agent_limits, ext_stats, mcp, print, provider, repl, session_path, session_start, tool_table,
    vision,
};
use agent_core::{AgentId, Session, SessionConfig, SystemChunk};
use agent_runtime::{RunnerCtx, SkillRegistry};
use agent_tools::ToolExecutor;
use agent_transport::{Client, config};

fn main() {
    let root = match config::load() {
        Ok(r) => r,
        Err(e) => fail(&format!("配置加载失败: {e}")),
    };
    let provider_cfg = match config::default_provider(&root) {
        Ok(p) => p,
        Err(e) => fail(&format!("{e}")),
    };
    let provider_name = root.default.provider.as_str();
    let adapter = match provider::build_provider(provider_name) {
        Ok(p) => p,
        Err(e) => fail(&e),
    };
    let Some(api_key) = provider_cfg.resolve_key() else {
        fail(
            "provider 没配 key：检查 providers.toml 里的 api_key，或对应的 api_key_env 指向的环境变量",
        );
    };

    // 内置工具（013）锁在启动时的当前工作目录之内——CLI 从哪启动，工具就只能
    // 读那棵目录树，不是整个文件系统（`ToolExecutor` 的路径监狱）。
    let tool_root = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => fail(&format!("拿不到当前工作目录: {e}")),
    };
    let mut fs = match ToolExecutor::new(&tool_root) {
        Ok(fs) => fs,
        Err(e) => fail(&format!(
            "内置工具初始化失败（root={}）: [{}] {}",
            tool_root.display(),
            e.code,
            e.message
        )),
    };
    // s5：识图工具（srv:vision/inspect，写死 Kimi 3）。CLI 没有 server 的上传
    // 端点——`image` 按启动目录内的本地相对路径解析（`VisionLinkSource::
    // LocalRoot`）。没配 kimi 段或没可用 key → 工具不配置、不声明，模型根本
    // 不知道有它（跟 `agent-server::bootstrap::resolve_vision` 同一个「vision
    // 是可选项」取舍，只是链接来源换成 LocalRoot）。
    let vision = vision::resolve(&root, &tool_root);
    let vision_enabled = vision.is_some();
    if let Some(v) = vision {
        fs = fs.with_vision(v);
    }

    // 只打长度/状态，永远不打 key 本身；provider 打的是配置里的名字，不是
    // 写死的字符串——这正是 023 要修的那个接线 bug。
    eprintln!(
        "provider={provider_name} model={} endpoint={} key=已配置（{} 字符） tools_root={} context_window={:?}",
        provider_cfg.model,
        provider_cfg.endpoint(),
        api_key.len(),
        tool_root.display(),
        provider_cfg.context_window,
    );
    eprintln!("vision={}", vision::banner(vision_enabled));

    let args: Vec<String> = std::env::args().collect();
    let session_file = session_path::resolve(&args);
    if let Some(path) = &session_file {
        eprintln!("会话文件={}", path.display());
    } else {
        eprintln!(
            "会话文件=（未指定，临时会话，进程退出即丢——用 --session <path> 或 AGENT_SESSION_PATH 落盘）"
        );
    }
    // 039：从项目 `./skills/`（相对启动目录）装载 skill。装载失败不致命——退回
    // 空 registry，CLI 照跑，只是没有 skill 可激活。139 起索引不再是这里手拼的
    // system chunk——`with_skills` 把 `srv:skill/index` 挂进 `SessionStart` 时机
    // 区，下面 135 的 `session_start::maybe_run` 跑完之后它自己落进
    // `Session::prefix_chunks()`，跟工具表一样随时都在；这里只装载 registry，
    // 激活集在会话状态里。
    let skills = SkillRegistry::load(&[tool_root.join("skills")]).unwrap_or_else(|e| {
        eprintln!("[skills] 装载失败，按无 skill 继续: {e}");
        SkillRegistry::empty()
    });
    eprintln!(
        "skills={}",
        if skills.is_empty() {
            "（无）".to_string()
        } else {
            skills.listing().len().to_string()
        }
    );

    // 045：从 `.mcp.json`（默认启动目录，`--mcp-config <path>` 可覆盖）装载 MCP server。
    // 缺失 / 坏配置不致命——退回零 MCP 工具，CLI 照跑（跟上面 skill 装载失败同一个精神）。
    // 活句柄进 `registry`（红线 3，store 外），工具批追加进下面的 `ToolTable`（红线 11，
    // 只加不改），装载状态给 `/mcp` 命令。**每次启动都跑**——kill-9 重启后 server 从这里
    // 重新 spawn，不从快照复活（docs/MCP.md §「活句柄住 store 外」）。
    let (mcp_config_path, mcp_explicit) = mcp::resolve_config_path(&args);
    let mcp = mcp::bootstrap(&mcp_config_path, mcp_explicit, &mut |m| {
        eprintln!("[mcp] {m}")
    });
    eprintln!("mcp={}", mcp.summary());

    let store = agent_runtime::open_backend(session_file.clone(), |e| eprintln!("[会话文件] {e}"));

    // 162（决策 32）：两道结构性硬限从 `--max-agent-depth`/`--max-children`（env
    // 兜底）来。配错了**拒绝启动**，不静默退默认档——理由见 `agent_limits` 模块
    // 文档。**同一份值喂给两条路**：新建会话走下面的 `set_agent_limits`，恢复走
    // `recover` 的 `limits` 入参（160），于是「新建的」和「恢复的」拿到同一组数。
    let limits = agent_limits::from_args_and_environment(&args)
        .unwrap_or_else(|message| fail(&format!("参数错误：{message}")));
    eprintln!("子 agent 上限={}", agent_limits::banner(limits));

    // 135：新建会话才跑开局工具——记下这一支，装完工具表后据此决定要不要调。
    let mut is_new_session = false;
    let mut session = match agent_runtime::recover(
        store.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        limits,
        &mut |key| {
            eprintln!("[会话文件] 快照里有一个这一版不认识的键，已忽略：{key:?}");
        },
    ) {
        Ok(Some(session)) => {
            print::session_recovered(session.turn_id());
            if agent_runtime::has_unresolved_tool_calls(&session) {
                print::unresolved_tool_call_notice();
            }
            session
        }
        Ok(None) => {
            is_new_session = true;
            let mut session = Session::new(AgentId::root());
            // 162：恢复那条路由上面的 `recover` 入参带进去，新建这条路在这里补，
            // 两条拿到同一组数。顺序要紧——下面 `with_spawn(session.agent_limits())`
            // 读的就是这一刻的值，颠倒过来模型会看到默认档的数字而闸是别的。
            session.set_agent_limits(limits);
            session
        }
        Err(e) => fail(&format!("{e}")),
    };
    let recovered_source_needs_fail_close =
        agent_runtime::recovered_transient_source_needs_fail_close(&session);

    let mut printer = print::EventPrinter::new();
    // 工具表的组成与顺序（红线 11 的契约）整段住 `tool_table` 模块——`limits`
    // 传的是 `session.agent_limits()` 而不是上面那个 `limits` 变量：两者此刻相等
    // （恢复走 `recover` 入参、新建走 `set_agent_limits`），但**真正该跟工具描述
    // 对齐的是会话手上那一份**，从会话读就不会有第二个真相。
    let (tool_table, ext_pending) = tool_table::assemble(
        tool_table::Parts {
            limits: session.agent_limits(),
            skills,
            mcp_tools: mcp.tools,
            vision: vision_enabled,
            ext_stats: ext_stats::enabled(&args),
            session_file: tool_table::owned(session_file.as_deref()),
        },
        &mut |m| eprintln!("ext-stats={m}"),
    );
    let mut ctx = RunnerCtx::new(
        Arc::from(adapter),
        Arc::new(Client::new()),
        provider_cfg.endpoint(),
        api_key,
        fs,
        tool_table,
        // skill 索引不在这里了（139）：它经 `session_start::maybe_run` 落进
        // `Session::prefix_chunks()`，`subagent::system_for` 会把这段基础 system
        // 之后、前缀块之前的顺序原样接上——这个 `Vec` 只留跟会话形态无关的那一段。
        vec![SystemChunk {
            label: Arc::from("base"),
            text: Arc::from("你是一个简洁、诚实的助手。"),
        }],
        SessionConfig {
            model: Arc::from(provider_cfg.model.as_str()),
            temperature: None,
            max_tokens: None,
            // 110 前置：从 `providers.toml` 里这家的配置取，不是硬编码
            // `None`——没配就是 `None`（安全默认，不触发任何一档压缩），
            // 配了就一路传到 `compact_ladder` 的触发判断。
            context_window: provider_cfg.context_window,
        },
        store,
        // `new` 收的这条是 M1..M2 的不带归属回调，CLI 不用它——真正的打印走下面
        // 的 `with_agent_events`（带 agent 前缀）。两条 sink 是**同一个字段**，
        // 后设的替换先设的，不存在「两条都在发」。
        Box::new(|_| {}),
    )
    .with_agent_events(Box::new(move |ev| printer.handle(ev)))
    // 活句柄表进 ctx（红线 3，store 外）：dispatch 的第四路（`mcp:` 前缀且工具表声明）
    // 拿它 + server id 查 client 起异步 `tools/call`。没连上任何 server 就是空表。
    .with_mcp(mcp.registry);
    // 149：扩展包的 ctx 半边（截获执行体）。两半边来自同一个包实例，机制保证；
    // 没开开关时是 `None`，一行都不跑。
    if let Some(pending) = ext_pending {
        pending.install(&mut ctx);
    }
    // 恢复之后必调——`persisted_seq` 这个同步水位不对齐，`persist::sync` 会把
    // `Session::restore` 灌回来的旧条目当新条目重新 append 一遍，连续几次
    // 「重启」之后 seq 会在文件中段跌回 0，下一次启动直接撞
    // `SeqNotIncreasing` 硬失败（独测抓到的真 bug 1，见
    // `agent_runtime::persist::seed_after_recover` 文档「真 bug」一节）。对全新
    // 会话是无害的空操作，不需要在这里分支判断「是不是恢复出来的」。
    agent_runtime::persist::seed_after_recover(&mut ctx, &session);
    // 135：工具表装完之后跑一次开局工具，只在新建会话那一支。**必须排在
    // `seed_after_recover` 之后**（139 修的真 bug）：`maybe_run` 会给新会话追加
    // 一条 journaled 的 `prefix_init` entry；排在 `seed_after_recover` 之前，
    // 这条刚写的 entry 会被误判成「已经在盘上」，从此永远不被 `persist::sync`
    // 真正落盘，重启即丢——跟上面这条真 bug 1 是同一个类别、这次是新写入把
    // `seed_after_recover` 的假设悄悄破坏。
    if let Err(msg) = session_start::maybe_run(is_new_session, &mut session, ctx.tools()) {
        fail(&msg);
    }
    if recovered_source_needs_fail_close {
        if let Err(failure) = agent_runtime::cancel_pending_remote_tools(&mut session, &mut ctx) {
            eprintln!("{failure:?}");
        }
    }

    let cancel = ctx.cancel_flag();
    if let Err(e) = ctrlc::set_handler(move || {
        cancel.store(true, Ordering::Relaxed);
    }) {
        eprintln!("装 Ctrl-C 处理器失败（{e}）：Ctrl-C 会退回系统默认行为，直接杀掉进程。");
    }

    println!(
        "输入一句话开始对话。命令：/quit 退出，/model <name> 切换 provider（可选：{}），\
         /undo 撤销上一轮，/redo 重做，/undo! 越过不可逆操作强制撤销，/skills 看已装载的技能，\
         /mcp 看 MCP server 状态。",
        provider_names(&root)
    );
    repl::run(&mut session, &mut ctx, &root, &mcp.status);
}

/// 启动横幅里报「可选哪些」，直接读已加载的 `providers.toml`——跟
/// `/model` 未知名字时报的可选值同一个数据源，不是另写一份写死的列表。
fn provider_names(root: &config::RootConfig) -> String {
    root.providers
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(" / ")
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}
