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

use agent_cli::{mcp, print, provider, repl, session_path};
use agent_core::{AgentId, Session, SessionConfig, SystemChunk};
use agent_runtime::{RunnerCtx, SkillRegistry, ToolTable};
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
    let fs = match ToolExecutor::new(&tool_root) {
        Ok(fs) => fs,
        Err(e) => fail(&format!(
            "内置工具初始化失败（root={}）: [{}] {}",
            tool_root.display(),
            e.code,
            e.message
        )),
    };

    // 只打长度/状态，永远不打 key 本身；provider 打的是配置里的名字，不是
    // 写死的字符串——这正是 023 要修的那个接线 bug。
    eprintln!(
        "provider={provider_name} model={} endpoint={} key=已配置（{} 字符） tools_root={}",
        provider_cfg.model,
        provider_cfg.endpoint(),
        api_key.len(),
        tool_root.display(),
    );

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
    // 空 registry，CLI 照跑，只是没有 skill 可激活。索引常驻进 system 前缀（跟
    // 工具表一样随时都在），激活集在会话状态里。
    let skills = SkillRegistry::load(&[tool_root.join("skills")]).unwrap_or_else(|e| {
        eprintln!("[skills] 装载失败，按无 skill 继续: {e}");
        SkillRegistry::empty()
    });
    let skill_index = skills.skill_index_chunk();
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

    let store = agent_runtime::open_backend(session_file, |e| eprintln!("[会话文件] {e}"));

    let mut session = match agent_runtime::recover(
        store.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
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
        Ok(None) => Session::new(AgentId::root()),
        Err(e) => fail(&format!("{e}")),
    };
    let recovered_source_needs_fail_close =
        agent_runtime::recovered_transient_source_needs_fail_close(&session);

    let mut printer = print::EventPrinter::new();
    let mut ctx = RunnerCtx::new(
        Arc::from(adapter),
        Arc::new(Client::new()),
        provider_cfg.endpoint(),
        api_key,
        fs,
        // 本地标准工具集含受版本保护、可显式撤回的文件事务；不会把浏览器/桌面
        // 交互伪装成本地工具。随后保留既有 spawn 开关，上限传的是
        // `session.agent_limits()`——工具描述里告诉模型的数字，必须跟真正拦它的
        // 那两道闸是同一组（`ToolTable::with_spawn` 的文档记了这个耦合）。
        // MCP 工具追加在最后（红线 11：builtin/shell/spawn/skills 的顺序是既有契约，
        // 只加不改；server 之间按 id、server 内按 tools/list，已在 `mcp::bootstrap` 排好）。
        // `with_status`（051）/ `with_collect`（053）紧跟在 `with_spawn` 之后、
        // skills/MCP 之前：这样「静态的那一段工具表」在所有会话里逐字节相同，
        // 不随装了几个 skill / 几个 MCP 工具而移位（红线 11）。
        //
        // 三个一起开：`background=true` 的 spawn 没有 collect 就是个陷阱——模型
        // 看得见后台这条路，却没有任何办法把结果拿回来，发出去的子全部在轮末被
        // 拆掉（`ToolTable::with_collect` 的文档记着这条）。
        ToolTable::standard_local()
            .with_spawn(session.agent_limits())
            .with_status()
            .with_collect()
            .with_skills(skills)
            .with_mcp(mcp.tools),
        vec![
            SystemChunk {
                label: Arc::from("base"),
                text: Arc::from("你是一个简洁、诚实的助手。"),
            },
            // 常驻 skill 索引（039）：跟工具表一样是稳定前缀的一部分，模型第一轮、
            // 激活之前就能发现有哪些 skill。空 registry → 空文本，被 system_text 滤掉。
            skill_index,
        ],
        SessionConfig {
            model: Arc::from(provider_cfg.model.as_str()),
            temperature: None,
            max_tokens: None,
            context_window: None,
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
    // 恢复之后必调——`persisted_seq` 这个同步水位不对齐，`persist::sync` 会把
    // `Session::restore` 灌回来的旧条目当新条目重新 append 一遍，连续几次
    // 「重启」之后 seq 会在文件中段跌回 0，下一次启动直接撞
    // `SeqNotIncreasing` 硬失败（独测抓到的真 bug 1，见
    // `agent_runtime::persist::seed_after_recover` 文档「真 bug」一节）。对全新
    // 会话是无害的空操作，不需要在这里分支判断「是不是恢复出来的」。
    agent_runtime::persist::seed_after_recover(&mut ctx, &session);
    if recovered_source_needs_fail_close {
        agent_runtime::cancel_pending_remote_tools(&mut session, &mut ctx);
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
