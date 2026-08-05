//! 从磁盘装载 skill：遍历 `<dir>/<name>/SKILL.md`，切 frontmatter/正文，建 [`Skill`]。
//!
//! frontmatter 形状（照抄本仓 `.claude/skills/*/SKILL.md` 的惯例）：
//!
//! ```text
//! ---
//! name: <id>
//! description: <进索引的那行>
//! tools:                      # 可选
//!   - name: srv:foo/bar
//!     description: ...
//!     schema: { ... }
//! ---
//! <正文，激活时整段进 late_system>
//! ```
//!
//! **宽容装载**：缺 frontmatter 就用目录名当 id、正文取全文；缺 `name` 用目录名；
//! 缺 `description` 空串；缺 `tools` 没有携带工具。装载不因为一个 skill 写得潦草
//! 就整批失败——只有真的 IO 错误（目录能列但文件读不出来）才返回 `Err`。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use agent_core::{SkillId, ToolSpec};
use serde_json::{Value, json};

use super::{Skill, SkillSource, yaml};

/// skill 装载失败。**只有真的 IO 错误**（列目录、读文件失败）会到这里——解析层面
/// 的潦草一律宽容兜底，不升级成错误（一个坏 skill 不该让整个会话起不来）。
#[derive(Debug)]
pub enum SkillLoadError {
    /// 读一个存在的路径失败（权限、坏链接……）。
    Io { path: String, message: String },
}

impl std::fmt::Display for SkillLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillLoadError::Io { path, message } => {
                write!(f, "读 skill 目录 {path} 失败：{message}")
            }
        }
    }
}

impl std::error::Error for SkillLoadError {}

/// 遍历一个来源目录，把里面每个 `<name>/SKILL.md` 装进 `out`（同名后来居上）。
/// 目录不存在直接返回 `Ok`——宿主指向一个还没建的 `./skills/` 是常态。
pub(super) fn load_dir(
    dir: &Path,
    out: &mut BTreeMap<Arc<str>, Skill>,
) -> Result<(), SkillLoadError> {
    if !dir.exists() {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| io_err(dir, &e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_err(dir, &e))?;
        let path = entry.path();
        let md = path.join("SKILL.md");
        if !md.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&md).map_err(|e| io_err(&md, &e))?;
        let fallback = entry.file_name().to_string_lossy().into_owned();
        let skill = build_skill(&fallback, &content);
        out.insert(Arc::clone(&skill.id.0), skill);
    }
    Ok(())
}

fn io_err(path: &Path, e: &std::io::Error) -> SkillLoadError {
    SkillLoadError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

/// 把一份 SKILL.md 文本建成 [`Skill`]。宽容：任何缺失都有兜底。
fn build_skill(fallback_id: &str, content: &str) -> Skill {
    let (front, body) = split_frontmatter(content);
    let meta = front.map(yaml::parse).unwrap_or(Value::Null);

    let id = meta
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(fallback_id);
    let description = meta
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let tools = meta
        .get("tools")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(tool_spec).collect::<Vec<_>>());

    Skill {
        id: SkillId::new(id),
        description: Arc::from(description),
        body: Arc::from(body.trim()),
        tools: tools.unwrap_or_default(),
        source: SkillSource::Disk,
    }
}

/// 一条 `tools` 项 → [`ToolSpec`]。缺 `name` 的项跳过（`None`）：一个没名字的
/// 工具喂给模型没有意义。缺 `schema` 兜底成 `{"type":"object"}`（红线 11：
/// `serde_json::Value` 的对象是 `BTreeMap`，逐字节确定）。
fn tool_spec(item: &Value) -> Option<ToolSpec> {
    let name = item.get("name").and_then(Value::as_str)?;
    let description = item
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let schema = item
        .get("schema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object"}));
    Some(ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(schema),
    })
}

/// 切 `---` frontmatter 和正文。没有起始 `---` → 没有 frontmatter，全文是正文。
/// 有起始但找不到收尾 `---` → 同样当没有 frontmatter（宁可把整段当正文，也不
/// 吞掉半段当元数据）。
fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let rest = match content.strip_prefix("---\n") {
        Some(rest) => rest,
        None => match content.strip_prefix("---\r\n") {
            Some(rest) => rest,
            None => return (None, content),
        },
    };
    // 找收尾的 `---` 行（行首）。
    for marker in ["\n---\n", "\n---\r\n"] {
        if let Some(pos) = rest.find(marker) {
            let front = &rest[..pos];
            let body = &rest[pos + marker.len()..];
            return (Some(front), body);
        }
    }
    // 收尾行可能是文件最后一行、没有结尾换行。
    if let Some(front) = rest.strip_suffix("\n---") {
        return (Some(front), "");
    }
    (None, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL_MD: &str = "\
---
name: testskill
description: 一个技能, IDX。
tools:
  - name: srv:testskill/ping
    description: ping。
    schema:
      type: object
      properties: {}
---
这是正文 BODY。
";

    #[test]
    fn builds_a_skill_with_name_description_body_and_tool() {
        let skill = build_skill("dirname", SKILL_MD);
        assert_eq!(skill.id.as_str(), "testskill");
        assert_eq!(&*skill.description, "一个技能, IDX。");
        assert_eq!(&*skill.body, "这是正文 BODY。");
        assert_eq!(skill.tools.len(), 1);
        assert_eq!(&*skill.tools[0].name, "srv:testskill/ping");
        assert_eq!(
            &*skill.tools[0].schema.to_string(),
            r#"{"properties":{},"type":"object"}"#
        );
    }

    /// 缺 frontmatter：目录名当 id，全文当正文，没有工具。
    #[test]
    fn a_bare_markdown_falls_back_to_the_dir_name() {
        let skill = build_skill("myskill", "just some instructions\n");
        assert_eq!(skill.id.as_str(), "myskill");
        assert_eq!(&*skill.body, "just some instructions");
        assert!(skill.tools.is_empty());
    }

    /// 不存在的目录：装载跳过、不报错。
    #[test]
    fn a_missing_directory_is_skipped_not_an_error() {
        let mut out = BTreeMap::new();
        load_dir(Path::new("/nonexistent/skills/dir/xyz"), &mut out).unwrap();
        assert!(out.is_empty());
    }
}
