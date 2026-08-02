//! 独立测试：缓存兜底第 1 层，发前比对（issue 024）。
//!
//! 只经公开 API（`agent_core::cache::{check_drift, DriftVerdict, PrefixIntent}`，
//! `agent_core::Segment`）验证。覆盖验收原文：
//! - drift 命中时报出且措辞里含段名，Tools/System/History 各一条
//! - 有意变更前缀时同一个 drift 不算事故
//! - 零网络：纯函数，同输入两次调用结果相等
//!
//! 这份测试是独立测试 agent 写的，没有读过 `src/cache/` 的实现或
//! `cache_guard_*.rs`——只信 rustdoc 和 issue 文档。

use agent_core::cache::{check_drift, DriftVerdict, PrefixIntent};
use agent_core::Segment;

#[test]
fn tools_drift_reuse_is_unexpected_and_names_segment() {
    let verdict = check_drift(Some(Segment::Tools), PrefixIntent::Reuse);
    assert_eq!(
        verdict,
        DriftVerdict::Unexpected {
            segment: Segment::Tools
        }
    );
    let text = verdict.to_string();
    assert!(
        text.contains("Tools"),
        "Unexpected(Tools) 的措辞里应能读出是 Tools 段漂了，实际: {text:?}"
    );
}

#[test]
fn system_drift_reuse_is_unexpected_and_names_segment() {
    let verdict = check_drift(Some(Segment::System), PrefixIntent::Reuse);
    assert_eq!(
        verdict,
        DriftVerdict::Unexpected {
            segment: Segment::System
        }
    );
    let text = verdict.to_string();
    assert!(
        text.contains("System"),
        "Unexpected(System) 的措辞里应能读出是 System 段漂了，实际: {text:?}"
    );
}

#[test]
fn history_drift_reuse_is_unexpected_and_names_segment() {
    let verdict = check_drift(Some(Segment::History), PrefixIntent::Reuse);
    assert_eq!(
        verdict,
        DriftVerdict::Unexpected {
            segment: Segment::History
        }
    );
    let text = verdict.to_string();
    assert!(
        text.contains("History"),
        "Unexpected(History) 的措辞里应能读出是 History 段漂了，实际: {text:?}"
    );
}

/// 同一个 drift（Tools 段漂了），本轮的意图不同，判读必须不同——
/// 有意变更前缀不能被当成事故（issue 024 验收 + 实做记录：`PrefixIntent`
/// 不是 `bool`，就是为了拦住「传反了静默放过」）。
#[test]
fn intentional_prefix_change_is_not_an_accident() {
    let accidental = check_drift(Some(Segment::Tools), PrefixIntent::Reuse);
    let intentional = check_drift(Some(Segment::Tools), PrefixIntent::Intentional);

    assert_eq!(
        accidental,
        DriftVerdict::Unexpected {
            segment: Segment::Tools
        }
    );
    assert_eq!(
        intentional,
        DriftVerdict::Expected {
            segment: Segment::Tools
        }
    );
    assert_ne!(
        accidental, intentional,
        "有意变更前缀时同一个 drift 不能算事故"
    );
}

/// 零网络的证明：`check_drift` 是纯函数，同一对输入调用两次必须完全一样，
/// 不依赖时钟、随机或任何外部状态（红线 1）。
#[test]
fn check_drift_is_pure_same_input_same_output() {
    let a = check_drift(Some(Segment::System), PrefixIntent::Reuse);
    let b = check_drift(Some(Segment::System), PrefixIntent::Reuse);
    assert_eq!(a, b);

    // None + Intentional：改了前缀但该复用的段没变，落 Clean。
    let a2 = check_drift(None, PrefixIntent::Intentional);
    let b2 = check_drift(None, PrefixIntent::Intentional);
    assert_eq!(a2, b2);
    assert_eq!(a2, DriftVerdict::Clean);
}
