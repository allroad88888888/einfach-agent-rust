//! 138 独立测试（agent-runtime 层）：`SkillRegistry::index_text()`——常驻索引
//! 文本本身，不装配（139 才把它接进 `SessionStart`）。
//!
//! 独立测试 agent 规则：只依据 `docs/issues/138-skill-index-tool.md` 的
//! 「验收」「注意」两节、`docs/issues/142-skill-hidden-frontmatter.md` 的
//! 「验收」节（只为确认"不带 hidden 字段的 skill 一律该出现在索引里"，本文件
//! 不写 hidden 用例——142 是另一条独立的 issue）、`docs/INVARIANTS.md` 红线 11、
//! 本 crate 既有测试里 `SKILL.md` fixture 的写法写成，**没看** `skill/index.rs`、
//! `skill/read.rs`（138/139 的被测实现，写这份测试时仓里还没有这两个文件）。
//!
//! # 假定的公开签名（未见实现体，风险点见下）
//!
//! ```ignore
//! impl SkillRegistry {
//!     pub fn index_text(&self) -> Arc<str>;
//! }
//! ```
//!
//! 语义（照抄任务方给的契约，与 138 原文一致）：空 registry → 空串；否则首行
//! 固定引导句（提到 `srv:skill/read`），之后每个 skill 一行 `<id> — <description>`
//! （id 与 description 之间是全角破折号，前后各一个半角空格），按 id 字典序；
//! description 里的换行折成空格；正文字节绝不出现在输出里。
//!
//! # 风险点
//!
//! 1. **首行只断言"提到 `srv:skill/read`"**（子串），不钉死整句引导语——138 的
//!    「验收」节原文就是"首行提到 srv:skill/read"这个宽松的判据，「做什么」节
//!    那句完整引导句是实现参考,不是逐字契约。
//! 2. `<id> — <description>` 的分隔符是任务方给的**契约**（不是我自己猜的），
//!    这里按字面钉死；如果实现用了别的分隔符，这是一个会红的真发现，不是我
//!    测试写错。
//! 3. "description 带换行"这个用例改用 `SkillRegistry::from_host_skills` 直接
//!    在 Rust 里塞一个含 `\n` 的 `Arc<str>`，不走磁盘 `SKILL.md`——`skill/yaml.rs`
//!    那个"缩进式 YAML 子集"支不支持多行块标量（`|`/`>`）不在本 agent 的可读范围
//!    内，无法确认能否在frontmatter 文本里安全写出一个内嵌换行的 `description`
//!    字段；`from_host_skills` 是既有 pub 构造器，绕开这个未知数，直接对着
//!    "registry 里已经有一个 description 含 `\n` 的 skill"这个状态断言折行行为，
//!    与 138 的验收范围完全一致（校验的是 `index_text` 的折行逻辑，不是 YAML
//!    解析）。其余用例都按任务指示走磁盘 `SKILL.md`（`SkillRegistry::load`）。

use crate::support;
use std::path::Path;
use std::sync::Arc;

use agent_core::{HostSkill, SkillId};
use agent_runtime::SkillRegistry;

/// 落一份最小可用的 skill 目录：frontmatter(name/description/一个工具) + 正文。
/// 形状照抄 `skill_indep_registry_and_activation_e2e.rs` 的 `write_test_skill`
/// 惯例——逐行拼接,不用带反斜杠续行的字符串字面量(会悄悄吃掉下一行开头的缩进)。
fn write_skill(
    skills_root: &Path,
    dir_name: &str,
    id: &str,
    description: &str,
    body_marker: &str,
) {
    let dir = skills_root.join(dir_name);
    std::fs::create_dir_all(&dir).unwrap();
    let lines = [
        "---".to_string(),
        format!("name: {id}"),
        format!("description: {description}"),
        "tools:".to_string(),
        format!("  - name: srv:{id}/ping"),
        "    description: 独立测试用的 ping 工具。".to_string(),
        "    schema:".to_string(),
        "      type: object".to_string(),
        "      properties: {}".to_string(),
        "---".to_string(),
        format!("这是 {id} 的正文,索引里不该出现它的任何字节。{body_marker}"),
    ];
    std::fs::write(dir.join("SKILL.md"), lines.join("\n") + "\n").unwrap();
}

fn load_registry(skills_root: &Path) -> SkillRegistry {
    SkillRegistry::load(&[skills_root.to_path_buf()]).expect("加载测试用 skill 目录不该失败")
}

/// 验收 1：0 个 skill → 空串。
#[test]
fn zero_skills_index_text_is_empty() {
    let registry = SkillRegistry::empty();
    assert_eq!(
        &*registry.index_text(),
        "",
        "空 registry 的 index_text() 该是空串"
    );
}

/// 目录名故意按 skill-a..skill-d 排,里面塞的 id 刻意打乱——多数文件系统对同一
/// 目录下子目录的遍历序接近目录名序(不保证,但足够构造一份"遍历序 ≠ id 字典序"
/// 的 fixture),用来验证 index_text() 排的是 id 而不是"谁先被扫到"。
const ORDER_SKILLS: &[(&str, &str, &str)] = &[
    ("skill-a", "zulu-flow", "最后一个流程的说明"),
    ("skill-b", "alpha-flow", "第一个流程的说明"),
    ("skill-c", "delta-flow", "第三个流程的说明"),
    ("skill-d", "mango-flow", "第二个流程的说明"),
];

/// 验收 2：N 个 skill,目录遍历序 ≠ id 字典序 → index_text() 的行序是 id 字典序。
/// 顺带钉住每行的确切格式 `<id> — <description>`(任务方给的契约)。
#[test]
fn n_skills_are_ordered_by_id_not_by_directory_traversal_order() {
    let skills_root = support::temp_dir("skill-index-order");
    for &(dir_name, id, description) in ORDER_SKILLS {
        write_skill(
            &skills_root,
            dir_name,
            id,
            description,
            &format!("BODY_{id}_ZX77"),
        );
    }
    let registry = load_registry(&skills_root);
    let text = registry.index_text();

    let mut lines = text.lines();
    let header = lines.next().expect("非空 registry 该有首行引导句");
    assert!(
        header.contains("srv:skill/read"),
        "首行该提到 srv:skill/read: {header}"
    );

    let mut expected: Vec<(&str, &str)> =
        ORDER_SKILLS.iter().map(|(_, id, d)| (*id, *d)).collect();
    expected.sort_by_key(|(id, _)| *id);
    let expected_lines: Vec<String> = expected
        .iter()
        .map(|(id, d)| format!("{id} — {d}"))
        .collect();
    let expected_refs: Vec<&str> = expected_lines.iter().map(String::as_str).collect();

    let body_lines: Vec<&str> = lines.collect();
    assert_eq!(
        body_lines, expected_refs,
        "index_text() 正文行序该是 id 字典序,不是目录遍历序,且每行该是「id — description」"
    );
}

/// 验收 3：同一 registry 两次 index_text() 逐字节相同。
#[test]
fn two_calls_on_the_same_registry_produce_byte_identical_text() {
    let skills_root = support::temp_dir("skill-index-repeat");
    write_skill(
        &skills_root,
        "only",
        "solo-flow",
        "唯一一个流程的说明",
        "BODY_SOLO_ZX77",
    );
    let registry = load_registry(&skills_root);

    let first = registry.index_text();
    let second = registry.index_text();

    assert_eq!(
        first.as_bytes(),
        second.as_bytes(),
        "同一 registry 两次调用 index_text() 该逐字节相同"
    );
}

/// 验收 4：输出含每个 id 与 description,不含任何正文字节(正文里放一个独特
/// 哨兵串,断言 !contains)。
#[test]
fn index_text_carries_ids_and_descriptions_but_never_a_body_byte() {
    let skills_root = support::temp_dir("skill-index-no-leak");
    const SENTINEL_ONE: &str = "BODY_SENTINEL_ONE_QK38";
    const SENTINEL_TWO: &str = "BODY_SENTINEL_TWO_QK38";
    write_skill(
        &skills_root,
        "dir-one",
        "report-flow",
        "出报表用的流程说明",
        SENTINEL_ONE,
    );
    write_skill(
        &skills_root,
        "dir-two",
        "audit-flow",
        "审计用的流程说明",
        SENTINEL_TWO,
    );

    let registry = load_registry(&skills_root);
    let text = registry.index_text();

    assert!(
        text.contains("report-flow"),
        "索引该含 id report-flow: {text}"
    );
    assert!(
        text.contains("出报表用的流程说明"),
        "索引该含 report-flow 的 description: {text}"
    );
    assert!(
        text.contains("audit-flow"),
        "索引该含 id audit-flow: {text}"
    );
    assert!(
        text.contains("审计用的流程说明"),
        "索引该含 audit-flow 的 description: {text}"
    );

    assert!(
        !text.contains(SENTINEL_ONE),
        "索引不该出现 report-flow 的正文字节: {text}"
    );
    assert!(
        !text.contains(SENTINEL_TWO),
        "索引不该出现 audit-flow 的正文字节: {text}"
    );
}

/// 验收 5：description 带换行 → 索引里该行内折成一个空格,不出现裸换行。
/// fixture 用 `from_host_skills` 直接塞含 `\n` 的 description(理由见文件头
/// 「风险点」第 3 条)。
#[test]
fn a_newline_inside_description_folds_into_a_single_space() {
    let registry = SkillRegistry::from_host_skills(vec![HostSkill {
        id: SkillId::new("folded-flow"),
        description: Arc::from("第一行\n第二行"),
        body: Arc::from("折行测试用的正文,索引不该出现它。BODY_FOLDED_ZX77"),
        tools: Vec::new(),
        tool_reversibility: Default::default(),
    }]);

    let text = registry.index_text();
    let mut lines = text.lines();
    let _header = lines.next().expect("非空 registry 该有首行引导句");
    let body_line = lines.next().expect("该有一行 folded-flow 的索引");
    assert_eq!(
        lines.next(),
        None,
        "description 里的换行不该在 index_text() 里产生额外的一行"
    );

    assert_eq!(
        body_line, "folded-flow — 第一行 第二行",
        "description 里的换行该折成一个空格,不是保留换行、也不是直接拼接掉"
    );
    assert!(
        !body_line.contains('\n'),
        "折行之后这一行内不该再有裸换行"
    );
}

/// 验收 6：非空 registry 的首行必须提到 `srv:skill/read`(模型据此知道怎么按
/// id 取正文)。独立于其它用例用一份最小 fixture,专门钉这一条。
#[test]
fn the_first_line_mentions_srv_skill_read() {
    let skills_root = support::temp_dir("skill-index-header");
    write_skill(
        &skills_root,
        "only",
        "header-flow",
        "首行断言用的流程说明",
        "BODY_HEADER_ZX77",
    );
    let registry = load_registry(&skills_root);

    let text = registry.index_text();
    let header = text.lines().next().expect("非空 registry 的 index_text() 该有首行");
    assert!(
        header.contains("srv:skill/read"),
        "首行该提到 srv:skill/read,让模型知道怎么按 id 取正文: {header}"
    );
}
