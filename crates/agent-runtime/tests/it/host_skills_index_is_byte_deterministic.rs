//! 064 §验收「红线 11：索引顺序」，**钉在两个落点上**（形状照 063 的
//! `host_tools_prefix_is_byte_deterministic.rs`，那里已经为工具表那一段立过范式）。
//!
//! 宿主声明的 skill 里，`description` 进**常驻索引**——每个 skill 一行 `id: 描述`，
//! 跟工具表一样是**随时都在的稳定前缀**的一部分，只不过它落在 **System 段**而不是
//! Tools 段。红线 11 那笔钱不是算在请求体上的，是算在**用来判「前缀有没有变」的那
//! 份镜像**上（`Encoded::prefix` 的 System 段）：请求体确定 ≠ 镜像确定，两条路一旦
//! 分叉，症状正是红线 11 的经典形态——功能完全正常，只是每一轮都全价。
//!
//! 所以每条都断言两处：wire 上那条 system 消息的正文，**和**镜像的 `bytes`/`hash`；
//! 外加 [`the_prefix_mirror_hashes_exactly_the_system_text_that_goes_on_the_wire`]
//! 把「两条路是不是同一份字节」本身变成一条会红的断言。
//!
//! # 会红的那一行
//!
//! `SkillRegistry.skills` 是 `BTreeMap`（红线 11 明文禁 `HashMap`）。**把它换成
//! `HashMap` 这两条当场红**：12 个 skill、两个各自 `new` 出来的 map，迭代顺序几乎
//! 必然不同（`RandomState` 每个实例一套键）。「打乱声明数组的顺序」那一半则是另一件
//! 事——客户端给的数组顺序不可靠（同一份声明两次连接可能不同序，
//! HOST-CAPABILITIES §六 第 2 条），它一个字节都不许漏进 prompt。

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::sync::{Arc, OnceLock};

use agent_core::{
    HostSkill, PrefixImage, RequestIntent, Segment, SegmentImage, SessionConfig, SkillId,
    SystemChunk,
};
use agent_providers::deepseek::DeepSeek;
use agent_providers::glm::Glm;
use agent_providers::kimi::Kimi;
use agent_providers::{Encoded, Ingredients, Provider};
use agent_runtime::SkillRegistry;
use serde_json::{Value, json};

/// 12 个 skill，**故意不按字典序给**。
///
/// 12 个而不是 2 个是为了突变验证能稳定复现：把 registry 的 `BTreeMap` 换成
/// `HashMap`，2 个元素有一半概率碰巧还是同序，12 个就不会。
const DECLARED: &[(&str, &str)] = &[
    ("zeta-flow", "最后一个流程"),
    ("crm-flow", "处理客户工单的标准流程"),
    ("alpha-flow", "第一个流程"),
    ("mail-flow", "发信流程"),
    ("billing-flow", "计费流程"),
    ("report-flow", "出报表"),
    ("audit-flow", "审计"),
    ("nine-flow", "第九个"),
    ("beta-flow", "第二个流程"),
    ("ops-flow", "运维流程"),
    ("kappa-flow", "第十个"),
    ("delta-flow", "第四个流程"),
];

fn skills(order: &[usize]) -> Vec<HostSkill> {
    order
        .iter()
        .map(|&i| {
            let (id, description) = DECLARED[i];
            HostSkill {
                id: SkillId::new(id),
                description: Arc::from(description),
                // 正文与自带工具**不该进索引**——放进来是为了让「索引只有一行摘要」
                // 这件事也被字节断言看住（正文漏进 System 段的话字节当场变）。
                body: Arc::from(format!("{id} 的正文，激活之后才该出现。")),
                tools: Vec::new(),
            }
        })
        .collect()
}

fn forward() -> Vec<usize> {
    (0..DECLARED.len()).collect()
}

fn reversed() -> Vec<usize> {
    (0..DECLARED.len()).rev().collect()
}

fn rotated(n: usize) -> Vec<usize> {
    let mut order = forward();
    order.rotate_left(n);
    order
}

/// 一份声明 → 这个会话的常驻索引段（`agent-server` 的 `actor::capabilities` 就是
/// 这么装的：`SkillRegistry::from_host_skills` → `skill_index_chunk` 追加进 system）。
fn index_chunk(order: &[usize]) -> SystemChunk {
    SkillRegistry::from_host_skills(skills(order)).skill_index_chunk()
}

fn providers() -> Vec<(&'static str, Box<dyn Provider>)> {
    vec![
        ("deepseek", Box::new(DeepSeek)),
        ("glm", Box::new(Glm)),
        ("kimi", Box::new(Kimi)),
    ]
}

fn config() -> &'static SessionConfig {
    static CONFIG: OnceLock<SessionConfig> = OnceLock::new();
    CONFIG.get_or_init(|| SessionConfig {
        model: Arc::from("determinism-fixture"),
        temperature: None,
        max_tokens: None,
        context_window: None,
    })
}

fn encode(provider: &dyn Provider, system: &[SystemChunk], prev: Option<&PrefixImage>) -> Encoded {
    provider.encode(&Ingredients {
        system,
        messages: &[],
        tools: &[],
        late_tools: &[],
        late_system: &[],
        config: config(),
        intent: RequestIntent::Free,
        prev_prefix: prev,
    })
}

/// 前缀镜像里 System 那一段。
fn system_segment(prefix: &PrefixImage) -> &SegmentImage {
    prefix
        .segments
        .iter()
        .find(|s| s.segment == Segment::System)
        .expect("镜像里该有 System 段")
}

/// 请求体里那条 `role: "system"` 消息的正文——**模型真正看到的那串字符**。
fn wire_system_text(enc: &Encoded) -> String {
    let body: Value = serde_json::from_slice(&enc.body).expect("请求体该是合法 JSON");
    let messages = body["messages"].as_array().expect("请求体里该有 messages");
    let system = messages
        .iter()
        .find(|m| m["role"] == json!("system"))
        .expect("该有一条 system 消息");
    system["content"]
        .as_str()
        .expect("system 消息该有文本正文")
        .to_string()
}

/// `agent-providers` 里 `wire::prefix::hash` 的同款复制（`DefaultHasher`，固定种子）。
/// 那个模块是 `pub(crate)`，跨 crate 拿不到；而「镜像和 wire 是不是同一份字节」这条
/// 只有把 wire 字节按同一个算法算一遍、跟 `SegmentImage.hash` 比才证得了。
fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    h.write(bytes);
    h.finish()
}

/// 第 1 条：同一份 skill 声明**建两次 registry、渲染两次**，索引字节完全相同。
///
/// 两次都重新 `from_host_skills` 一遍（而不是拿同一个 registry 渲染两次）——要看的
/// 正是「从声明到索引」这一步有没有夹带不确定性。
#[test]
fn the_same_declaration_renders_the_very_same_index_twice() {
    for (family, provider) in providers() {
        let one = index_chunk(&forward());
        let other = index_chunk(&forward());
        let (a, b) = (
            encode(&*provider, &[one], None),
            encode(&*provider, &[other], None),
        );

        assert_eq!(
            system_segment(&a.prefix),
            system_segment(&b.prefix),
            "{family}：同一份 skill 声明两次渲染，前缀镜像的 System 段不一样"
        );
        assert_eq!(
            wire_system_text(&a),
            wire_system_text(&b),
            "{family}：同一份 skill 声明两次渲染，wire 上的索引不一样"
        );
    }
}

/// 第 2 条：**打乱声明数组的顺序**再渲染，字节仍然完全相同。
///
/// 最后那条 `drift` 才是这条真正要拦的东西：镜像不一样 → 下一轮判前缀漂了 →
/// 整条前缀作废。宿主重连时把同一份声明按另一个顺序报上来，不该让这个会话每轮都
/// 付全价。
#[test]
fn shuffling_the_declaration_array_never_moves_a_byte_of_the_index() {
    for (family, provider) in providers() {
        let first = encode(&*provider, &[index_chunk(&forward())], None);

        for (label, order) in [("倒序", reversed()), ("轮转 5 位", rotated(5))] {
            let again = encode(&*provider, &[index_chunk(&order)], Some(&first.prefix));

            // 先断 `drift`：它是这条最贵的那一格——判漂了整条前缀作废，功能一切正常，
            // 只是每一轮都全价。后两条说的是同一件事的字节形态。
            assert_ne!(
                again.drift,
                Some(Segment::System),
                "{family}/{label}：同一份 skill 声明换个数组顺序就被判成前缀漂了——功能一切正常，只是每一轮都全价（红线 11）"
            );
            assert_eq!(
                system_segment(&again.prefix),
                system_segment(&first.prefix),
                "{family}/{label}：前缀镜像的 System 段跟着数组顺序变了"
            );
            assert_eq!(
                wire_system_text(&again),
                wire_system_text(&first),
                "{family}/{label}：客户端给的数组顺序漏进了 prompt 字节"
            );
        }
    }
}

/// 两条路是不是同一份字节：镜像哈希的，必须**正是**请求体里那一段。
///
/// 只比长度不够——「镜像那边把索引行倒过来」这种改法长度一个字节不差。
#[test]
fn the_prefix_mirror_hashes_exactly_the_system_text_that_goes_on_the_wire() {
    for (family, provider) in providers() {
        let encoded = encode(&*provider, &[index_chunk(&forward())], None);
        // 镜像那一段是 `canonical(&json!(system_text))`（三家 `encode.rs` 同形），
        // 也就是把整段正文当一个 JSON 字符串序列化。
        let wire =
            serde_json::to_vec(&json!(wire_system_text(&encoded))).expect("字符串序列化不会失败");
        let mirror = system_segment(&encoded.prefix);

        assert_eq!(
            mirror.bytes as usize,
            wire.len(),
            "{family}：镜像记的字节数跟 wire 上那一段对不上"
        );
        assert_eq!(
            mirror.hash,
            hash(&wire),
            "{family}：前缀镜像哈希的不是 wire 上那一段字节——两条路分叉了，请求体确定不代表缓存判定确定（红线 11）"
        );
    }
}

/// 索引那几行**原样钉死**：按 id 字典序、一行一个 `id: 描述`、**正文一个字都没有**。
///
/// 上面三条比的都是「两次相等」，一个「索引恒为空」的实现全都能过。这一条把内容
/// 本身钉住，两边合起来才是完整的。
#[test]
fn the_index_is_one_sorted_line_per_skill_and_carries_no_body() {
    let text = &*index_chunk(&reversed()).text;

    let mut expected: Vec<String> = DECLARED
        .iter()
        .map(|(id, d)| format!("{id}: {d}"))
        .collect();
    expected.sort();
    let lines: Vec<&str> = text.lines().skip(1).collect();
    assert_eq!(
        lines, expected,
        "索引该是按 id 排序、一行一个「id: 描述」（第一行是那句抬头）"
    );

    assert!(
        !text.contains("的正文"),
        "正文只在激活之后进 late_system，常驻索引里一个字都不该有：{text}"
    );
}
