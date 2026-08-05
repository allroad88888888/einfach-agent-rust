//! 验收 2：`CallProvider` 是薄的（决策 15 的实检，见
//! docs/issues/001-loop-contract.md「实做记录」）。序列化成 JSON 后，
//! 内层字段 key 集合恰为 `{agent, epoch}`——出现 `payload`/`body`/`request`
//! 任何一个都算失败，因为那意味着 core 又想自己组装请求了。

mod support;

use std::collections::BTreeSet;

use agent_core::{Effect, Epoch};

#[test]
fn call_provider_json_keys_are_exactly_agent_and_epoch() {
    let effect = Effect::CallProvider {
        agent: support::agent(),
        epoch: Epoch::START,
    };
    let json = serde_json::to_value(&effect).expect("序列化不应失败");

    // Effect 是外部打标签（externally tagged）的枚举：顶层只有一个 key
    // "CallProvider"，值才是携带的字段。
    let inner = json
        .get("CallProvider")
        .unwrap_or_else(|| panic!("期望顶层 key 为 \"CallProvider\"，实际 json={json}"))
        .as_object()
        .expect("CallProvider 的负载应是一个 JSON object");

    let keys: BTreeSet<&str> = inner.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = ["agent", "epoch"].into_iter().collect();

    assert_eq!(
        keys, expected,
        "CallProvider 必须薄：core 只说「该调了」，请求由 adapter 组装（决策 15）"
    );

    for forbidden in ["payload", "body", "request"] {
        assert!(
            !inner.contains_key(forbidden),
            "CallProvider 不该出现 `{forbidden}` 字段——那意味着 core 又在替 adapter 组装请求了"
        );
    }
}
