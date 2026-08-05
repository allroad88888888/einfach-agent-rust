//! issue 024 验收 · 兜底**第 1 层**：发前比对，以及三层判读的纯函数/零网络。
//!
//! 这一层写错了不会有任何功能异常、任何别的测试变红——唯一的信号是账单。
//! 所以这里**主动构造违反**，不只测正常路径（红线 11 的运行期实检手段）。
//!
//! 「哪一段漂了」是 adapter 的活（红线 12），本文件里那部分由下面两个辅助函数
//! **扮演** adapter，好让验收能从真实字节一路走到判读结果。

use std::hash::{DefaultHasher, Hash, Hasher};

use agent_core::cache::{DriftVerdict, PrefixIntent, check_drift};
use agent_core::{PrefixImage, Segment, SegmentImage};

fn hash_bytes(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.as_bytes().hash(&mut h);
    h.finish()
}

/// 按 `[Tools][System][History]` 造一份镜像——三家实测的渲染顺序，不能改。
fn image(tools: &str, system: &str, history: &str) -> PrefixImage {
    let seg = |segment, text: &str| SegmentImage {
        segment,
        bytes: u32::try_from(text.len()).unwrap(),
        hash: hash_bytes(text),
    };
    PrefixImage {
        segments: vec![
            seg(Segment::Tools, tools),
            seg(Segment::System, system),
            seg(Segment::History, history),
        ],
        prompt_tokens: Some(2432),
    }
}

/// adapter 干的事：对着上一轮镜像找出**第一段**对不上的。缓存从头匹配，
/// 第一段一漂后面全作废，所以只报第一段。
fn first_drift(prev: &PrefixImage, next: &PrefixImage) -> Option<Segment> {
    prev.segments
        .iter()
        .zip(&next.segments)
        .find(|(a, b)| a != b)
        .map(|(a, _)| a.segment)
}

/// 验收：构造一个「`HashMap` 导致 key 顺序翻转」的输入，**必须被抓到，
/// 且报出是哪一段**。
///
/// 两个 key 顺序不靠真的 `HashMap` 来抖——同一个进程里它不保证抖，测试会变成
/// 偶发绿。直接把两次序列化的结果写死：**这正是同一份工具表在两个进程里用
/// `HashMap` 序列化出来的样子**，逻辑等价、字节不同。
#[test]
fn key_order_flip_in_tools_is_caught_and_named() {
    let system = "你是一个助手";
    let history = r#"[{"role":"user","content":"hi"}]"#;

    let run1 = r#"[{"name":"fs/read","description":"读文件"}]"#;
    let run2 = r#"[{"description":"读文件","name":"fs/read"}]"#;
    assert_ne!(run1, run2, "构造的两次序列化必须真的不同，否则这条测试什么也没测");

    let prev = image(run1, system, history);
    let next = image(run2, system, history);

    // 本轮没打算改前缀（M1 恒如此）——所以这是事故，不是预期。
    let verdict = check_drift(first_drift(&prev, &next), PrefixIntent::Reuse);
    assert_eq!(verdict, DriftVerdict::Unexpected { segment: Segment::Tools });

    // 「只报前缀变了等于没报」：措辞里必须说得出是哪一段。
    let text = verdict.to_string();
    assert!(text.contains("Tools"), "{text}");
}

/// 同一份料两次组装逐字节相同时，这一层必须闭嘴——否则告警会被当噪声无视。
#[test]
fn identical_prefix_is_clean() {
    let a = image("[tools]", "system", "history");
    let b = image("[tools]", "system", "history");
    assert_eq!(first_drift(&a, &b), None);
    assert_eq!(check_drift(None, PrefixIntent::Reuse), DriftVerdict::Clean);
}

/// System / History 段漂了也要报出各自的段名，不是笼统的「前缀变了」。
#[test]
fn each_segment_is_named_individually() {
    let base = image("[tools]", "system", "history");

    let sys_changed = image("[tools]", "system v2", "history");
    assert_eq!(
        check_drift(first_drift(&base, &sys_changed), PrefixIntent::Reuse),
        DriftVerdict::Unexpected { segment: Segment::System }
    );

    let hist_changed = image("[tools]", "system", "history 被中途改写了");
    assert_eq!(
        check_drift(first_drift(&base, &hist_changed), PrefixIntent::Reuse),
        DriftVerdict::Unexpected { segment: Segment::History }
    );
}

/// 有意变更前缀时，同一个 drift 不算事故——否则压缩一次报一次假警报，
/// 然后人开始无视这一层。
#[test]
fn intentional_change_is_expected_not_an_accident() {
    let prev = image("[tools]", "system", "history");
    let next = image("[tools]", "system", "压缩后的 history");
    let drift = first_drift(&prev, &next);

    assert_eq!(
        check_drift(drift, PrefixIntent::Reuse),
        DriftVerdict::Unexpected { segment: Segment::History }
    );
    assert_eq!(
        check_drift(drift, PrefixIntent::Intentional),
        DriftVerdict::Expected { segment: Segment::History }
    );
}

/// 验收：第 1 层不发任何 HTTP。判读是纯函数——同样的输入调一万次结果一样，
/// 没有隐藏状态可言。零网络那一半由下面的元测试直接查源码。
#[test]
fn layer1_is_pure() {
    let inputs = [None, Some(Segment::Tools), Some(Segment::System), Some(Segment::History)];
    for intent in [PrefixIntent::Reuse, PrefixIntent::Intentional] {
        for drift in inputs {
            let first = check_drift(drift, intent);
            for _ in 0..10_000 {
                assert_eq!(check_drift(drift, intent), first);
            }
        }
    }
}

/// 三层判读的源码里不许出现时钟、随机源、网络（红线 1 / 红线 7）。
///
/// 用 `grep` 而不是编译期检查：要抓的是「压根没调用」这件事本身，静态类型抓不到。
/// 跑子进程的写法与理由同 `no_clock_meta_test.rs`。
#[test]
fn cache_module_has_no_clock_no_random_no_network() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cache");
    assert!(dir.is_dir(), "cache 目录应该存在：{}", dir.display());

    let output = std::process::Command::new("grep")
        .args([
            "-rn",
            "-E",
            "Instant::now|SystemTime::now|rand::|thread_rng|OsRng|std::net|reqwest|ureq",
        ])
        .arg(&dir)
        .output()
        .expect("grep 应该能正常执行");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let offending: Vec<&str> = stdout
        .lines()
        .filter(|line| {
            let content = line.splitn(3, ':').nth(2).unwrap_or(line).trim_start();
            !content.starts_with("//")
        })
        .collect();

    assert!(offending.is_empty(), "cache/ 下不许出现时钟/随机/网络：\n{}", offending.join("\n"));
}
