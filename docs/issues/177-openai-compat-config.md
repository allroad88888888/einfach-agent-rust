# 177 OpenAI 兼容的配置面

**里程碑** L · **依赖** [176](176-openai-compat-adapter.md) · **模型** sonnet · **独测** ✅ · **估时** 20min · **状态** 待开始

## 目标

让人**能填得进去**。[176](176-openai-compat-adapter.md) 写完 adapter 之后，
如果配置面不通，对外等于不存在。

这个 issue 的读者是**第一次 clone 仓库的陌生人**——`providers.example.toml` 是他们
接触这个项目的第二个文件（第一个是 README）。它写得清不清楚，直接决定漏斗到这里断不断。

## 做什么

1. `providers.toml` 的解析加新段，形状跟既有三家对齐（`api_key` / `base_url` / `model`）。
2. `providers.example.toml` 加**注释充分**的示例段——现有三家那几段的注释密度就是标杆
   （它们连 `beta_base_url` 为什么存在都写了）。至少给三个可直接复制的例子：
   - OpenAI 官方
   - **Ollama 本地**（`base_url = "http://localhost:11434/v1"`，`api_key` 随便填）
     —— 这条对「先试试看」的人是零成本入口，**值得放在第一个**
   - OpenRouter 或硅基流动（任选，代表「一把 key 打多家」）
3. 校验：`base_url` 缺失要**明确报错**，不要静默退默认。
   判据照决策 32 那条既有取舍：**有没有下游替它报错**——这里没有，所以硬失败。

## 验收

- 单测：三种配置各能解析成预期结构；缺 `base_url` 报错且**点名是哪一段**
- `providers.example.toml` 里的 Ollama 那段**可直接复制粘贴就能用**（不是伪代码）
- `cargo test --workspace` 绿

## 注意

`providers.toml` 已 gitignore，改的是 `providers.example.toml`。
别把任何真 key 写进 example——**提交前 grep 一遍**。
