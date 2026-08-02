//! 前缀镜像、漂移判定、命中预测——缓存兜底第 1/2 层的**输入**在这里算，三家
//! 共用。段序 `[Tools][System][History]` 是三家实测的渲染顺序（PROVIDERS.md
//! §一），core 只存这个镜像、只做减法，判读全在这里（红线 12）。
//!
//! 判定逻辑统一走**仅扩展**语义：新请求的 Tools/System 段要跟上一轮逐字节
//! 相同，History 段要是上一轮 History 的严格延长，块粒度向下取整。
//!
//! - 对仅扩展匹配的家（DeepSeek/Kimi）这是精确模型。
//! - 对真前缀树匹配的家（GLM）这是**保守低估**：真实命中可能更高（改写中段
//!   也能保住一部分前缀），但我们没有服务端才有的信息去建模真前缀树，而
//!   「好于预期」不需要告警——所以直接复用同一套仅扩展逻辑，块粒度按各家
//!   自己的 `block` 参数传入即可，不需要为 GLM 单独分支。
//!
//! 块粒度 `block` 由各家在 `encode` 里传入（DeepSeek 128 / Kimi 256 / GLM 64，
//! 常量定义在各自的 `mod.rs`）。

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;

use agent_core::{PrefixImage, Segment, SegmentImage};
use serde_json::Value;

/// 三段各自的规范序列化字节。`encode` 算出来，这里只管比。
pub struct SegmentBytes {
    pub tools: Vec<u8>,
    pub system: Vec<u8>,
    pub history: Vec<u8>,
}

/// History 段的规范字节：**逐条消息拼接，不套成一个 JSON 数组**。
///
/// 套数组的话末尾那个 `]` 会让「在末尾追加一条消息」不再是字节级的延长
/// （`…}]` 变成 `…},{`），前缀比对当场判漂——而 provider 那边其实是满命中的。
/// 这个 bug 不报错、不影响回答，只让第 1 层每轮都误报、第 2 层的预测永远是 0，
/// 正是红线 11 那一类「只在账单上浮出来」的错。
pub fn concat(items: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for item in items {
        serde_json::to_writer(&mut out, item).expect("Value 序列化不会失败");
    }
    out
}

impl SegmentBytes {
    fn of(&self, seg: Segment) -> &[u8] {
        match seg {
            Segment::Tools => &self.tools,
            Segment::System => &self.system,
            Segment::History => &self.history,
        }
    }
}

/// 哈希**只做相等比对，不做安全用途**——不抗碰撞攻击，也不进任何鉴权路径。
/// 用 `DefaultHasher`（固定种子，同一二进制内确定）而不是 `RandomState`：
/// 镜像要跨请求比较，随机种子会让每次都判成漂了。
/// 换 Rust 版本时算法可能变，那会让升级后的第一轮多报一次漂——偏保守的一侧。
pub fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    h.write(bytes);
    h.finish()
}

pub fn image(bytes: &SegmentBytes) -> PrefixImage {
    PrefixImage {
        segments: [Segment::Tools, Segment::System, Segment::History]
            .into_iter()
            .map(|segment| {
                let raw = bytes.of(segment);
                SegmentImage {
                    segment,
                    bytes: u32::try_from(raw.len()).unwrap_or(u32::MAX),
                    hash: hash(raw),
                }
            })
            .collect(),
        // 这一轮实际花了多少 prompt token 要等响应，宿主拿到 usage 后回填。
        prompt_tokens: None,
    }
}

/// 返回「哪一段漂了」和「这次该命中多少 token」。
///
/// - 冷启动（没有上一轮）：不算漂——没有可比的东西，报 `Some` 会让第 1 层
///   每次开会话都告警；预测 0，因为确实一个块都不该命中。
/// - Tools / System 变了：这两段在前缀最前面，动了后面全废 → 预测 0。
/// - History：必须是上一轮 History 的**严格延长**（前 `prev.bytes` 字节逐字节
///   相同）才算没漂。变短、或前缀对不上，都是漂。
/// - 预测值只用**实测的 `prompt_tokens`**：字节数换算不出 token 数。上一轮没
///   回填过就预测 0——没实测值宁可失明，不可瞎猜（瞎猜会让第 2 层误报）。
///
/// `block`：命中数总是这个数的整数倍向下取整（DeepSeek 128、Kimi 256、GLM
/// 64，实测夹逼出来的公约数，PROVIDERS.md §一）。
///
/// `min_predict`：**低于它就不预测**（返回 0，第 2 层视为「无预测不判」）。
/// 给有实测「零区」的家用：GLM 在 ~460 token 完全不缓存、~860 才起效——
/// 零区里按块取整出一个正数是确定地错（真实两轮实测：predicted=448 /
/// actual=0，第 2 层误报）。门槛带内宁可失明，不可瞎猜。
/// 没有零区的家传 0。这个值是模型数据，只能在 adapter 侧——第 2 层在 core，
/// 红线 12 禁止它知道任何一家的门槛。
pub fn compare(
    bytes: &SegmentBytes,
    prev: Option<&PrefixImage>,
    block: u32,
    min_predict: u32,
) -> (Option<Segment>, u32) {
    let Some(prev) = prev else {
        return (None, 0);
    };

    for segment in [Segment::Tools, Segment::System] {
        match prev.segments.iter().find(|s| s.segment == segment) {
            Some(p) if same(bytes.of(segment), p) => {}
            // 上一轮的镜像缺这一段 = 比不了，按漂处理（保守：预测归零）。
            _ => return (Some(segment), 0),
        }
    }

    let Some(p) = prev.segments.iter().find(|s| s.segment == Segment::History) else {
        return (Some(Segment::History), 0);
    };
    let head = p.bytes as usize;
    let now = bytes.of(Segment::History);
    if head > now.len() || hash(&now[..head]) != p.hash {
        return (Some(Segment::History), 0);
    }

    let tokens = prev.prompt_tokens.unwrap_or(0);
    if tokens < min_predict {
        return (None, 0); // 零区/门槛带内：不预测，不是预测 0 个块
    }
    (None, tokens / block * block)
}

fn same(now: &[u8], prev: &SegmentImage) -> bool {
    now.len() == prev.bytes as usize && hash(now) == prev.hash
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: u32 = 128;

    fn bytes(tools: &str, system: &str, history: &str) -> SegmentBytes {
        SegmentBytes {
            tools: tools.into(),
            system: system.into(),
            history: history.into(),
        }
    }

    #[test]
    fn image_is_three_segments_in_render_order() {
        let img = image(&bytes("[]", "sys", "hist"));
        let segs: Vec<Segment> = img.segments.iter().map(|s| s.segment).collect();
        assert_eq!(segs, vec![Segment::Tools, Segment::System, Segment::History]);
        assert_eq!(img.segments[1].bytes, 3);
        assert_eq!(img.prompt_tokens, None);
    }

    #[test]
    fn cold_start_is_not_drift() {
        assert_eq!(compare(&bytes("[]", "s", "h"), None, BLOCK, 0), (None, 0));
    }

    /// 严格延长：不漂，预测 = 上一轮 prompt_tokens 按块粒度向下取整
    /// （cache-prefix.json：434 → 384，块 128）。
    #[test]
    fn strict_extension_predicts_floor_of_prev_prompt() {
        let mut prev = image(&bytes("[]", "sys", "abc"));
        prev.prompt_tokens = Some(434);
        assert_eq!(compare(&bytes("[]", "sys", "abcdef"), Some(&prev), BLOCK, 0), (None, 384));
    }

    /// GLM 的零区：门槛 860 之下不预测（真实两轮实测过 predicted=448/actual=0 的误报）。
    #[test]
    fn below_min_predict_returns_zero_not_a_rounded_guess() {
        let mut prev = image(&bytes("[]", "sys", "abc"));
        prev.prompt_tokens = Some(461);
        assert_eq!(compare(&bytes("[]", "sys", "abcdef"), Some(&prev), 64, 860), (None, 0));
        // 到了门槛就正常按块取整。
        prev.prompt_tokens = Some(900);
        assert_eq!(compare(&bytes("[]", "sys", "abcdef"), Some(&prev), 64, 860), (None, 896));
    }

    /// 换一个块粒度（Kimi 256）预测值跟着变。
    #[test]
    fn different_block_size_changes_prediction() {
        let mut prev = image(&bytes("[]", "sys", "abc"));
        prev.prompt_tokens = Some(700);
        assert_eq!(compare(&bytes("[]", "sys", "abcdef"), Some(&prev), 256, 0), (None, 512));
    }

    /// 上一轮没回填 prompt_tokens → 预测 0，不拿字节数瞎折算。
    #[test]
    fn no_measured_prompt_tokens_means_no_prediction() {
        let prev = image(&bytes("[]", "sys", "abc"));
        assert_eq!(compare(&bytes("[]", "sys", "abcd"), Some(&prev), BLOCK, 0), (None, 0));
    }

    #[test]
    fn each_segment_drifts_independently() {
        let mut prev = image(&bytes("[]", "sys", "abc"));
        prev.prompt_tokens = Some(1000);

        let d = compare(&bytes("[{}]", "sys", "abc"), Some(&prev), BLOCK, 0);
        assert_eq!(d, (Some(Segment::Tools), 0));
        let d = compare(&bytes("[]", "sys2", "abc"), Some(&prev), BLOCK, 0);
        assert_eq!(d, (Some(Segment::System), 0));
        // 中段改写（压缩）：长度可能一样，但前缀对不上 → 漂。
        let d = compare(&bytes("[]", "sys", "aXc"), Some(&prev), BLOCK, 0);
        assert_eq!(d, (Some(Segment::History), 0));
        // 变短也是漂——仅扩展语义下不存在「缩回去还能命中」。
        let d = compare(&bytes("[]", "sys", "ab"), Some(&prev), BLOCK, 0);
        assert_eq!(d, (Some(Segment::History), 0));
    }
}
