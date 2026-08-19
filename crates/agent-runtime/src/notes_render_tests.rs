//! `notes_render` 的单元测试：**措辞**这一半。一个 `Session` 都不造。
//!
//! 端到端（真的写、真的 undo、真的恢复）在 `tests/it/notes_indep_*.rs`。

use super::*;

use std::sync::Arc;

fn notes(pairs: &[(&str, &str)]) -> Notes {
    pairs
        .iter()
        .map(|(k, v)| (Arc::from(*k), Arc::from(*v)))
        .collect()
}

/// **红线 11**：同一张表渲染两次逐字节相同，且跟插入顺序无关——
/// 这段字节会以 tool_result 的形式进历史并被缓存。
#[test]
fn the_same_pad_renders_byte_identically() {
    let forward = render(&notes(&[("a", "一"), ("b", "二")]));
    let backward = render(&notes(&[("b", "二"), ("a", "一")]));
    assert_eq!(forward, backward);
    assert_eq!(forward, render(&notes(&[("a", "一"), ("b", "二")])));
}

/// 条目按 key 升序出现（容器自带，这条钉的是「渲染没把它打乱」）。
#[test]
fn entries_come_out_in_key_order() {
    let body = render(&notes(&[("zeta", "后"), ("alpha", "先")]));
    let first = body.find("alpha").expect("alpha 该在正文里");
    let second = body.find("zeta").expect("zeta 该在正文里");
    assert!(first < second, "该按 key 升序：{body}");
}

/// 空表也要给一段正文。**「查到了，里面是空的」跟「这个工具坏了」必须分得开**
/// ——回空字符串的话模型只能猜。
#[test]
fn an_empty_pad_still_says_something() {
    let body = render(&Notes::new());
    assert!(body.contains("空"), "{body}");
    assert!(
        body.contains(crate::NOTES_SET_TOOL),
        "空的时候更该告诉它怎么往上记：{body}"
    );
}

/// 写成功的回执必须说清「记下了 ≠ 下一轮我会看见」——草稿纸不自动进 prompt，
/// 不说这句模型会把它当成背景板。
#[test]
fn the_write_receipt_says_it_will_not_show_up_by_itself() {
    let body = wrote("plan", None);
    assert!(body.contains("plan"), "{body}");
    assert!(body.contains("不会自动出现"), "{body}");
    assert!(body.contains(crate::NOTES_TOOL), "{body}");
}

/// 截断要**如实说**，还要说清怎么拿到完整的。静默截断的后果是模型基于一段
/// 残缺的笔记下结论，而它完全不知道笔记残缺。
#[test]
fn truncation_is_reported_with_the_original_size() {
    let body = wrote("long", Some(4096));
    assert!(body.contains("截断"), "{body}");
    assert!(body.contains("4096"), "得说原来多大：{body}");
    assert!(body.contains("拆成几条"), "得给下一步：{body}");
}

/// 删是幂等的，回执也不该把「本来就没有」说成失败。
#[test]
fn deleting_is_idempotent_in_wording_too() {
    let body = removed("gone");
    assert!(body.contains("gone"), "{body}");
    assert!(body.contains("本来就没有"), "{body}");
}

/// 撞顶那条要给出**两条**出路：删几条，或者改一条已有的（覆盖不占新格子）。
/// 只说「满了」的话模型会卡在那儿反复重试。
#[test]
fn a_full_pad_offers_both_ways_out() {
    let body = explain(&NoteDenied::TooManyNotes { live: 32, max: 32 });
    assert!(body.contains("先删几条"), "{body}");
    assert!(body.contains("覆盖已有的 key 不占新格子"), "{body}");
}

/// key 超长**不截断**，而且要说清为什么——截短的 key 是另一个名字，
/// 模型下一轮拿原名查不到，而它记的时候明明成功了。
#[test]
fn a_long_key_is_refused_with_the_reason_spelled_out() {
    let body = explain(&NoteDenied::KeyTooLong { bytes: 200, max: 64 });
    assert!(body.contains("200"), "{body}");
    assert!(body.contains("64"), "{body}");
    assert!(body.contains("不替你截断"), "{body}");
}

/// 每一条拒绝都得带上下一步——这是本仓拒绝文案的通例（`send_tool::explain`）。
#[test]
fn every_refusal_is_actionable() {
    use agent_core::AgentId;
    let all = [
        NoteDenied::EmptyKey,
        NoteDenied::KeyTooLong { bytes: 99, max: 64 },
        NoteDenied::ValueTooLong {
            bytes: 9999,
            max: 1024,
        },
        NoteDenied::TooManyNotes { live: 32, max: 32 },
        NoteDenied::NotInSession {
            agent: AgentId::new("root/x"),
        },
        NoteDenied::NotLive {
            agent: AgentId::new("root/x"),
        },
    ];
    for denied in all {
        let body = explain(&denied);
        assert!(
            body.starts_with("记笔记失败："),
            "拒绝该一眼看出是哪个工具拒的：{body}"
        );
        assert!(body.len() > 20, "只说「不行」没有用：{body}");
    }
}
