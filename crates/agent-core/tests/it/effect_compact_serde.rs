//! 105 独立测试：`Effect::Compact` / `Event::CompactDone` / `Event::CompactFailed`
//! 的**形状**——序列化往返、`Compact` 带 `epoch`、序列化出来的 key 白名单。
//!
//! 这是这个 crate 之外、不依赖 `effect.rs` 内部 `#[cfg(test)]` 单元测试的独立验证：
//! 独立测试的意义就在于不借用实现自带的测试。行为契约（epoch 闸怎么丢/怎么不丢）
//! 在 `effect_compact_epoch_gate.rs`，这里只管「序列化对不对、payload 有没有偷偷
//! 带正文」（红线 3）。
//!
//! 全程零网络：只有 `serde_json` 往返，没有任何 provider/工具调用。

use std::sync::Arc;

use agent_core::{AgentId, Effect, Epoch, Event, Notice, ToolCallId, TurnStatus};

/// 五个 `Effect` 变体全部 serde 往返——`Compact` 是 105 加上的第五个。
#[test]
fn roundtrip_all_five_effect_variants() {
    let effects = vec![
        Effect::CallProvider {
            agent: AgentId::root(),
            epoch: Epoch(1),
        },
        Effect::ExecuteTool {
            agent: AgentId::root(),
            call_id: ToolCallId::new("call_1"),
            tool: Arc::from("srv:fs/read"),
            input: Arc::new(serde_json::json!({"path": "/tmp/a"})),
            epoch: Epoch(1),
        },
        Effect::Compact {
            agent: AgentId::root(),
            upto: 12,
            epoch: Epoch(4),
        },
        Effect::CancelInFlight { epoch: Epoch(2) },
        Effect::Emit(Notice::TurnStatusChanged {
            status: TurnStatus::Done { truncated: false },
        }),
    ];

    let s = serde_json::to_string(&effects).unwrap();
    assert_eq!(serde_json::from_str::<Vec<Effect>>(&s).unwrap(), effects);
}

/// `Compact` 单独往返一次，附带字段值校验——不仅仅是「打包进 Vec 能还原」，
/// 而是 `epoch` 这个红线 6 的凭证本身没有在路上被打混。
#[test]
fn compact_effect_roundtrips_and_keeps_its_epoch() {
    let effect = Effect::Compact {
        agent: AgentId::root(),
        upto: 30,
        epoch: Epoch(5),
    };

    let s = serde_json::to_string(&effect).unwrap();
    let back: Effect = serde_json::from_str(&s).unwrap();
    assert_eq!(back, effect);

    match back {
        Effect::Compact { epoch, upto, .. } => {
            assert_eq!(epoch, Epoch(5));
            assert_eq!(upto, 30);
        }
        other => panic!("期待 Compact，收到 {other:?}"),
    }
}

/// `Compact` 序列化出来的 key 只有 `agent`/`upto`/`epoch` 三个——跟 `effect.rs`
/// 里 `call_provider_carries_no_payload` 同款的白名单断言。多一个 key（比如
/// 历史正文）就是决策 15 的精神被违反：要摘要哪一段由 `upto` 表达，正文由宿主
/// 自己从状态取，effect 不该跟着变胖。
#[test]
fn compact_carries_no_history_payload() {
    let json = serde_json::to_value(Effect::Compact {
        agent: AgentId::root(),
        upto: 7,
        epoch: Epoch(0),
    })
    .unwrap();
    let fields = json["Compact"].as_object().unwrap();
    let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["agent", "epoch", "upto"]);
}

/// 两个新 `Event` 变体各自 serde 往返：`CompactDone` 带 `summary` 正文（它是
/// 进来的事件，不是 primitive，红线 3 之外的约束不适用——只要能序列化就行），
/// `CompactFailed` 完全不带业务字段。
#[test]
fn compact_done_and_failed_events_roundtrip() {
    let events = vec![
        Event::CompactDone {
            agent: AgentId::root(),
            summary: Arc::from("这是一段摘要正文"),
            epoch: Epoch(3),
        },
        Event::CompactFailed {
            agent: AgentId::root(),
            epoch: Epoch(3),
        },
    ];

    let s = serde_json::to_string(&events).unwrap();
    assert_eq!(serde_json::from_str::<Vec<Event>>(&s).unwrap(), events);
}

/// 两个新变体都要过 epoch 闸（红线 6）：`Event::epoch()` 对它们必须返回
/// `Some`，`Event::agent()` 必须原样带出路由用的 agent——穷举 `match` 加变体时
/// 编译器会逼这两个提取器回答，这里钉住它们确实答对了。
#[test]
fn compact_events_are_epoch_bearing_and_routed() {
    let epoch = Epoch(9);
    let agent = AgentId::root();

    let done = Event::CompactDone {
        agent: agent.clone(),
        summary: Arc::from("x"),
        epoch,
    };
    let failed = Event::CompactFailed {
        agent: agent.clone(),
        epoch,
    };

    assert_eq!(done.epoch(), Some(epoch), "CompactDone 必须带 epoch");
    assert_eq!(failed.epoch(), Some(epoch), "CompactFailed 必须带 epoch");
    assert_eq!(done.agent(), &agent);
    assert_eq!(failed.agent(), &agent);
}
