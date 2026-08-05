//! SKILL.md frontmatter 用的**缩进式 YAML 子集**解析器（039）。
//!
//! # 为什么手写而不是拉一个 crate
//!
//! 本仓到 M4 为止一个 YAML 依赖都没有，加一个（`serde_yaml` 已归档、维护中的 fork
//! 又各有取舍）要动 workspace 依赖图、要能联网拉包——而 frontmatter 要解析的形状
//! 是一个**极小的固定子集**：标量、嵌套 map、`- ` 列表、以及 `{}`/`[]` 空容器
//! （skill 工具的 `schema` 那一层）。为这点东西引一个几千行的通用解析器不划算，
//! 也把「这段能解析成什么」这件事交给了一个我们不控制的库。
//!
//! # 支持的子集（超出就当字符串标量兜底，不 panic）
//!
//! - `key: value` 标量映射（value 里可以有冒号：`name: srv:foo/bar`，只按**第一个**
//!   冒号切）
//! - `key:` 后跟更深缩进的嵌套 map / 列表
//! - `- item` 块列表；列表项可以是标量，也可以是 map（第一字段跟在 `- ` 后面）
//! - 行内空容器 `{}` → 空 object、`[]` → 空 array
//! - `#` 整行注释、空行：跳过
//!
//! **刻意不支持**：多行标量（`|`/`>`）、锚点、流式非空 `{a: 1}`、引号内转义。
//! SKILL.md 的 frontmatter 不需要它们；真需要了再换实现，接口（[`parse`] 返回
//! `serde_json::Value`）不动。

use serde_json::{Map, Value};

/// 一行有效内容：缩进列数 + 去掉缩进后的正文。空行/注释在收集时就丢了。
struct Line {
    indent: usize,
    text: String,
}

/// 把一段 frontmatter 文本解析成 `serde_json::Value`（顶层通常是一个 object）。
pub(crate) fn parse(text: &str) -> Value {
    let lines = collect(text);
    parse_node(&lines)
}

/// 收集有效行：去掉空行与整行注释，算出每行缩进。
fn collect(text: &str) -> Vec<Line> {
    text.lines()
        .filter_map(|raw| {
            let trimmed = raw.trim_end();
            let indent = trimmed.len() - trimmed.trim_start().len();
            let body = &trimmed[indent..];
            if body.is_empty() || body.starts_with('#') {
                return None;
            }
            Some(Line {
                indent,
                text: body.to_string(),
            })
        })
        .collect()
}

/// 解析一段同属一个父节点的行（它们的最小缩进是同一个）。
fn parse_node(lines: &[Line]) -> Value {
    let Some(first) = lines.first() else {
        return Value::Null;
    };
    if is_seq_marker(&first.text) {
        parse_seq(lines, first.indent)
    } else {
        parse_map(lines, first.indent)
    }
}

fn is_seq_marker(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

/// 块列表：每个 `- ` 起一项，项内更深缩进的行归这一项。
fn parse_seq(lines: &[Line], indent0: usize) -> Value {
    let mut items = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        // 一项从这行（indent0 且 `- `）开始，到下一个同缩进的 `- ` 之前结束。
        let start = i;
        i += 1;
        while i < lines.len() && !(lines[i].indent == indent0 && is_seq_marker(&lines[i].text)) {
            i += 1;
        }
        items.push(parse_item(&lines[start..i], indent0));
    }
    Value::Array(items)
}

/// 一个列表项：把 `- ` 后面那截当成缩进 `indent0 + 2` 的第一行，跟后续更深的行
/// 一起当一个子块解析——于是 `- name: x` 后面对齐的 `description:` / `schema:`
/// 自然并进同一个 map。
fn parse_item(lines: &[Line], indent0: usize) -> Value {
    let head = lines[0].text[1..].trim_start(); // 去掉开头的 '-'
    let mut sub: Vec<Line> = Vec::new();
    if !head.is_empty() {
        sub.push(Line {
            indent: indent0 + 2,
            text: head.to_string(),
        });
    }
    for line in &lines[1..] {
        sub.push(Line {
            indent: line.indent,
            text: line.text.clone(),
        });
    }
    parse_node(&sub)
}

/// 块映射：每个 `key: ...` 一个键。有行内值就当标量，没有就把更深缩进的块当值。
fn parse_map(lines: &[Line], indent0: usize) -> Value {
    let mut map = Map::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].indent != indent0 {
            // 更深的行归上一个键（在下面的 child 收集里已经吃掉了），更浅的不该出现。
            i += 1;
            continue;
        }
        let (key, inline) = split_key(&lines[i].text);
        // 收集这个键底下更深缩进的子行。
        let child_start = i + 1;
        let mut j = child_start;
        while j < lines.len() && lines[j].indent > indent0 {
            j += 1;
        }
        let value = match inline {
            Some(v) if !v.is_empty() => scalar(v),
            _ => {
                if child_start < j {
                    parse_node(&lines[child_start..j])
                } else {
                    Value::Null
                }
            }
        };
        if let Some(key) = key {
            map.insert(key, value);
        }
        i = j;
    }
    Value::Object(map)
}

/// 按**第一个**冒号把 `key: value` 切开。value 里可以再有冒号（工具全名
/// `srv:foo/bar`）。没有冒号的行（不合法）返回 `(None, ..)`，调用方跳过。
fn split_key(text: &str) -> (Option<String>, Option<&str>) {
    match text.split_once(':') {
        Some((k, v)) => (Some(k.trim().to_string()), Some(v.trim())),
        None => (None, None),
    }
}

/// 行内标量：`{}` / `[]` 认成空容器，其余当字符串（去掉一对包裹引号）。
fn scalar(v: &str) -> Value {
    match v {
        "{}" => Value::Object(Map::new()),
        "[]" => Value::Array(Vec::new()),
        _ => Value::String(unquote(v).to_string()),
    }
}

fn unquote(v: &str) -> &str {
    let bytes = v.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// SKILL.md frontmatter 的真实形状：name/description + 一个带 schema 的工具。
    #[test]
    fn parses_the_skill_frontmatter_shape() {
        let text = "\
name: testskill
description: 一个技能, MARKER。
tools:
  - name: srv:testskill/ping
    description: ping 工具。
    schema:
      type: object
      properties: {}
";
        let v = parse(text);
        assert_eq!(v["name"], json!("testskill"));
        assert_eq!(v["description"], json!("一个技能, MARKER。"));
        let tool = &v["tools"][0];
        assert_eq!(tool["name"], json!("srv:testskill/ping"));
        assert_eq!(tool["description"], json!("ping 工具。"));
        assert_eq!(tool["schema"], json!({"type": "object", "properties": {}}));
    }

    /// 值里的冒号只按第一个切；空容器认得出来；注释/空行跳过。
    #[test]
    fn colons_in_values_and_empty_containers_and_comments() {
        let text = "\
# 这是注释
name: a

full: srv:ns/tool
empty_obj: {}
empty_arr: []
quoted: \"hi\"
";
        let v = parse(text);
        assert_eq!(v["name"], json!("a"));
        assert_eq!(v["full"], json!("srv:ns/tool"));
        assert_eq!(v["empty_obj"], json!({}));
        assert_eq!(v["empty_arr"], json!([]));
        assert_eq!(v["quoted"], json!("hi"));
    }

    /// 没有 tools 的最小 frontmatter 也能解析。
    #[test]
    fn a_minimal_frontmatter_without_tools() {
        let v = parse("name: solo\ndescription: d\n");
        assert_eq!(v["name"], json!("solo"));
        assert!(v.get("tools").is_none());
    }
}
