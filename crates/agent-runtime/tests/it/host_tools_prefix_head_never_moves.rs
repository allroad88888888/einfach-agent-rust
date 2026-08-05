//! 063 第 3 条：**注入排在表尾、前缀不动**——拿一个不带声明的会话的工具表做基线，
//! 带声明那个的前 N 项与基线**逐项字节相同**。
//!
//! 落点跟同批另一个文件一样是**前缀镜像那一侧**（wire 那一侧 062 的 e2e 已经断过）：
//! 所有会话共有的那一段不因为某个客户端注入了东西而移位，说的就是缓存前缀。
//! 逐项相同之外还多断一条**整段字节前缀**——缓存命中比的是字节，不是「项」。
//!
//! 把 `tool_table_host.rs` 里的 `self.specs.push(spec)` 改成 `insert(0, spec)`
//! （或者把 `with_host_tools` 挪到装配链的前面）这条就红。

mod host_tools_bytes_support;

use agent_providers::wire_name;
use host_tools_bytes_support::{
    DECLARED, assert_same_bytes, baseline_table, encode, items, providers, table_with, text,
    tools_segment, wire_tools_bytes,
};
use serde_json::Value;

#[test]
fn an_injection_only_appends_and_the_shared_head_stays_byte_identical() {
    for (family, provider) in providers() {
        let (baseline, injected) = (baseline_table(), table_with(DECLARED));
        let base = encode(&*provider, baseline.specs(), None);
        let full = encode(&*provider, injected.specs(), None);
        let (base_bytes, full_bytes) = (wire_tools_bytes(&base), wire_tools_bytes(&full));
        let (base_items, full_items) = (items(base_bytes), items(full_bytes));

        assert!(
            full_items.len() > base_items.len(),
            "{family}：注入之后工具反而没变多，夹具白搭了（{} vs {}）",
            base_items.len(),
            full_items.len()
        );

        // ── 逐项字节相同：共有的那一段一项都不许动。
        for (n, (only, both)) in base_items.iter().zip(&full_items).enumerate() {
            assert_same_bytes(&format!("{family}：共有那一段的第 {n} 项被注入挤动了"), only, both);
        }

        // ── 比逐项更硬的一条：基线整段（去掉收尾的 `]`）必须是注入那一段的**字节前缀**。
        // 缓存命中比的就是这个——逐项相同但项之间多了个空格照样全价。
        let head = &base_bytes[..base_bytes.len() - 1];
        assert!(
            full_bytes.starts_with(head),
            "{family}：基线那一段不是注入之后的字节前缀\n基线：{}\n注入：{}",
            text(head),
            text(full_bytes)
        );

        // ── 镜像那一侧：Tools 段只在尾部长了，长出来的字节数跟 wire 上完全对得上
        //（镜像自己短了/长了都说明它算的不是同一份东西）。
        assert_eq!(
            u64::from(tools_segment(&full.prefix).bytes) - u64::from(tools_segment(&base.prefix).bytes),
            (full_bytes.len() - base_bytes.len()) as u64,
            "{family}：前缀镜像 Tools 段的增量跟 wire 上的增量对不上"
        );

        // ── 表尾那一段自己按名字排序（客户端给的顺序进不来，HOST-CAPABILITIES §六）。
        let tail: Vec<String> = full_items[base_items.len()..].iter().map(|item| declared_name(item)).collect();
        let mut sorted = tail.clone();
        sorted.sort();
        assert_eq!(tail, sorted, "{family}：注入的那一段没按名字排序");
    }
}

/// 一项工具的字节 → 它的工具全名。wire 上的 `function.name` 是转义过的（050），
/// 用 provider 自己那把解码器还原，免得把转义规则在测试里抄一遍。
fn declared_name(item: &[u8]) -> String {
    let parsed: Value = serde_json::from_slice(item).expect("每一项都是合法 JSON");
    wire_name::from_wire(parsed["function"]["name"].as_str().expect("每一项都有 function.name")).to_string()
}
