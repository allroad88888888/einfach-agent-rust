# 177 OpenAI 兼容的配置面

**里程碑** L · **依赖** [176](176-openai-compat-adapter.md) · **模型** sonnet · **独测** ✅ · **估时** 20min · **状态** 完成（2026-08-13）

## 目标

让人**能填得进去**。[176](176-openai-compat-adapter.md) 写完 adapter 之后，
如果配置面不通，对外等于不存在。

这个 issue 的读者是**第一次 clone 仓库的陌生人**——`providers.example.toml` 是他们
接触这个项目的第二个文件（第一个是 README）。它写得清不清楚，直接决定漏斗到这里断不断。

## 做什么

1. `providers.toml` 的解析加新段，形状跟既有三家对齐（`api_key` / `base_url` / `model`）。
2. `providers.example.toml` 加**注释充分**的示例段——现有三家那几段的注释密度就是标杆
   （它们连 `beta_base_url` 为什么存在都写了）。例子给 OpenAI 官方 + OpenRouter
   两个，并**显式标注「本仓未跑过真机」**：字段形状照规范写，但没验过就不能让它
   看起来像验过的。
   （原案第一个例子是 Ollama，2026-08-13 撤销——理由见下面实做记录 §example 里的例子。）
3. 校验：`base_url` 缺失要**明确报错**，不要静默退默认。
   判据照决策 32 那条既有取舍：**有没有下游替它报错**——这里没有，所以硬失败。

## 验收

- 单测：三种配置各能解析成预期结构；缺 `base_url` 报错且**点名是哪一段**
- `providers.example.toml` 的例子标注了「未跑过真机」，不冒充已验证
- `cargo test --workspace` 绿

## 注意

`providers.toml` 已 gitignore，改的是 `providers.example.toml`。
别把任何真 key 写进 example——**提交前 grep 一遍**。

---

## 实做记录（2026-08-13）

### 核心是一个 issue 里没写的设计问题：**怎么说「这个端点用通用 adapter」**

原计划是「`providers.toml` 加新段」。动手才发现不成立：**分发是按段名做的**
（`build_provider("deepseek")`），那么通用 adapter 只能有一个段叫 `openai`
——**想同时配 Ollama 和 OpenRouter，第二个没处放**。而「同时接多个兼容端点」
恰恰是这个 adapter 存在的全部理由。

解法：`ProviderConfig` 加 `adapter: Option<String>`，**段名与编解码解耦**。

```toml
[providers.ollama]
adapter = "openai"                        # 段名随便叫，编解码走通用那套
base_url = "http://localhost:11434/v1"

[providers.openrouter]
adapter = "openai"                        # 两个段，同一套编解码
base_url = "https://openrouter.ai/api/v1"
```

**缺省 = 段名**（`#[serde(default)]` → `None` → 回落），所以既有的 `providers.toml`
一个字都不用改。判据一句话：**段名是「这个端点叫什么」，`adapter` 是「用哪套编解码」。**

### 改了哪些

1. `agent-transport/provider_config.rs`：加 `adapter` 字段 + `from_host` 补齐
2. `agent-cli/provider.rs`：新增 `adapter_name(section, cfg)`（`adapter` 优先、
   缺省回落段名）；分发表加 `"openai" => OpenAiCompat`
3. `agent-server/provider_dispatch.rs`：同样加一条（三张分发表是 030 有意为之的
   重复，不是疏漏——见那两个文件的模块文档）
4. `agent-cli/main.rs`：启动时走 `adapter_name` 而不是直接用段名
5. `providers.example.toml`：新增一整段，排在三家之前

两处错误文案同步改成「可选：deepseek / kimi / glm / openai …或段内的 adapter 字段」
——配错的人第一反应是去看段名，得告诉他还有第二个地方可能写错。

### example 里的例子

**曾经把 Ollama 写成第一个例子，2026-08-13 撤掉了**（用户拍板：这个项目跟 Ollama
没关系）。原因值得记：我当时的理由是「海外读者手里没有三家的 key，Ollama 零成本」
——但那是**把一个测试/上手需求变成了产品叙事**。einfach-agent 不是一个本地 LLM 项目，
把一个外部运行时摆在 example 第一行，等于替读者选了一条我们既不维护、也没验过的路。

现在留 OpenAI 官方 + OpenRouter 两个例子，并**显式标注「本仓未跑过真机」**——
实测覆盖的是三家的兼容端点（[174](174-openai-compat-probe.md)）。
写一条没验过的配置却让它看起来像验过的，跟 [167](167-readme-stale-mechanism.md)
修掉的那个问题是同一类。

注释里把 [174](174-openai-compat-probe.md)/[175](175-openai-compat-decision.md)
的两条代价写明白了，别让人拿它当默认档：
**给不了确定性采样**；**`base_url` 要带全路径**（`/v1` 不是通用约定，GLM 没有）。

### 顺手修了一条 flaky 测试（不在原计划里）

推 [176](176-openai-compat-adapter.md) 之后 CI 红了，红在
`cancel_endpoint_stops_the_flying_turn`——**同一个 commit 重跑一次就绿**。
是 flaky，不是回归。

`gh run rerun --failed` 是判定 flaky 最便宜的手段，比读日志猜快得多。

修法不是「把超时数字调大」了事，而是先想清楚它在证什么：上游是
`Script::HangAfterHeaders`——**永远不返回**。所以「任何有限时间内收到 Cancelled」
就已经证明了「取消不等 provider」。**窄预算不会让这条测试更严**（对面永远不返回，
不存在「差一点就自己结束了」这种可混淆的情形），**只会让它在负载高的 runner 上变红**。

于是：预算 3s → 30s，并且每次读取拿「剩下多少」而不是各自一份完整预算
（否则一次慢读就吃掉整个窗口，实际生效时长跟写的数字对不上）。断言文案也补上
「上游是 HangAfterHeaders，不取消的话它永远不会结束」——让下一个看到它红的人
立刻知道该怀疑什么。

> **flaky 测试比红测试更毒**：它训练人忽略红色。CI 刚建起来就出现一条，
> 必须当场处理，不能留着「下次再说」。

### 验收

- [x] 单测：`agent-transport` 三条（无 `adapter` 照常解析且为 `None` / 显式
      `adapter` 生效且两个段可共存 / `endpoint()` 不补 `/v1`）；
      `agent-cli` 四条（回落段名 / `adapter` 压过段名且段名本身不是合法 adapter
      名 / 四个 adapter 全可解析 / 未知名字报错且文案提到 `adapter`）
- [x] `providers.example.toml` 的两个例子都标了「未跑过真机」，不冒充已验证
- [x] 六道门全绿（红线 / clippy / test / ts feature / typecheck / build-wasm）
- [x] **红线 12**：`git diff --stat crates/agent-core/` 空
- [x] `grep` 过 example 无真 key

### 留给 [178](178-openai-compat-dogfood.md)

配置面通了，但**一次真实请求都还没发过**。[178](178-openai-compat-dogfood.md)
已按「用已有三家当被测端点」重写——同一个端点、特化 adapter vs 通用 adapter
对着跑，差异全部归因于 adapter 本身。

而「缓存字段缺失」那条隐患已经拆出去单独做掉了（[198](198-missing-cache-field-guard.md)）：
它根本不需要真机，假响应就能钉死，**而且能进 CI 永久守着**——
一次性真机验证证明的是「那天没坏」，单测证明的是「以后坏了会红」。
