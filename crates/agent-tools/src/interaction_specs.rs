//! 由浏览器宿主执行的标准交互工具声明。
//!
//! 这些工具只有模型可见的契约：运行时把它们路由给 Web 宿主，再由宿主带着同一
//! `tool_call_id` 回传结果。这里不持有浏览器状态，也不伪造本地执行器。

use agent_core::ToolSpec;
use serde_json::json;
use std::sync::Arc;

/// Web 宿主实现的标准交互工具，顺序与 web-agent 保持一致。
pub(crate) fn interaction_specs() -> Vec<ToolSpec> {
    vec![
        ask_user_question_spec(),
        browser_action_spec(),
        save_file_spec(),
    ]
}

fn ask_user_question_spec() -> ToolSpec {
    spec(
        "ask_user_question",
        "向用户展示一个结构化问题卡并暂停当前工具槽，直到用户提交答案。不要把答案猜成工具输出，\
         浏览器提交后才会继续本轮。每项以 id 作为稳定答案键、以 text 展示问题；为容忍卡片的\
         渐进渲染，各问题对象字段刻意不强制类型，由浏览器归一化并过滤无效项。",
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "可选：本问题卡的稳定标识。"
                },
                "title": {
                    "type": "string",
                    "description": "可选：显示给用户的简短标题。"
                },
                "context": { "description": "可选：帮助用户作答的背景。" },
                "questions": {
                    "type": "array",
                    "description": "必填：等待用户逐项作答的问题。",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "description": "问题标识（字符串）；缺失或为空会被浏览器丢弃。"
                            },
                            "text": {
                                "description": "问题文本（字符串）；缺失或为空会被浏览器丢弃。"
                            },
                            "type": {
                                "description": "可选类型：text、single-choice、multi-choice 或 confirm；其他值会归一为 text。"
                            },
                            "options": {
                                "description": "choice 类问题的可选项，建议使用字符串数组；不合规值会被浏览器忽略。"
                            },
                            "required": {
                                "description": "可选：是否必须作答（boolean）；不合规值会被浏览器忽略。"
                            }
                        }
                    }
                }
            },
            "required": ["questions"]
        }),
    )
}

fn browser_action_spec() -> ToolSpec {
    spec(
        "browser_action",
        "请求浏览器渲染一个信息卡；目前唯一允许的 action 是 render_card。payload.title 必填，\
         body 可选且不超过 100000 个字符。工具结果只在浏览器成功渲染后返回 cardId；\
         它不读取网页，也不执行任意脚本。",
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "enum": ["render_card"],
                    "description": "必填：固定为 render_card。"
                },
                "payload": {
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": 200,
                            "description": "必填：卡片标题。"
                        },
                        "body": {
                            "type": "string",
                            "maxLength": 100000,
                            "description": "可选：卡片正文。"
                        }
                    },
                    "required": ["title"],
                    "additionalProperties": false
                }
            },
            "required": ["action", "payload"],
            "additionalProperties": false
        }),
    )
}

fn save_file_spec() -> ToolSpec {
    spec(
        "save_file",
        "请求浏览器在用户手势下保存一个下载文件。filename 与 content 必填；content 的 UTF-8\
         字节长度最多 5 MiB，mimeType 可选。浏览器拒绝或用户取消时会返回错误结果；\
         不要把它当成 workspace 写入工具，也不能用它覆盖服务端文件。",
        json!({
            "type": "object",
            "properties": {
                "filename": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 255,
                    "description": "必填：建议保存给用户的文件名。"
                },
                "content": {
                    "type": "string",
                    "maxLength": 5242880,
                    "description": "必填：待下载的文本内容，UTF-8 编码后最多 5 MiB。"
                },
                "mimeType": {
                    "type": "string",
                    "maxLength": 255,
                    "description": "可选：下载的 MIME 类型，例如 text/plain。"
                }
            },
            "required": ["filename", "content"],
            "additionalProperties": false
        }),
    )
}

fn spec(name: &'static str, description: &'static str, schema: serde_json::Value) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(schema),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_keep_the_ai_friendly_web_agent_contracts() {
        let specs = interaction_specs();
        let names: Vec<&str> = specs.iter().map(|spec| &*spec.name).collect();
        assert_eq!(names, ["ask_user_question", "browser_action", "save_file"]);
        let ask = &specs[0].schema;
        assert_eq!(ask["required"], json!(["questions"]));
        assert_eq!(ask["properties"]["id"]["type"], "string");
        assert_eq!(ask["properties"]["title"]["type"], "string");
        assert!(ask["properties"]["context"].get("type").is_none());
        assert!(ask.get("additionalProperties").is_none());
        assert!(ask["properties"]["questions"].get("minItems").is_none());
        let question = &ask["properties"]["questions"]["items"];
        for field in ["id", "text", "type", "options", "required"] {
            assert!(question["properties"][field]["description"].is_string());
        }
        assert!(question.get("additionalProperties").is_none());

        let browser = &specs[1].schema;
        assert_eq!(
            browser["properties"]["action"]["enum"],
            json!(["render_card"])
        );
        assert_eq!(browser["additionalProperties"], false);
        assert_eq!(
            browser["properties"]["payload"]["additionalProperties"],
            false
        );

        let save = &specs[2].schema;
        assert_eq!(save["properties"]["filename"]["maxLength"], 255);
        assert_eq!(save["properties"]["content"]["maxLength"], 5_242_880);
        assert!(save["properties"].get("mimeType").is_some());
        assert_eq!(save["additionalProperties"], false);
    }
}
