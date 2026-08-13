# 178 OpenAI 兼容真机收官

**里程碑** L · **依赖** [177](177-openai-compat-config.md) · **模型** sonnet · **估时** 20min · **状态** 待开始

## 目标

真 provider 跑一轮，不是 mock。**本仓每个里程碑都以真机 dogfood 收尾**
（M9–M18 无一例外），这条不能因为「只是加个 adapter」就破例——
录制帧独测保证的是「解析对了」，保证不了「真的能用」。

## 做什么

按 [177](177-openai-compat-config.md) 配好，逐条跑：

| # | 验的什么 | 怎么算过 |
|---|---|---|
| 1 | OpenAI 官方一轮对话 | 有回复，无 `Adjustment` 意外告警 |
| 2 | **Ollama 本地一轮** | 有回复——这条最重要，它是零成本入口，坏了没人会来报 |
| 3 | 工具调用 | 模型主动调 `srv:fs/read` 读一个文件并据此回答 |
| 4 | 流式 | 分片正常，没有卡死或吞字 |
| 5 | 缓存字段 | 有的家读到真数字；**没有的家优雅报「不知道」而不是假 0** |
| 6 | 401 | 填错 key 被分类成 `Failed(Provider(Auth))`，不是笼统失败 |
| 7 | undo | 兼容家上 `/undo` 行为与三家一致 |

## 验收

- 七条逐条记录进本 issue 的实做记录（照 [163](163-m18-dogfood.md) 的写法：
  **记下真实数字和真实报错文本**，不写「正常」两个字）
- 三门禁：`cargo test --workspace` / `check-invariants.sh --all` / `build-wasm.sh` 全绿
  —— **`build-wasm.sh` 这条别漏**：新 provider 会进 wasm 产物，编不过就是把
  [170](170-pages-workflow.md) 的 demo 一起搞挂了
- 不过的条目**照实记**，不许含糊过去

## 完成后

回 [../ROADMAP.md](../ROADMAP.md) §二补一段，并把 [165](165-launch-positioning-decision.md)
L1 的「拉新前置」标记为已兑现。
