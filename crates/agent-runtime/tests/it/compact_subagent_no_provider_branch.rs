//! 106 验收：「core 里搜不到任何为摘要新增的 provider 分支」（红线 12）。
//!
//! 106 的契约 2 是「摘要子 agent 用哪个模型由它自己的 `ChildConfig` 定，
//! `agent-core` 不许为摘要多出任何 provider 分支」。106 issue 原文自己点名了
//! 一条可行的验证方式——「可以用 grep 断言」——这份文件就是那句话的落地：
//! 跑不动一个真正的压缩子 agent（那需要走 `dispatch::run_effect`，是
//! `agent-runtime` 的 crate 内部实现细节，独立测试 agent 按规矩不读也碰不到），
//! 但「压缩相关的源文件里没有厂商名/能力位分支」是纯静态事实，grep 就能钉死，
//! 不需要真的起一次子 agent。
//!
//! # 两个范围，两条独立断言
//!
//! 1. **字面意义的「core」**：106 涉及的 `agent-core` 源文件（105 的
//!    `Effect::Compact`/`Event::CompactDone`/`CompactFailed`、099 的 `SendPlan`、
//!    104 的 `advance_boundary`、096 的 `compaction/` 阈值模块、`ids.rs` 的
//!    `SummaryId`）——这是红线 12 字面管辖的范围。
//! 2. **契约 2 的另一半**：`agent-runtime` 里真正调度摘要子 agent 的那几个文件
//!    （`dispatch.rs`、`compact_spawn.rs`、`compact_slot.rs`，以及给
//!    `ChildConfig::execution_profile` 上药的 `ctx.rs`/`execution_binding.rs`）。
//!    这几个文件不算「core」，红线 12 不直接管——但 106 的契约要求「用哪个模型
//!    由 `ChildConfig` 定」，这句话翻成静态事实就是：调度这条路上也不该出现
//!    `match provider` 式的厂商名分支，选择面应该只剩一个不透明的
//!    `ExecutionProfileId`。加这一半，是把「或者用两个不同 ChildConfig 模型各跑
//!    一次证明走的是同一条路」这条验收备选也在静态层面覆盖一遍：如果调度代码
//!    里压根没有厂商名，那「换个 ChildConfig 换个模型」就不可能触发一条没测过的
//!    分支——因为分支从一开始就不存在。
//!
//! # 判据跟 `scripts/check-invariants.sh` 的 `check_no_model_branch` 同一套
//!
//! 厂商名单、`Capabilities`/`caps.` 两条正则原样照抄那个脚本（`--all` 模式已经在
//! 全仓跑过，见 `agent-core/tests/it/session_indep_meta_invariants.rs`），
//! 过滤注释行的写法照抄 `agent-core/tests/it/no_clock_meta_test.rs`——三份检查
//! 判的是同一件事（「静态文本里搜不到某个模式」），没必要各写一套过滤逻辑。
//! 这份测试比那两份**更窄更具体**：钉的是「106 新增/涉及的文件」，不是整个
//! `agent-core`——check-invariants.sh 已经兜底了全仓，这里的价值是把「压缩这条
//! 支线不该有分支」这句话变成一条指名道姓的、106 专属的回归锁。

use std::path::Path;
use std::process::Command;

/// 厂商名单，照抄 `scripts/check-invariants.sh` 的 `check_no_model_branch`。
const VENDOR_PATTERN: &str = "(deepseek|kimi|moonshot|zhipu|glm|openai|anthropic|gemini|qwen)";
/// 能力位分支，同款照抄——`if caps.xxx()` 是 `match provider` 换了层皮。
const CAPS_PATTERN: &str = "(Capabilities|caps\\.)";

#[test]
fn agent_core_compaction_files_have_no_vendor_names() {
    assert_no_match_in(&core_compaction_files(), VENDOR_PATTERN, true, "厂商名");
}

#[test]
fn agent_core_compaction_files_have_no_capability_branches() {
    assert_no_match_in(&core_compaction_files(), CAPS_PATTERN, false, "能力位分支（Capabilities/caps.）");
}

/// 契约 2 的另一半：调度摘要子 agent 的 `agent-runtime` 文件同样不许长出厂商名
/// 分支——选择面必须只剩 `ChildConfig::execution_profile` 这一个不透明数字。
#[test]
fn agent_runtime_compaction_dispatch_files_have_no_vendor_names() {
    assert_no_match_in(
        &runtime_compaction_files(),
        VENDOR_PATTERN,
        true,
        "厂商名",
    );
}

#[test]
fn agent_runtime_compaction_dispatch_files_have_no_capability_branches() {
    assert_no_match_in(
        &runtime_compaction_files(),
        CAPS_PATTERN,
        false,
        "能力位分支（Capabilities/caps.）",
    );
}

/// 105/099/104/096 落地或改动过的 `agent-core` 源文件：`Effect::Compact` 的形状、
/// `SendPlan` 及其投影、`advance_boundary` 命令、`compaction/` 阈值模块、
/// `SummaryId`。
fn core_compaction_files() -> Vec<std::path::PathBuf> {
    let core_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../agent-core/src");
    [
        "compaction/mod.rs",
        "compaction/clear_policy.rs",
        "command/advance_boundary.rs",
        "command/send_plan.rs",
        "command/transitions/mod.rs",
        "value/send_plan.rs",
        "value/send_plan/project.rs",
        "value/send_plan_codec.rs",
        "engine/effect.rs",
        "engine/event.rs",
        "engine/notice.rs",
        "ids.rs",
    ]
    .iter()
    .map(|rel| core_src.join(rel))
    .collect()
}

/// 106 落地的、真正调度摘要子 agent 的 `agent-runtime` 文件。
fn runtime_compaction_files() -> Vec<std::path::PathBuf> {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    [
        "dispatch.rs",
        "compact_spawn.rs",
        "compact_slot.rs",
        "ctx.rs",
        "execution_binding.rs",
    ]
    .iter()
    .map(|rel| runtime_src.join(rel))
    .collect()
}

/// 对每个文件跑一次 `grep -nE`（大小写按 `case_insensitive`），剔除以 `//`
/// 开头的注释行（跟 `no_clock_meta_test.rs` 同款过滤——文档提厂商名讨论权衡是
/// 允许的，代码里按厂商名分支才是红线 12 要挡的事），断言零命中。
fn assert_no_match_in(files: &[std::path::PathBuf], pattern: &str, case_insensitive: bool, what: &str) {
    for file in files {
        assert!(file.is_file(), "文件应该存在：{}", file.display());

        let mut cmd = Command::new("grep");
        cmd.arg("-nE");
        if case_insensitive {
            cmd.arg("-i");
        }
        cmd.arg(pattern).arg(file);
        let output = cmd.output().expect("grep 应该能正常执行");

        // grep 找不到匹配时退出码是 1（不是执行错误）——零命中正是我们想要的结果。
        let stdout = String::from_utf8_lossy(&output.stdout);
        let offending: Vec<&str> = stdout
            .lines()
            .filter(|line| {
                let content = line.split_once(':').map_or(*line, |(_, rest)| rest).trim_start();
                !content.starts_with("//")
            })
            .collect();

        assert!(
            offending.is_empty(),
            "{} 不许出现{}（红线 12：模型相关判断全部归 agent-providers，\
             摘要用哪个模型只能通过 ChildConfig::execution_profile 这个不透明数字传递）：\n{}",
            file.display(),
            what,
            offending.join("\n")
        );
    }
}
