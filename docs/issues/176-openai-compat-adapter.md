# 176 OpenAI 兼容 adapter 实现

**里程碑** L · **依赖** [175](175-openai-compat-decision.md) · **模型** sonnet · **独测** ✅ · **估时** 20min · **状态** 待开始

## 目标

按 [175](175-openai-compat-decision.md) 拍的案子写代码。**只写 adapter 与 wire，不碰配置面**
（配置在 [177](177-openai-compat-config.md)）——拆开是为了让这一步能被录制帧独测覆盖，
不牵扯文件 IO。

## 做什么

按 [175](175-openai-compat-decision.md) 的结论走。无论 A 案 B 案，这几条都要满足：

1. 实现在 `crates/agent-providers/`，**不新增 HTTP 依赖**——那个 crate 依赖只有
   `agent-core` + serde，IO 全在 `agent-transport`（决策 26 里核实过的事实，别破坏它，
   wasm 目标靠它活着）。
2. 流式分片按 [174](174-openai-compat-probe.md) 的实测形状解析，尤其 `tool_calls` 的分片。
3. usage / 缓存字段按实测填；**没有缓存字段的家要能优雅地报「不知道」**，
   不能填 0 冒充命中——那会让 [024](024-cache-guard.md) 的三层兜底得到假绿。
4. 做不到的意图**报 `Adjustment`**，不要静默降级（决策 17）。

## 验收

- **录制帧独测全绿、零网络**（照 [025](025-provider-seam.md) 的形状）：
  至少覆盖普通回复、流式、工具调用、401 报错四条
- `cargo test --workspace` 绿
- `check-invariants.sh --all` 退出码 0
- **红线 12 自查**：`git diff crates/agent-core/` 为空。core 一个字节都不该动——
  动了就说明 [175](175-openai-compat-decision.md) 的结论有问题，回去重定，别在这里将就

## 不做

- 不改 `providers.example.toml`（[177](177-openai-compat-config.md) 做）
- 不跑真机（[178](178-openai-compat-dogfood.md) 做）
