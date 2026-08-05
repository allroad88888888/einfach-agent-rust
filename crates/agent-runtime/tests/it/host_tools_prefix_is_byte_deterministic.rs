//! 063 第 1、2 条 + §注意那条 schema key 序，**钉在前缀镜像那一侧**。
//!
//! 062 已经把 wire 那一侧钉过了（`agent-server/tests/http_capabilities_scoped_to_one_session.rs`
//! 断言假上游收到的请求体）。红线 11 那笔钱却不是算在 wire 上的：它算在**用来判
//! 「前缀有没有变」的那份镜像**上（`wire/prefix.rs` 的 `SegmentBytes.tools` →
//! `Encoded::prefix` 的 Tools 段）。请求体确定 ≠ 镜像确定；两条路一旦分叉，症状正是
//! 红线 11 的经典形态——功能完全正常，只是每一轮都全价。
//!
//! 所以这里每条都断言两处：wire 那一段原始字节，**和**镜像的 `bytes`/`hash`；
//! 外加 [`the_prefix_mirror_hashes_exactly_the_bytes_that_go_on_the_wire`] 把「两条路
//! 是不是同一份字节」本身变成一条会红的断言。
//!
//! **TODO(064)**：宿主声明的 skill 还没装配（`http/capabilities/assemble.rs` 只翻译顶层
//! `tools`，常驻索引与 skill 自带的工具是 064 的事），所以这里只覆盖工具表那一段。
//! 064 合入时照本文件的形状补一条：常驻索引进的是 **System** 段，落点是
//! `Encoded::prefix` 的 System 段 + 请求体里那条 system 消息。
//!
//! 今天它们确实是同一份：三家的 `encode` 都是 `let built = tools::build(..)` 一次，
//! 镜像拿 `canonical(&built.value)`、请求体拿 `built.value.clone()`
//! （`deepseek/encode.rs:58` 与 `:112`，glm/kimi 同形）。**这条「今天成立」不是白拿的
//! 结论，是需要被看住的性质**——哪天有人为了给镜像加个什么处理而另起一次
//! `tools::build`，上面那条 tie 就是唯一会红的地方。

mod host_tools_bytes_support;

use agent_core::Segment;
use host_tools_bytes_support::{
    DECLARED, assert_same_bytes, encode, hash, items, providers, reversed, rotated, table_with,
    text, tools_segment, wire_tools_bytes,
};

/// 第 1 条：同一份声明**装两次表、渲染两次**，字节完全相同。
///
/// 两次都重新 `with_host_tools` 一遍（而不是拿同一张表 encode 两次）——要看的正是
/// 「从声明到表」这一步有没有夹带不确定性（比如哪天有人把注入表换成 `HashMap`）。
#[test]
fn the_same_declaration_renders_the_very_same_bytes_twice() {
    for (family, provider) in providers() {
        let one = table_with(DECLARED);
        let other = table_with(DECLARED);
        let (a, b) = (
            encode(&*provider, one.specs(), None),
            encode(&*provider, other.specs(), None),
        );

        assert_eq!(
            tools_segment(&a.prefix),
            tools_segment(&b.prefix),
            "{family}：同一份声明两次渲染，前缀镜像的 Tools 段不一样"
        );
        assert_same_bytes(
            &format!("{family}：同一份声明两次渲染，wire 上的工具段"),
            wire_tools_bytes(&a),
            wire_tools_bytes(&b),
        );
    }
}

/// 第 2 条：**打乱声明数组的顺序**再渲染，字节仍然完全相同。
///
/// 最后那条 `drift` 才是这条真正要拦的东西：镜像不一样 → 下一轮判前缀漂了 →
/// 整条前缀作废。宿主重连时把同一份声明按另一个顺序报上来（客户端数组顺序不可靠，
/// HOST-CAPABILITIES §六 第 2 条），不该让这个会话每轮都付全价。
///
/// 删掉 `tool_table_host.rs` 里那行 `sort_by` 这条就红。
#[test]
fn shuffling_the_declaration_array_never_moves_a_byte() {
    for (family, provider) in providers() {
        let first = {
            let table = table_with(DECLARED);
            encode(&*provider, table.specs(), None)
        };

        for (label, decl) in [
            ("倒序", reversed(DECLARED)),
            ("轮转 5 位", rotated(DECLARED, 5)),
        ] {
            let table = table_with(&decl);
            let again = encode(&*provider, table.specs(), Some(&first.prefix));

            // 先断 `drift`：它是这条最贵的那一格——判漂了整条前缀作废，功能一切正常，
            // 只是每一轮都全价。后两条说的是同一件事的字节形态。
            assert_ne!(
                again.drift,
                Some(Segment::Tools),
                "{family}/{label}：同一份声明换个数组顺序就被判成前缀漂了——功能一切正常，只是每一轮都全价（红线 11）"
            );
            assert_eq!(
                tools_segment(&again.prefix),
                tools_segment(&first.prefix),
                "{family}/{label}：前缀镜像的 Tools 段跟着数组顺序变了"
            );
            assert_same_bytes(
                &format!("{family}/{label}：客户端给的数组顺序漏进了 prompt 字节"),
                wire_tools_bytes(&first),
                wire_tools_bytes(&again),
            );
        }
    }
}

/// 两条路是不是同一份字节：镜像哈希的，必须**正是**请求体里那一段。
///
/// 只比长度不够——「镜像那边把数组倒过来」这种改法长度一个字节不差。
#[test]
fn the_prefix_mirror_hashes_exactly_the_bytes_that_go_on_the_wire() {
    for (family, provider) in providers() {
        let table = table_with(DECLARED);
        let encoded = encode(&*provider, table.specs(), None);
        let wire = wire_tools_bytes(&encoded);
        let mirror = tools_segment(&encoded.prefix);

        assert_eq!(
            mirror.bytes as usize,
            wire.len(),
            "{family}：镜像记的字节数跟 wire 上那一段对不上"
        );
        assert_eq!(
            mirror.hash,
            hash(wire),
            "{family}：前缀镜像哈希的不是 wire 上那一段字节——两条路分叉了，请求体确定不代表缓存判定确定（红线 11）"
        );
    }
}

/// 一条声明的 schema，key 序在**文本里**换一换，渲染出来的字节不变（063 §注意：
/// `serde_json::Map` 是 `BTreeMap` 所以天然确定——**但别假设，写断言**；
/// `agent-core/src/value/tool.rs` 有同款先例）。
///
/// 这里不只比「两次相等」，还把那一项的字节**原样钉死**：key 全部按字典序排
/// （`description` < `name` < `parameters`、`properties` < `type`）。根 `Cargo.toml`
/// 一旦给 `serde_json` 开了 `preserve_order`，`Map` 就换成 `IndexMap`、跟着插入顺序
/// 走，这条当场红。
#[test]
fn the_key_order_inside_a_declared_schema_never_reaches_the_bytes() {
    const ORDER_A: &str = r#"[{ "name": "web:crm/lookup", "description": "查档案",
        "schema": { "type": "object", "properties": { "id": { "type": "string" }, "since": { "type": "number" } } } }]"#;
    const ORDER_B: &str = r#"[{ "name": "web:crm/lookup", "description": "查档案",
        "schema": { "properties": { "since": { "type": "number" }, "id": { "type": "string" } }, "type": "object" } }]"#;
    const EXPECTED: &str = concat!(
        r#"{"function":{"description":"查档案","name":"web_3Acrm_2Flookup","#,
        r#""parameters":{"properties":{"id":{"type":"string"},"since":{"type":"number"}},"type":"object"}},"#,
        r#""type":"function"}"#
    );

    for (family, provider) in providers() {
        let a = {
            let table = table_with(ORDER_A);
            encode(&*provider, table.specs(), None)
        };
        let table = table_with(ORDER_B);
        let b = encode(&*provider, table.specs(), Some(&a.prefix));

        let declared = items(wire_tools_bytes(&a))
            .pop()
            .expect("注入的那一项在表尾");
        assert_eq!(
            text(&declared),
            EXPECTED,
            "{family}：声明渲染出来的字节不是按 key 字典序排的"
        );
        assert_eq!(
            tools_segment(&b.prefix),
            tools_segment(&a.prefix),
            "{family}：schema 的 key 序变了前缀镜像"
        );
        assert_same_bytes(
            &format!("{family}：schema 的 key 序漏进了 prompt 字节"),
            wire_tools_bytes(&a),
            wire_tools_bytes(&b),
        );
        assert_ne!(
            b.drift,
            Some(Segment::Tools),
            "{family}：同一份 schema 换个 key 序就被判前缀漂了（红线 11）"
        );
    }
}
