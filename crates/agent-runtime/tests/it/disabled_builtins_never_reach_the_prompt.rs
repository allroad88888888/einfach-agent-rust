//! 076 §验收第 1、4、6 条，**钉在真正进 prompt 的那份字节上**（形状照 063 的
//! `host_tools_prefix_is_byte_deterministic.rs` / `host_tools_prefix_head_never_moves.rs`，
//! 夹具也是那一套）。
//!
//! 三条各自要拦的东西：
//!
//! 1. [`a_disabled_tool_is_absent_from_the_bytes_the_model_sees`]——「不启用」的定义
//!    是**连名字带描述都不进 prompt**。断在 wire 字节和前缀镜像两处：一个「渲染时才
//!    滤掉名字、描述还在」的实现，只查 `declares()` 是抓不住的。
//! 2. [`shuffling_the_switch_never_moves_a_byte`]——关闭列表的顺序/重复项不可以泄漏
//!    进 prompt（红线 11）。
//! 3. [`without_the_field_every_byte_is_what_it_was_before_076`]——**不带这个字段时
//!    工具表与 076 之前逐字节相同**。这条是整个 issue 的向后兼容承诺。
//!
//! # 关于第 2 条的诚实说明（免得下一个人以为它是白写的）
//!
//! prompt 那一侧对关闭列表的顺序**天生免疫**：剔除是集合运算
//! （`ToolTable::without_builtins` 用 `retain`，保住五档原有次序），所以删掉
//! `str_set` 里的 `sort()/dedup()` **不会**让这一条红——它红在
//! `agent-core/tests/disabled_builtins_indep_restore.rs::the_stored_bytes_do_not_
//! depend_on_input_order_or_duplicates`（落盘字节那一侧，也就是恢复时回放的东西）。
//!
//! 那这一条看住的是什么：**「集合运算」这个性质本身**。哪天有人把 `retain` 换成
//! 「按关闭列表逐个 `remove`/重建」之类顺序敏感的写法，这里立刻红。它是那个性质的
//! 看门狗，不是 `sort()` 的看门狗——两个落点各钉各的，不重复。

use std::sync::Arc;

use crate::host_tools_bytes_support::{
    assert_same_bytes, encode, hash, providers, text, tools_segment, wire_tools_bytes,
};
use agent_core::{AgentLimits, Segment};
use agent_runtime::ToolTable;

/// 一个装满的部署档：内置只读 + shell + 编排三件。
fn deployed() -> ToolTable {
    ToolTable::with_shell()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_collect()
}

fn off(list: &[&str]) -> Vec<Arc<str>> {
    list.iter().map(|n| Arc::from(*n)).collect()
}

/// 关掉的那批：一个编排工具 + 那个最该关的 shell。
const DISABLED: [&str; 2] = ["srv:agent/spawn", "srv:shell/exec"];

/// 验收第 1 条：关掉之后，那个工具的**名字和描述在进 prompt 的字节里一个都找不到**。
///
/// 三处断言缺一不可：
/// - `declares()` 为假——spawn 的截获闸（`crate::dispatch`）问的就是它；
/// - wire 字节里搜不到名字**也搜不到描述**——一个「只从 `specs()` 里摘掉名字」的
///   实现会让描述留在 prompt 里，那笔钱照付、而且模型仍然被那段文字影响；
/// - 没关的那些**还在**（正对照）——只断言「关掉的没了」的话，一个「把整张表清空」
///   的实现同样会绿。
#[test]
fn a_disabled_tool_is_absent_from_the_bytes_the_model_sees() {
    // 每个被关掉的工具两把钥匙：wire 上转义过的名字（050），以及**只在它自己的
    // 描述里出现**的一小段文字。后者才是这条真正要的——名字没了、描述还在，那笔钱
    // 照付，而且模型仍然被那段文字影响。
    const KEYS: [(&str, &str); 2] = [
        ("agent_2Fspawn", "交给一个新的子 agent"),
        ("shell_2Fexec", "sh -c"),
    ];

    for (family, provider) in providers() {
        let full = encode(&*provider, deployed().specs(), None);
        let full_bytes = text(wire_tools_bytes(&full));
        for (name, hint) in KEYS {
            assert!(
                full_bytes.contains(name) && full_bytes.contains(hint),
                "{family}：夹具前提——{name} 的名字与描述本来都在 prompt 里"
            );
        }

        let reduced = deployed().without_builtins(&off(&DISABLED));
        assert!(
            !reduced.declares("srv:agent/spawn"),
            "{family}：关掉之后 declares 必须为假（spawn 的截获闸问的就是它）"
        );
        assert!(
            !reduced.declares("srv:shell/exec"),
            "{family}：关掉之后 declares 必须为假"
        );

        let encoded = encode(&*provider, reduced.specs(), None);
        let bytes = text(wire_tools_bytes(&encoded));
        for (name, hint) in KEYS {
            assert!(
                !bytes.contains(name),
                "{family}：关掉的工具名 {name} 还在进 prompt 的字节里"
            );
            assert!(
                !bytes.contains(hint),
                "{family}：{name} 的名字没了、**描述还在**（「{hint}」）= 那笔钱照付，模型还被那段文字影响"
            );
        }

        // 正对照：没点名的那些一件不少（否则一个「把整张表清空」的实现同样会绿）。
        for name in [
            "fs_2Fread",
            "fs_2Flist",
            "agent_2Fstatus",
            "agent_2Fcollect",
        ] {
            assert!(
                bytes.contains(name),
                "{family}：没关的工具 {name} 被顺手带走了"
            );
        }
    }
}

/// 验收第 4 条（红线 11）：同一份关闭列表**打乱顺序、多写一个重复项**，进 prompt
/// 的字节完全相同、前缀镜像也不动。
///
/// 先断 `drift`：它是这条最贵的那一格——判漂了整条前缀作废，功能一切正常，只是每
/// 一轮都全价。
#[test]
fn shuffling_the_switch_never_moves_a_byte() {
    for (family, provider) in providers() {
        let first = {
            let table = deployed().without_builtins(&off(&DISABLED));
            encode(&*provider, table.specs(), None)
        };

        for (label, list) in [
            ("倒序", off(&["srv:shell/exec", "srv:agent/spawn"])),
            (
                "含重复项",
                off(&["srv:shell/exec", "srv:agent/spawn", "srv:shell/exec"]),
            ),
        ] {
            let table = deployed().without_builtins(&list);
            let again = encode(&*provider, table.specs(), Some(&first.prefix));

            assert_ne!(
                again.drift,
                Some(Segment::Tools),
                "{family}/{label}：同一份关闭列表换个写法就被判成前缀漂了——功能一切正常，只是每一轮都全价（红线 11）"
            );
            assert_eq!(
                tools_segment(&again.prefix),
                tools_segment(&first.prefix),
                "{family}/{label}：前缀镜像的 Tools 段跟着列表写法变了"
            );
            assert_same_bytes(
                &format!("{family}/{label}：关闭列表的写法漏进了 prompt 字节"),
                wire_tools_bytes(&first),
                wire_tools_bytes(&again),
            );
        }
    }
}

/// 验收第 6 条：**不带这个字段时，工具表与 076 之前逐字节相同**。
///
/// 「076 之前」的基线就是一张压根没调过 `without_builtins` 的表——同一档、同一条
/// 装配链。断言落在 wire 字节 + 前缀镜像的 `bytes`/`hash` 上：只比工具个数的话，
/// 一个「顺手重排了一下」的实现照样滑过去（063 那条「只比长度不够」的范式）。
#[test]
fn without_the_field_every_byte_is_what_it_was_before_076() {
    for (family, provider) in providers() {
        let before = encode(&*provider, deployed().specs(), None);
        let after = encode(
            &*provider,
            deployed().without_builtins(&[]).specs(),
            Some(&before.prefix),
        );

        assert_same_bytes(
            &format!(
                "{family}：空开关改了工具表的字节——不带这个字段的会话本该跟 076 之前一个字节不差"
            ),
            wire_tools_bytes(&before),
            wire_tools_bytes(&after),
        );
        assert_eq!(
            tools_segment(&after.prefix),
            tools_segment(&before.prefix),
            "{family}：空开关动了前缀镜像"
        );
        assert_ne!(
            after.drift,
            Some(Segment::Tools),
            "{family}：空开关把前缀判漂了（红线 11）"
        );
    }
}

/// 关掉表**尾巴**上那一件时，前面那一整段跟不关的会话**逐字节相同**（红线 11：
/// 既有顺序是契约，剔除不许顺带重排）。
///
/// 把 `without_builtins` 的 `retain` 换成「过滤 + 排序」或者「重建成另一种次序」，
/// 这条当场红——而 `declares()` 那一侧完全看不出来。
#[test]
fn dropping_the_tail_leaves_the_shared_head_byte_identical() {
    for (family, provider) in providers() {
        let base = encode(&*provider, deployed().specs(), None);
        let reduced = deployed().without_builtins(&off(&["srv:agent/collect"]));
        let cut = encode(&*provider, reduced.specs(), None);

        let (base_bytes, cut_bytes) = (wire_tools_bytes(&base), wire_tools_bytes(&cut));
        assert!(
            cut_bytes.len() < base_bytes.len(),
            "{family}：夹具白搭了，关掉之后字节反而没变少"
        );
        // 去掉收尾的 `]` 之后，剪短的那一段必须是原来那一段的**字节前缀**——
        // 缓存命中比的就是这个，逐项相同但项之间多了个空格照样全价。
        let head = &cut_bytes[..cut_bytes.len() - 1];
        assert!(
            base_bytes.starts_with(head),
            "{family}：关掉表尾那一件之后，前面共有的那一段不再是原来的字节前缀\n原：{}\n剪：{}",
            text(base_bytes),
            text(cut_bytes)
        );
        // 镜像那一侧记的必须正是 wire 上这一段（不然「请求体确定」推不出「缓存判定确定」）。
        assert_eq!(
            tools_segment(&cut.prefix).hash,
            hash(cut_bytes),
            "{family}：前缀镜像哈希的不是 wire 上那一段字节"
        );
    }
}
