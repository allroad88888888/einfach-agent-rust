//! 148 的**装配落位**单测：一包经装配入口进来之后，两半边各自到位没有。
//!
//! 各道闸被撞时的行为（前缀强制的 release 语义、只装一半、包内重名）在姊妹文件
//! `tool_table_extension_guard_tests.rs`；共用夹具在
//! `tool_table_extension_fixtures.rs`。
//!
//! # 这里够不着的那一格：模型脚本化调一次
//!
//! 「模型发 ToolCall → dispatch 查表 → 闭包跑 → tool_result 进下一轮 prompt」需要
//! 一个假 SSE 服务器（`tests/it/support`），单测里没有。**它也不需要在这里重证**：
//! 146 的独测已经把「`register_session_tool` 注册的名字被模型调到、结果回到下一轮
//! 请求体」这条链跑通，而装配的 ctx 半边（`PendingInterceptors::install`）落地到的
//! 就是同一个 `register_session_tool`。所以这里钉的是接缝这一侧的判据：
//! **`declares()` 为真 + `session_tool_registered()` 为真**——正是 `dispatch` 把一次
//! 模型调用路由进截获表所依据的那两件事。整包经真实 `run_turn` 那一条留给
//! `tests/it/` 的独测。

use std::sync::Arc;

use agent_core::{AgentId, ChildConfig, Event, Location, Reversibility, Session};
use serde_json::Value;

use super::fixtures::{
    HOOK_SENTINEL, HOOK_TOOL, PACK, READ_SENTINEL, READ_TOOL, WRITE_TOOL, build_ctx, log, nop_tool,
    spec, test_pack, tree_echo,
};
use super::*;
use crate::tool_table::CallTiming;
use crate::turn_end;

/// 表半边（spec 进模型面、可逆性进映射、timed 进 timed 区）与 ctx 半边（截获注册）
/// 各自落位，且 `TurnEnd` 钩子经**真实驱动** [`turn_end::fire`] 真的被调到。
#[test]
fn a_pack_lands_its_specs_reversibility_timed_hooks_and_intercepts() {
    let log = log();
    let (tools, pending) = ToolTable::builtin().with_extension(test_pack(Arc::clone(&log)));

    // 表半边。
    assert!(tools.declares(READ_TOOL), "截获工具要进模型面");
    assert_eq!(
        &*tools.specs().last().unwrap().name,
        READ_TOOL,
        "追加在表尾：前面那段所有会话共有的字节一个都不动（红线 11）"
    );
    assert!(
        !tools.declares(HOOK_TOOL),
        "timed 钩子不进模型面（133：specs/declares 一个字节看不见 timed 区）"
    );
    assert!(tools.declares_timed(HOOK_TOOL), "它该在 timed 区");
    assert_eq!(tools.timed(CallTiming::TurnEnd).count(), 1);
    let request = tools.snapshot(READ_TOOL, Arc::new(Value::Null));
    assert_eq!(request.reversibility, Reversibility::Pure, "包声明的那个值");
    assert_eq!(request.location, Location::Server, "`ext:` 在本进程里跑");

    // ctx 半边。
    let mut ctx = build_ctx(tools);
    assert!(
        !ctx.session_tool_registered(READ_TOOL),
        "install 之前只有表半边——这正是防呆要挡的中间态"
    );
    pending.install(&mut ctx);
    assert!(
        ctx.session_tool_registered(READ_TOOL),
        "install 之后 dispatch 才会把这个名字路由进截获表"
    );

    // TurnEnd 钩子：走 136 的真实驱动，不是在这里手工调执行体。
    turn_end::fire(&ctx);
    assert_eq!(*log.lock().unwrap(), vec![HOOK_SENTINEL]);
}

/// 包里那条纯读工具的**函数体**：真的读了 `Session` 的状态，且只数调用者的后代
/// （红线 10）——root 看得见那个子，子看不见 root。
#[test]
fn the_packs_read_tool_narrows_to_the_callers_subtree() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let _ = session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("拆一个"),
    });
    let child = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();

    let from_root = tree_echo(&mut session, &root, &Value::Null).unwrap();
    assert_eq!(&*from_root, format!("{READ_SENTINEL} descendants=1"));

    let from_child = tree_echo(&mut session, &child, &Value::Null).unwrap();
    assert_eq!(
        &*from_child,
        format!("{READ_SENTINEL} descendants=0"),
        "子看不见 root，也看不见兄弟（红线 10）"
    );
}

/// 同一条装配链，装包与不装包：装了的那张只在**尾部**追加，前面那段（所有会话
/// 共有的前缀）逐字节没动；不装包的那张则连一处新代码路径都不经过——没有截获
/// 注册，`fire` 也一条钩子都调不到。
#[test]
fn a_session_without_the_pack_is_byte_for_byte_what_it_was() {
    let plain = ToolTable::with_shell().with_status();
    let baseline = serde_json::to_string(plain.specs()).unwrap();

    let log = log();
    let (packed, pending) = ToolTable::with_shell()
        .with_status()
        .with_extension(test_pack(Arc::clone(&log)));
    let head = serde_json::to_string(&packed.specs()[..plain.specs().len()]).unwrap();
    assert_eq!(head, baseline, "装了包也只许在尾部追加（红线 11）");
    assert_eq!(packed.specs().len(), plain.specs().len() + 1);
    pending.install(&mut build_ctx(packed));

    let plain_ctx = build_ctx(plain);
    assert!(!plain_ctx.session_tool_registered(READ_TOOL));
    turn_end::fire(&plain_ctx);
    assert!(
        log.lock().unwrap().is_empty(),
        "没装包的会话不该有任何钩子被调到"
    );
}

/// 声明的值直接就是 `snapshot` 的答案。`Pure` 那条是真判据：名字规则对 `ext:`
/// 一律落 `Irreversible`（`tool_table_names.rs` 的看门狗），所以它只可能来自包的
/// 声明经注入映射查到——这条一红就说明那一级被绕过去了，症状会是「`/undo` 白白
/// 多问一句」，或者反过来更糟。
#[test]
fn the_declared_reversibility_is_the_answer_not_the_name_rule() {
    let pack = ExtensionPack::new(PACK)
        .with_tool(spec(READ_TOOL, "纯读"), Reversibility::Pure, nop_tool())
        .with_tool(
            spec(WRITE_TOOL, "会写东西"),
            Reversibility::Irreversible,
            nop_tool(),
        );
    let (tools, pending) = ToolTable::builtin().with_extension(pack);

    assert_eq!(
        tools.snapshot(READ_TOOL, Arc::new(Value::Null)).reversibility,
        Reversibility::Pure
    );
    assert_eq!(
        tools
            .snapshot(WRITE_TOOL, Arc::new(Value::Null))
            .reversibility,
        Reversibility::Irreversible,
        "屏障照常登记：/undo 撞上它会停下来问"
    );

    pending.install(&mut build_ctx(tools));
}
