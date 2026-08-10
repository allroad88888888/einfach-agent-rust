//! 053 的单元面：入参解析、拒绝文案的可见性边界（红线 10），以及
//! **红线 6 在 collect 绑定这一侧的门**——绑定记的必须是「那次 collect 调用的
//! 世代」，不是回写那一刻的当前世代。
//!
//! e2e 那一面（父真的被挡住、真的恢复、孤儿收尾不再触发）在 `tests/collect_*`。

use std::sync::Arc;

use agent_core::{
    AgentId, ChildConfig, ContentBlock, Epoch, Event, PrefixImage, Session, SessionConfig,
    StopReason, TokenUsage, ToolCallId, TurnStatus,
};
use agent_providers::deepseek::DeepSeek;
use agent_tools::ToolExecutor;
use agent_transport::Client;
use serde_json::json;

use super::*;
use crate::tool_table::ToolTable;

// ---------- 入参 ----------

/// `id` 是**必填**（跟 `status` 的可选 `id` 不同）：领哪个是有后果的选择，
/// 替模型猜一个是最不该省的那种省事。
#[test]
fn id_is_required_and_must_be_a_non_empty_string() {
    assert!(parse(&json!({})).is_err());
    assert!(parse(&json!({ "id": null })).is_err());
    assert!(parse(&json!({ "id": "   " })).is_err());
    assert!(parse(&json!({ "id": 7 })).is_err());
    assert_eq!(
        parse(&json!({ "id": " root/a1 " })).unwrap(),
        AgentId::new("root/a1")
    );
}

// ---------- 工具说明书（071） ----------

/// 描述里那几句话，逐条对着 [`intercept`] 的三条出路：只领后台开的子、一份结果
/// 只能领一次、这一轮没领就没了，以及跟 `status` 的配合（先看谁 Done 再领谁）。
///
/// 071 核对下来这段本来就是对的（053 写的时候 `collect` 和 `background` 已经同时
/// 存在），这条测试是**把它钉住**：三个工具的描述互相引用，改任何一个都可能让
/// 另一个变成假话。
///
/// **断关键子串不断整段**：文案会改，行为不该改。另外两个工具名走
/// [`crate::STATUS_TOOL`] 常量，改名而描述没跟上一样红。
#[test]
fn the_spec_states_the_facts_the_model_cannot_guess() {
    let spec = collect_spec();
    let text = &*spec.description;
    assert_eq!(&*spec.name, COLLECT_TOOL);
    assert!(text.contains("background=true"), "它只领后台开的子：{text}");
    assert!(
        text.contains("只能领一次"),
        "领取即消费（`take_stashed` 的 remove）：{text}"
    );
    assert!(
        text.contains("拆掉"),
        "轮末没领的下场（`orphan::reap`）：{text}"
    );
    assert!(
        text.contains(crate::STATUS_TOOL),
        "先 status 再 collect 的配合：{text}"
    );
    assert_eq!(spec.schema["required"], json!(["id"]));
}

// ---------- 拒绝文案：红线 10 由构造保证 ----------

/// 拒绝文本里那句「你现在能领的是」**只列调用者的后代**。一句好心的提示把兄弟
/// 子树的 agent id 漏给模型，就是一次横读（红线 10）——而它不会让任何东西报错。
#[test]
fn a_refusal_never_names_agents_outside_the_callers_subtree() {
    let mut subtree = Subtree::default();
    let epoch = Epoch::START;
    subtree.detach(AgentId::new("root/a1/b1"), AgentId::new("root/a1"), epoch);
    subtree.detach(AgentId::new("root/a2/b9"), AgentId::new("root/a2"), epoch);

    let caller = AgentId::new("root/a1");
    let text = not_collectable(&AgentId::new("root/a1/b7"), &caller, &subtree);
    assert!(
        text.contains("root/a1/b1"),
        "该告诉它自己那棵子树里能领的：{text}"
    );
    assert!(
        !text.contains("root/a2/b9"),
        "兄弟那棵子树的 id 一个字都不该露出来：{text}"
    );

    // 拒绝理由本身当然会点名它要领的那个 id；不该露出来的是**清单**那一半。
    let text = not_a_descendant(&caller, &AgentId::new("root/a2/b9"), &subtree);
    let listed = text.split("你现在能领的是：").nth(1).unwrap_or("");
    assert!(
        !listed.contains("root/a2/b9"),
        "清单里不该有兄弟子树的 id：{text}"
    );
    assert!(
        listed.contains("root/a1/b1"),
        "同样要告诉它自己能领哪些：{text}"
    );
}

/// 一个都没有时也有话说（跟 `status_tool::you_can_see` 同款）。
#[test]
fn with_nothing_to_collect_the_refusal_says_so() {
    let subtree = Subtree::default();
    let caller = AgentId::root();
    let text = not_collectable(&AgentId::new("root/a1"), &caller, &subtree);
    assert!(text.contains("没有等着领"), "{text}");
}

// ---------- 领取即消费 ----------

/// stash 里那份结果被端走之后就不在了：第二次 collect 同一个 id 会落到
/// 「领不了」那条路上（`is_error`），而不是把同一份答案发两遍。
#[test]
fn a_stashed_result_can_only_be_taken_once() {
    let (session, child, spawned_at) = finished_background_child();
    let mut ctx = build_ctx();
    let mut subtree = Subtree::default();
    subtree.detach(child.clone(), AgentId::root(), spawned_at);
    assert!(
        subtree.harvest(&session, &mut ctx).is_empty(),
        "后台子不回写父，只进 stash"
    );

    assert!(subtree.take_stashed(&child).is_some(), "第一次该领得到");
    assert!(
        subtree.take_stashed(&child).is_none(),
        "第二次该空手——领取即消费"
    );
    assert!(
        subtree.collectable().is_empty(),
        "领完两张表都该干净（轮末不该再报「没人领」）"
    );
}

// ---------- 红线 6：collect 绑定的世代 ----------

/// **红线 6。** collect 绑定期间世代被推走（取消/undo 那一下）→ 子落终态时的回写
/// 带的是**绑定那一刻**的世代，撞 `Session::step` 入口的闸被丢，父那个 collect 槽
/// **不被幽灵结果填**。
///
/// 推世代用的是一个跟本案无关的**诱饵子 agent**：`Cancel` 会 bump 世代，而取消
/// root 会顺手清空它的槽位、取消被 collect 的那个子会改掉它的终态——两者都会让
/// 下面那条断言因为别的原因而绿。取消诱饵则**只**动世代，root 和被领的子一个
/// 字节没变，于是「结果没落地」就只可能是闸干的。
///
/// 孪生对照在下一条：同一份脚本、同一次收割，只是不推世代，结果就老老实实落进
/// 父的历史。把 `harvest_slots` 里的 `epoch: slot.epoch` 改成 `session.epoch()`
/// （= 用现在的世代交差、绕过闸），这一条立刻红。
#[test]
fn a_collect_binding_write_back_is_dropped_when_the_epoch_moved_on() {
    let (mut session, child, decoy, call_id) = parked_collect();
    let mut ctx = build_ctx();
    let mut subtree = bind_collect(&session, &child, &call_id);

    // 推世代：取消那个诱饵。root 仍在 ToolsPending，child 仍在 Thinking。
    let bound_at = session.epoch();
    let _ = session.step(Event::Cancel { agent: decoy });
    assert_ne!(
        session.epoch(),
        bound_at,
        "取消该推走世代，否则这条测试是空跑的"
    );
    assert_eq!(
        session.status(),
        TurnStatus::ToolsPending,
        "root 不该被这次取消碰到"
    );

    finish(&mut session, &child);
    let mut events = subtree.harvest(&session, &mut ctx);

    // 收割**确实产出了**那条回写（否则下面「没落地」对一个根本没发生的回写也成立）。
    assert_eq!(events.len(), 1, "子落终态该产出一条回写：{events:#?}");
    let event = events.remove(0);
    assert_eq!(
        event.epoch(),
        Some(bound_at),
        "回写该带绑定那一刻的世代，不是现在的"
    );

    let effects = session.step(event);
    assert!(effects.is_empty(), "过期世代的回写该被闸整条丢掉（红线 6）");
    assert_eq!(
        session.status(),
        TurnStatus::ToolsPending,
        "collect 槽该还空着等"
    );
    assert!(
        !root_saw("ANSWERCOLLECT", &session),
        "幽灵结果填了 collect 槽（红线 6）"
    );
}

/// 上一条的孪生：**不**推世代，同一次收割该老老实实落进父的历史、父的槽收敛。
#[test]
fn and_the_very_same_write_back_lands_when_the_epoch_still_matches() {
    let (mut session, child, _decoy, call_id) = parked_collect();
    let mut ctx = build_ctx();
    let mut subtree = bind_collect(&session, &child, &call_id);

    finish(&mut session, &child);
    let mut events = subtree.harvest(&session, &mut ctx);

    assert_eq!(events.len(), 1);
    let effects = session.step(events.remove(0));
    assert!(!effects.is_empty(), "槽收敛该让父接着发下一跳");
    assert!(
        root_saw("ANSWERCOLLECT", &session),
        "世代没变时该落地（否则上一条是空跑的）"
    );
    assert!(
        subtree.take_stash().is_empty(),
        "领到的结果不该再进一次 stash（轮末会误报「没人领」）"
    );
    assert!(
        subtree.take_orphans(&session).is_empty(),
        "领完的子不该再被当孤儿"
    );
}

// ---------- 夹具 ----------

/// 一个停在「root 等一次 collect、子还在跑、外加一个诱饵子」的会话。
///
/// root 的 `ToolsPending` 是真的：它那条 `ToolUse` 经转移表进去，槽位由 core 记，
/// 不是给平结构字段赋值造出来的。
fn parked_collect() -> (Session, AgentId, AgentId, ToolCallId) {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let _ = session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("拆一个后台的"),
    });

    let child = session.spawn_child(&root, ChildConfig::default()).unwrap();
    let _ = session.step(Event::UserInput {
        agent: child.clone(),
        text: Arc::from("BGTASK"),
    });
    let decoy = session.spawn_child(&root, ChildConfig::default()).unwrap();
    let _ = session.step(Event::UserInput {
        agent: decoy.clone(),
        text: Arc::from("DECOY"),
    });

    let call_id = ToolCallId::new("call_collect");
    let _ = session.step(Event::ProviderDone {
        agent: root,
        epoch: session.epoch(),
        blocks: vec![ContentBlock::ToolUse {
            id: call_id.clone(),
            name: Arc::from(COLLECT_TOOL),
            input: Arc::new(json!({ "id": child.as_str() })),
        }],
        stop: StopReason::ToolUse,
        usage: usage(),
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    });
    assert_eq!(
        session.status(),
        TurnStatus::ToolsPending,
        "root 该停在等 collect 的槽上"
    );
    (session, child, decoy, call_id)
}

/// 后台子 + 一次 collect 绑定（`intercept` 的第二条路做的那两笔账）。
fn bind_collect(session: &Session, child: &AgentId, call_id: &ToolCallId) -> Subtree {
    let mut subtree = Subtree::default();
    let root = AgentId::root();
    subtree.detach(child.clone(), root.clone(), session.epoch());
    subtree.record(
        child.clone(),
        root,
        call_id.clone(),
        session.epoch(),
        COLLECT_TOOL,
    );
    subtree
}

/// 子答完收工。
fn finish(session: &mut Session, child: &AgentId) {
    let _ = session.step(Event::ProviderDone {
        agent: child.clone(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::Text(Arc::from("ANSWERCOLLECT 后台子的答案"))],
        stop: StopReason::EndTurn,
        usage: usage(),
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    });
    assert_eq!(
        session.status_of(child),
        TurnStatus::Done { truncated: false }
    );
}

/// 一个 root 底下挂着一个已经答完的后台子的会话（`Epoch` 是 spawn 那一刻的）。
fn finished_background_child() -> (Session, AgentId, Epoch) {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let _ = session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("拆一个"),
    });
    let spawned_at = session.epoch();
    let child = session.spawn_child(&root, ChildConfig::default()).unwrap();
    let _ = session.step(Event::UserInput {
        agent: child.clone(),
        text: Arc::from("BGTASK"),
    });
    finish(&mut session, &child);
    (session, child, spawned_at)
}

fn root_saw(needle: &str, session: &Session) -> bool {
    session.messages_of(&AgentId::root()).iter().any(|m| {
        m.blocks.iter().any(|b| match b {
            ContentBlock::ToolResult { content, .. } => content.contains(needle),
            ContentBlock::Text(t) => t.contains(needle),
            _ => false,
        })
    })
}

fn usage() -> TokenUsage {
    TokenUsage {
        prompt: 10,
        completion: 5,
        cached: None,
    }
}

/// 收割要一个 `RunnerCtx` 发通报。这里一次网络都不会打（`harvest` 只调 `ctx.emit`），
/// 端点/密钥是占位（照 `ctx_tests.rs` 的同款装配）。
fn build_ctx() -> RunnerCtx {
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "http://127.0.0.1:1/chat/completions".to_string(),
        "fake-key".to_string(),
        ToolExecutor::new(std::env::temp_dir()).unwrap(),
        ToolTable::builtin(),
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
