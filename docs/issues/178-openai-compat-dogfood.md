# 178 OpenAI 兼容真机收官

**里程碑** L · **依赖** [177](177-openai-compat-config.md) · **模型** sonnet · **估时** 20min · **状态** 待开始

## 目标

真 provider 跑一轮，不是 mock。**本仓每个里程碑都以真机 dogfood 收尾**
（M9–M18 无一例外），这条不能因为「只是加个 adapter」就破例——
录制帧独测保证的是「解析对了」，保证不了「真的能用」。

## 拿什么当被测端点

**用已有的三家**（2026-08-13 修订）。原案是「装 Ollama 打本地一发」，
**撤销**——这个项目跟 Ollama 没关系，为了跑一条验收去引一个外部运行时，
是把测试需求变成了产品依赖。

而且用三家其实是**更好的 dogfood**：它们既是真实的 OpenAI 兼容端点，
又已经各有一个特化 adapter。于是能做一件 Ollama 做不到的事——
**同一个端点、同一份料，特化 adapter vs 通用 adapter，两边对着跑**。
差异全部归因于 adapter 本身，不掺服务端差异。

`adapter = "openai"` 指过去就行（[177](177-openai-compat-config.md) 的字段），
`providers.toml` 里加一段：

```toml
[providers.deepseek-generic]
adapter = "openai"                        # 同一个端点，走通用编解码
api_key_env = "..."                       # 跟 deepseek 段同一把 key
base_url = "https://api.deepseek.com/v1"  # 注意带 /v1
model = "deepseek-v4-flash"
```

## 做什么

| # | 验的什么 | 怎么算过 |
|---|---|---|
| 1 | 通用 adapter 打 DeepSeek 一轮对话 | 有回复，`Adjustment` 为空 |
| 2 | 通用 adapter 打 GLM 一轮 | 同上。**注意 base_url 不带 `/v1`**（[174](174-openai-compat-probe.md) 结论一） |
| 3 | 通用 adapter 打 Kimi 一轮 | 有回复——这条专门验 [175](175-openai-compat-decision.md) 决策二：不发 `temperature` 就不会撞上「只收 1.0」 |
| 4 | 工具调用 | 模型主动调 `srv:fs/read` 读一个文件并据此回答 |
| 5 | 流式 | 分片正常，无卡死无吞字 |
| 6 | **缓存字段读得对** | 第二轮 `cached` 是真数字，且跟特化 adapter 读到的**一致**（[174](174-openai-compat-probe.md) 实测 DeepSeek 两条路径数值相同，这里验代码真的走通了） |
| 7 | 401 | 填错 key 被分类成 `Failed(Provider(Auth))` |
| 8 | undo | 通用 adapter 上 `/undo` 行为与三家一致 |

## 「缓存字段缺失」那条隐患：**不放这里，走单测**

[174](174-openai-compat-probe.md) 唯一没验掉的是——**一家什么缓存字段都不给时，
`stream/usage.rs` 会读成 `None`（不知道）还是 `Some(0)`（确定没命中）**。
读成 0 的后果是静默的：[024](024-cache-guard.md) 的第 2 层每轮都得到
「预测 0、实际 0、完美吻合」，从此**永远不告警**。

三家都给了字段，所以真机上碰不到这条。**但它不需要真机**：
写一个不返回缓存字段的假端点就能钉死，而且**能进 CI 永久守着**，
比一次性真机验证更值钱。

→ 落 [198](198-missing-cache-field-guard.md)，不占本 issue。

## 验收

- 八条逐条记录进本 issue 的实做记录（照 [163](163-m18-dogfood.md) 的写法：
  **记下真实数字和真实报错文本**，不写「正常」两个字）
- 三门禁：`cargo test --workspace` / `check-invariants.sh --all` /
  `build-wasm.sh` 全绿——**`build-wasm.sh` 别漏**：新 provider 会进 wasm 产物，
  编不过就是把 [170](170-pages-workflow.md) 的 demo 一起搞挂了
- 不过的条目**照实记**，不许含糊过去

## 完成后

回 [../ROADMAP.md](../ROADMAP.md) §二补一段，并把
[165](165-launch-positioning-decision.md) L1 的「拉新前置」标记为已兑现。
