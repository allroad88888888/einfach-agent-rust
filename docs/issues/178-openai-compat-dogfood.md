# 178 OpenAI 兼容真机收官

**里程碑** L · **依赖** [177](177-openai-compat-config.md) · **模型** sonnet · **估时** 20min · **状态** 完成（2026-08-13，八条全过 + 抓到一个真 bug）

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

---

## 实做记录（2026-08-13）

`providers.toml` 里加三段（同端点、同 key、`adapter = "openai"`），用
`printf ... | cargo run -p agent-cli` 非交互驱动 CLI，逐条跑。验收完成后那几段已删——
它们是本地验收用的，`providers.toml` 本来也 gitignore。

### 第一发就抓到一个真 bug，而这正是真机的意义

`/model ds-generic` 报 **「没有对应的 adapter」**。

原因：[177](177-openai-compat-config.md) 加 `adapter` 字段时**只改了 `main.rs` 的启动
路径**，`model_switch.rs` 的 `/model <name>` 仍然拿**段名**去查 adapter 表。于是
`adapter = "openai"` 的段永远切不过去。

**177 的单测为什么没抓到**：它测的是 `adapter_name()` **函数本身**（回落、覆盖、
未知名字报错都测了），测不到「**调用方有没有用它**」。这类「新函数写对了但某个调用点
没改」的洞，单测天然看不见——它需要端到端或真机。

修法：`model_switch.rs` 改走 `provider::adapter_name(name, provider_cfg)`，并**补了一条
能钉住调用方的回归测试**：夹具里加一段 `[providers.my-gateway]`（段名**不是**任何合法
adapter 名，靠 `adapter = "openai"` 指过去），测试先断言 `build_provider("my-gateway")`
确实报错（否则这条测试测了个寂寞），再断言 `switch()` 成功。

### 八条逐条结果

| # | 验的什么 | 结果 |
|---|---|---|
| 1 | 通用 adapter 打 DeepSeek | ✅ `PONG`，`endpoint=/v1/chat/completions`，缓存 6656/6696 = **99%**，`adjustments: 无` |
| 2 | 打 GLM（**base_url 无 `/v1`**） | ✅ `PONG`，`endpoint=/api/paas/v4/chat/completions`，`cached=0` |
| 3 | **打 Kimi**（决策二的关键） | ✅ `PONG`，**没有 400** —— 不发 `temperature` 就撞不上「只收 1」 |
| 4 | 工具调用 | ✅ 模型主动读 `README.md`，答出第一行 `# einfach-agent`，两跳缓存都命中 6656 |
| 5 | 流式 | ✅ 三家都正常出字（含 thinking 段），无卡死无吞字 |
| 6 | 缓存字段读得对 | ✅ 见下 |
| 7 | 401 | ✅ `[本轮失败: Provider(Auth)]`，错误体原文可见，**没有重试** |
| 8 | undo | ✅ 撤两轮后中性追问 →「No, you never told me a passphrase. This is our only exchange so far.」 |

### 第 6 条在真机上拿到了三种形态，正好是 [198](198-missing-cache-field-guard.md) 守的那条区分

| | `cached` | 含义 |
|---|---|---|
| DeepSeek | `6656` | 真命中 |
| GLM | `0` | **确定没命中**（这家显式给 0） |
| Kimi | `None（这家没报）` | **不知道**（这家字段整个缺失） |

[198](198-missing-cache-field-guard.md) 用假响应钉的「缺失 ≠ 0」，在真机上是 Kimi 与 GLM
的真实差别，而且 CLI 的输出把两者**印成了不同的字**。要是当初读成 `Some(0)`，
这里会显示成跟 GLM 一样的 `0`，[024](024-cache-guard.md) 第 2 层从此在 Kimi 上永远
「完美吻合」——**这就是那条静默失效的真实长相**。

### 三门禁 + 红线 12

`cargo test --workspace` / `check-invariants.sh --all` / `build-wasm.sh` 全绿，
外加 `clippy -D warnings` 与 ts feature。**`git diff --stat crates/agent-core/` 空** ——
整条 OpenAI 兼容链（[174](174-openai-compat-probe.md)–178 + [198](198-missing-cache-field-guard.md)）
下来，core 一个字节没动。

### 之后

[165](165-launch-positioning-decision.md) L1 的「OpenAI 兼容是拉新前置」到此兑现：
配置面通了、三家真机过了、静默失效有单测守着。
