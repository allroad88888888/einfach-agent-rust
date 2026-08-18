//! 独立测试：只依据 `docs/issues/148-extension-pack-seam.md`「验收」「注意」两节 +
//! `docs/EXTENSIONS.md`（接缝文档，对外契约）+ 委派任务给定的公开签名写成，**不看**
//! `crates/agent-runtime/src/` 下任何实现体。实现由另一个 agent 并行落地，本文件与它
//! 互不通信；委派时状态是「已全绿」，这里只钉住契约，不重新发现实现。
//!
//! # 被测契约（委派任务原文，一字不差）
//!
//! ```ignore
//! use agent_runtime::{Aftermath, ExtensionPack, PendingInterceptors, ToolTable, CallTiming, SessionToolFn, TimedRun};
//! ExtensionPack::new(name).with_tool(spec, SessionToolFn).with_timed(spec, CallTiming, TimedRun).name();
//! ToolTable::with_extension(self, pack) -> (ToolTable, PendingInterceptors);
//! PendingInterceptors::install(self, &mut ctx);
//! ```
//!
//! # 201 改了其中一条契约（决策 199 §一）
//!
//! `with_tool` 的第二个位置参数（`Reversibility`）删了，可逆性改由执行体返回的
//! [`Aftermath`] 逐次交代。下面「验收 2」那两条因此换了判据：**不是**「声明 Pure /
//! 声明 Irreversible」，而是「交 `Nothing` / 交 `Irreversible`」——被测的行为
//! （不挡 undo / 挡住直到 `/undo!`）一个字节没变，这正是这次换法该有的样子。
//! 「交 `Undo(f)` 之后 `/undo` 真的把外部世界收拾了」是 201 自己的验收，住
//! `ext_undo_fn_delivery.rs`。
//!
//! 复杂文件豁免（>300 行、≤500 行）：七条测试是同一个接缝契约的五个不重叠角度
//! （见下），拆开会把「测试对应哪条验收」这条映射打散；落点被委派任务锁定为
//! 「唯二两个文件」（本文件 + `main.rs` 加一行），拆共享 helper 模块不在允许改动
//! 范围内——跟同批次 136 独立测试 `turn_end_indep.rs` 同一个取舍、同一条理由。
//!
//! # 七条测试对应「要覆盖」五条
//!
//! 1. [`a_scripted_pack_call_reaches_the_next_prompt_and_the_turn_end_hook_fires_once`]：
//!    装一个 pack（纯读截获工具 + 一个 TurnEnd hook）→ 脚本化模型调用截获工具 →
//!    下一轮请求体含哨兵；轮跑完 → hook 计数 +1。
//! 2. [`an_ext_tool_that_touched_nothing_leaves_no_barrier_and_undo_crosses_it_cleanly`] /
//!    [`an_irreversible_ext_tool_leaves_a_barrier_that_stops_undo_until_forced`]：
//!    交 `Nothing` 不挡 undo；交 `Irreversible` 挡住 undo，`/undo!` 才越过（照抄
//!    `mcp_undo_barrier.rs`/`shell_exec_undo_barrier.rs` 的既有屏障测试手法；
//!    201 之前这两条的判据是注册时声明的 `Reversibility`）。
//! 3. [`a_pack_installed_but_never_called_only_appends_bytes_after_the_existing_table_tail`]：
//!    不装 pack 的会话与装了但模型从未调用的会话，第一轮请求体里 `tools` 段的共有
//!    表头逐字节相同——pack 只在表尾追加（红线 11，手法照抄
//!    `host_tools_prefix_head_never_moves.rs` 的「整段字节前缀」判据）。
//! 4. [`a_pack_with_a_bare_name_tool_panics_at_assembly_in_debug_builds`]（debug 半）/
//!    [`a_bare_name_tool_is_dropped_alone_in_release_the_legit_sibling_still_works`]
//!    （release 半，`#[cfg(not(debug_assertions))]` 门住——debug 构建下这条会在
//!    `with_extension` 那一步就 panic，测不到「丢一条不丢包」这个 release 专属语义；
//!    这跟 `call_timing_indep.rs`/`intercept_registry_indep_guards.rs` 用
//!    `#[cfg(debug_assertions)]` 门住撞名 panic 测试是同一个取舍的镜像）：裸名条目
//!    在装配期被单独丢弃，同包里合法的条目不受连累。
//! 5. [`extension_tool_still_works_after_snapshot_recovery_reinstalls_the_pack`]：
//!    装 pack 的会话跑一轮、快照恢复后，宿主重新 `with_extension` + `install` 一遍，
//!    截获工具仍然能调通（手法照抄 `jsonl_restart_continues.rs` 的「进程 1／进程 2」
//!    两段式重建）。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_core::{AgentId, ContentBlock, Session, ToolSpec, TurnStatus, UndoReport};
use agent_providers::wire_name;
use agent_runtime::{
    Aftermath, CallTiming, ExtensionPack, SessionToolFn, TimedRun, ToolTable, run_turn,
};
use serde_json::{Value, json};

use crate::support;

const ECHO_TOOL: &str = "ext:demo/echo";
const ECHO_SENTINEL: &str = "EXT-PACK-ECHO-SENTINEL-4c1a9f";
const HOOK_TOOL: &str = "ext:demo/on-turn-end";
const UNUSED_TOOL: &str = "ext:demo/never-called";
const BARRIER_TOOL: &str = "ext:barrier/act";
const BARRIER_SENTINEL: &str = "EXT-PACK-BARRIER-SENTINEL-9d2f61";
const RELWIT_SENTINEL: &str = "EXT-PACK-RELWIT-SENTINEL-e83a07";

/// 一个最小合法 `ToolSpec`：schema 是空 object，够 `declares()` 判真就行——这些
/// 测试从不真的按 schema 校验入参（照 `intercept_registry_indep_support::spec` 的
/// 既有先例）。
fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 什么都不做、总是 `Ok` 的截获执行体。交 `Aftermath::Nothing`：它连外部世界的
/// 边都没碰过。
fn ok_session_fn() -> SessionToolFn {
    Box::new(|_session: &mut Session, _agent: &AgentId, _input: &Value| {
        Ok((Arc::from("ok"), Aftermath::Nothing))
    })
}

/// 忽略入参、原样回哨兵串的截获执行体——闭包每次调用都现造一个新 `Box`
/// （`SessionToolFn` 不是 `Clone`）。`aftermath` 是每次调用现造的那一份交代
/// （`Aftermath` 也不是 `Clone`：它里面可能装着一个 `FnOnce`）。
fn sentinel_session_fn(sentinel: &'static str, aftermath: fn() -> Aftermath) -> SessionToolFn {
    Box::new(
        move |_session: &mut Session, _agent: &AgentId, _input: &Value| {
            Ok((Arc::from(sentinel), aftermath()))
        },
    )
}

/// 每次调用给共享计数器 +1 的 timed 执行体。
fn counting_turn_end_run(counter: Arc<AtomicUsize>) -> TimedRun {
    Box::new(
        move |_table: &ToolTable,
              _session: &Session,
              _input: &Value|
              -> Result<Arc<str>, Arc<str>> {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::from("ok"))
        },
    )
}

/// 某个 agent 历史里 `call_id` 那一次调用的结果：`(正文, is_error)`——照
/// `intercept_registry_indep_support::tool_result` 的既有先例，按 call_id 取，
/// 不按顺序取。
fn tool_result(session: &Session, agent: &AgentId, call_id: &str) -> (String, bool) {
    session
        .messages_of(agent)
        .iter()
        .flat_map(|m| m.blocks.iter())
        .find_map(|block| match block {
            ContentBlock::ToolResult {
                id,
                content,
                is_error,
            } if &*id.0 == call_id => Some((content.to_string(), *is_error)),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "{} 的历史里没有 call_id={call_id} 的 tool_result",
                agent.as_str()
            )
        })
}

/// 请求体里 `"tools":` 后面那一个 JSON 值的原始字节——手法照抄
/// `host_tools_bytes_support::wire_tools_bytes`，只是直接在捕获到的请求体文本上
/// 操作，不经 `Provider::encode` 那条更底层的路。
fn tools_array_bytes(body: &str) -> &[u8] {
    const KEY: &str = "\"tools\":";
    let body_bytes = body.as_bytes();
    let at = body
        .find(KEY)
        .unwrap_or_else(|| panic!("请求体里没有 tools 段：{body}"));
    let start = at + KEY.len();
    let mut stream =
        serde_json::Deserializer::from_slice(&body_bytes[start..]).into_iter::<Value>();
    let value = stream
        .next()
        .expect("tools 后面该有一个值")
        .expect("该是合法 JSON");
    assert!(value.is_array(), "定位到的 tools 段不是数组：{body}");
    &body_bytes[start..start + stream.byte_offset()]
}

/// 验收 1：一纯读截获工具 + 一个 TurnEnd hook 装进同一个 pack。
#[test]
fn a_scripted_pack_call_reaches_the_next_prompt_and_the_turn_end_hook_fires_once() {
    let dir = support::temp_dir("extension-pack-echo-and-hook");
    let wire = wire_name::to_wire(ECHO_TOOL);
    let (port, bodies) = support::spawn_recording_server(vec![
        support::sse_tool_call("call_1", &wire, "{}"),
        support::sse_text("done reading the echo"),
    ]);

    let hook_calls = Arc::new(AtomicUsize::new(0));
    let pack = ExtensionPack::new("demo")
        .with_tool(
            spec(ECHO_TOOL, "纯读截获工具：回哨兵串"),
            sentinel_session_fn(ECHO_SENTINEL, || Aftermath::Nothing),
        )
        .with_timed(
            spec(HOOK_TOOL, "每轮收尾计数一次"),
            CallTiming::TurnEnd,
            counting_turn_end_run(Arc::clone(&hook_calls)),
        );
    let (tools, pending) = ToolTable::builtin().with_extension(pack);

    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);
    pending.install(&mut ctx);

    let mut session = Session::new(AgentId::root());
    let status = run_turn(&mut session, &mut ctx, "call the demo echo tool")
        .expect("scripted turn should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "两跳：工具调用一跳 + 收尾文本一跳");
    assert!(
        bodies[1].contains(ECHO_SENTINEL),
        "下一轮请求体该带上 ext 工具 tool_result 里的哨兵串：{}",
        bodies[1]
    );

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        1,
        "完成一整轮之后，pack 里的 TurnEnd hook 该恰好被调一次"
    );
}

/// 验收 2 上半（201 换判据）：交 `Aftermath::Nothing` 的 ext 工具跑完之后，
/// `/undo` 干净越过，不撞屏障。
#[test]
fn an_ext_tool_that_touched_nothing_leaves_no_barrier_and_undo_crosses_it_cleanly() {
    let dir = support::temp_dir("extension-pack-barrier-pure");
    let wire = wire_name::to_wire(BARRIER_TOOL);
    let pack = ExtensionPack::new("barrier").with_tool(
        spec(BARRIER_TOOL, "交 Nothing 的截获工具"),
        sentinel_session_fn(BARRIER_SENTINEL, || Aftermath::Nothing),
    );
    let (tools, pending) = ToolTable::builtin().with_extension(pack);
    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call("call_1", &wire, "{}"),
        support::sse_text("done"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);
    pending.install(&mut ctx);

    let mut session = Session::new(AgentId::root());
    let status = run_turn(&mut session, &mut ctx, "call the pure ext tool")
        .expect("pure turn should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let report = session.undo_turn();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "交 Nothing 该干净越过：{report:?}"
    );
    assert!(session.messages().is_empty(), "越过之后这一轮该整个退掉");
}

/// 验收 2 下半（201 换判据）：交 `Aftermath::Irreversible` 的 ext 工具跑完之后，
/// `/undo` 撞屏障停下（推 `UndoReport::Blocked`），`/undo!` 才越过。
#[test]
fn an_irreversible_ext_tool_leaves_a_barrier_that_stops_undo_until_forced() {
    let dir = support::temp_dir("extension-pack-barrier-irreversible");
    let wire = wire_name::to_wire(BARRIER_TOOL);
    let pack = ExtensionPack::new("barrier").with_tool(
        spec(BARRIER_TOOL, "交 Irreversible 的截获工具"),
        sentinel_session_fn(BARRIER_SENTINEL, || Aftermath::Irreversible),
    );
    let (tools, pending) = ToolTable::builtin().with_extension(pack);
    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call("call_1", &wire, "{}"),
        support::sse_text("done"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);
    pending.install(&mut ctx);

    let mut session = Session::new(AgentId::root());
    let status = run_turn(&mut session, &mut ctx, "call the irreversible ext tool")
        .expect("irreversible turn should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let report = session.undo_turn();
    let UndoReport::Blocked { barrier_seq, .. } = report else {
        panic!("交 Irreversible 该撞屏障停下，拿到 {report:?}");
    };
    let barrier_entry = session
        .history()
        .entries()
        .find(|e| e.seq == barrier_seq)
        .unwrap();
    assert_eq!(
        barrier_entry.meta.undoability,
        agent_core::Undoability::Blocked,
        "撞停的这条 entry 该是屏障"
    );

    let report = session.undo_turn_force();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "强制越过该成功：{report:?}"
    );
    assert!(session.messages().is_empty(), "越过之后这一轮该整个退掉");
}

/// 验收 3（红线 11 看门狗）：不装 pack 的会话 vs 装了但模型从未调用的会话——第一轮
/// 请求体里 `tools` 段的共有表头逐字节相同，pack 只在表尾追加。
#[test]
fn a_pack_installed_but_never_called_only_appends_bytes_after_the_existing_table_tail() {
    let dir_base = support::temp_dir("extension-pack-prefix-base");
    let dir_pack = support::temp_dir("extension-pack-prefix-pack");

    let (port_base, bodies_base) =
        support::spawn_recording_server(vec![support::sse_text("plain reply, no ext tools")]);
    let (mut ctx_base, _events_base) = support::build_ctx(port_base, &dir_base);
    let mut session_base = Session::new(AgentId::root());
    let status_base = run_turn(&mut session_base, &mut ctx_base, "hello")
        .expect("baseline turn should not be a source failure");
    assert_eq!(status_base, TurnStatus::Done { truncated: false });

    let (port_pack, bodies_pack) =
        support::spawn_recording_server(vec![support::sse_text("plain reply, no ext tools")]);
    let pack = ExtensionPack::new("demo").with_tool(
        spec(UNUSED_TOOL, "从不被模型调用的截获工具，只用来证明表尾追加"),
        ok_session_fn(),
    );
    let (tools, pending) = ToolTable::builtin().with_extension(pack);
    let (mut ctx_pack, _events_pack) = support::build_ctx_with(port_pack, &dir_pack, tools);
    pending.install(&mut ctx_pack);
    let mut session_pack = Session::new(AgentId::root());
    let status_pack = run_turn(&mut session_pack, &mut ctx_pack, "hello")
        .expect("packed turn should not be a source failure");
    assert_eq!(status_pack, TurnStatus::Done { truncated: false });

    let bodies_base = bodies_base.lock().unwrap();
    let bodies_pack = bodies_pack.lock().unwrap();
    let tools_base = tools_array_bytes(&bodies_base[0]);
    let tools_full = tools_array_bytes(&bodies_pack[0]);

    assert!(
        tools_full.len() > tools_base.len(),
        "装了 pack 之后 tools 段该变长，不然这个夹具没测到东西"
    );
    // 去掉基线收尾的 `]`：装了 pack 之后的表该原样把这一段当前缀，长出来的字节
    // 全部在表尾。
    let head = &tools_base[..tools_base.len() - 1];
    assert!(
        tools_full.starts_with(head),
        "红线 11：不装/装了但未调用两边共有的表头必须是字节前缀\n基线：{}\n装包：{}",
        String::from_utf8_lossy(tools_base),
        String::from_utf8_lossy(tools_full),
    );
}

/// 验收 4 debug 半：pack 里一条裸名（没有 `ext:` 前缀）的工具，`with_extension`
/// 装配期在 debug 构建下当场 `debug_assert!` panic。
#[cfg(debug_assertions)]
#[test]
#[should_panic]
fn a_pack_with_a_bare_name_tool_panics_at_assembly_in_debug_builds() {
    let pack = ExtensionPack::new("relwit")
        .with_tool(
            spec("bare-name-no-prefix", "裸名，没有 ext: 前缀"),
            ok_session_fn(),
        )
        .with_tool(
            spec("ext:relwit/legit", "合法名字"),
            sentinel_session_fn(RELWIT_SENTINEL, || Aftermath::Nothing),
        );
    let _ = ToolTable::builtin().with_extension(pack);
}

/// 验收 4 release 半（`#[cfg(not(debug_assertions))]` 门住，理由见文件头注释）：
/// 裸名条目在装配期被单独丢弃——不进表、也不进任何执行路径，同包里合法的条目
/// 照常可调。裸名条目之后走既有 `unknown_tool` 路，与 146 独立测试
/// `intercept_registry_indep_guards.rs` 的「未声明名字」判据同一套。
#[cfg(not(debug_assertions))]
#[test]
fn a_bare_name_tool_is_dropped_alone_in_release_the_legit_sibling_still_works() {
    let dir = support::temp_dir("extension-pack-release-drop-one");
    let bare_wire = wire_name::to_wire("bare-name-no-prefix");
    let legit_wire = wire_name::to_wire("ext:relwit/legit");

    let pack = ExtensionPack::new("relwit")
        .with_tool(
            spec("bare-name-no-prefix", "裸名，没有 ext: 前缀"),
            ok_session_fn(),
        )
        .with_tool(
            spec("ext:relwit/legit", "合法名字，不该被裸名连累"),
            sentinel_session_fn(RELWIT_SENTINEL, || Aftermath::Nothing),
        );
    let (tools, pending) = ToolTable::builtin().with_extension(pack);
    assert!(
        !tools.declares("bare-name-no-prefix"),
        "release 下裸名条目该被丢掉，不进表"
    );
    assert!(
        tools.declares("ext:relwit/legit"),
        "同包里合法的条目不该被裸名连累一起丢掉"
    );

    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call("call_bare", &bare_wire, "{}"),
        support::sse_tool_call("call_legit", &legit_wire, "{}"),
        support::sse_text("both attempts done"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);
    pending.install(&mut ctx);

    let mut session = Session::new(AgentId::root());
    let status = run_turn(&mut session, &mut ctx, "call both tools")
        .expect("release run should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let (bare_result, bare_is_error) = tool_result(&session, &AgentId::root(), "call_bare");
    assert!(bare_is_error, "裸名条目该走 unknown_tool：{bare_result}");
    let (legit_result, legit_is_error) = tool_result(&session, &AgentId::root(), "call_legit");
    assert!(!legit_is_error, "合法条目不该受裸名连累：{legit_result}");
    assert!(legit_result.contains(RELWIT_SENTINEL), "{legit_result}");
}

/// 验收 5：装 pack 的会话跑一轮、快照恢复后，宿主重新 `with_extension` + `install`
/// 一遍，`ext:demo/echo` 仍然能调通——表按装配重建、注册照旧。
#[test]
fn extension_tool_still_works_after_snapshot_recovery_reinstalls_the_pack() {
    let dir = support::temp_dir("extension-pack-recovery");
    let session_path = dir.join("session.jsonl");
    let wire = wire_name::to_wire(ECHO_TOOL);

    fn echo_pack() -> ExtensionPack {
        ExtensionPack::new("demo").with_tool(
            spec(ECHO_TOOL, "恢复前后都该能调的截获工具"),
            sentinel_session_fn(ECHO_SENTINEL, || Aftermath::Nothing),
        )
    }

    // ---- 「进程 1」：装 pack，调一次，落盘，drop 掉 ctx。----
    {
        let port = support::spawn_scripted_server(vec![
            support::sse_tool_call("call_1", &wire, "{}"),
            support::sse_text("first round done"),
        ]);
        let (tools, pending) = ToolTable::builtin().with_extension(echo_pack());
        let (mut ctx, _events) =
            support::build_ctx_with_store(port, &dir, tools, Some(session_path.clone()));
        pending.install(&mut ctx);
        let mut session = Session::new(AgentId::root());
        let status = run_turn(&mut session, &mut ctx, "call the recovery-bound echo tool")
            .expect("first turn should not be a source failure");
        assert_eq!(status, TurnStatus::Done { truncated: false });
        agent_runtime::persist::maybe_snapshot(&mut ctx, &session);
        // ctx（连同它的 Jsonl）在这个块结束时 drop。
    }

    // ---- 「进程 2」：全新 backend 指向同一路径，load 回来。----
    let backend = agent_runtime::open_backend(Some(session_path.clone()), |e| {
        panic!("不该有加载错误：{e}")
    });
    let mut recovered = agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        agent_core::AgentLimits::default(),
        &mut |k| panic!("不该有不认识的键：{k:?}"),
    )
    .unwrap()
    .expect("写过一轮，该恢复出 Some");

    // ---- 宿主的恢复路径：重新 with_extension + install，不是从磁盘长回来的。----
    let port2 = support::spawn_scripted_server(vec![
        support::sse_tool_call("call_2", &wire, "{}"),
        support::sse_text("second round done"),
    ]);
    let (tools2, pending2) = ToolTable::builtin().with_extension(echo_pack());
    let (mut ctx2, _events2) =
        support::build_ctx_with_store(port2, &dir, tools2, Some(session_path.clone()));
    pending2.install(&mut ctx2);

    recovered.begin_turn();
    agent_runtime::persist::sync(&mut ctx2, &mut recovered);
    let status = run_turn(&mut recovered, &mut ctx2, "call it again after recovery")
        .expect("post-recovery turn should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let (result_text, is_error) = tool_result(&recovered, &AgentId::root(), "call_2");
    assert!(!is_error, "恢复之后再调 ext 工具不该失败：{result_text}");
    assert!(
        result_text.contains(ECHO_SENTINEL),
        "该拿到闭包的真实返回值，不是 unknown_tool：{result_text}"
    );
}
