# 180 crates.io 名字查重与取名

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** sonnet · **估时** 15min · **状态** 待开始

## 目标

在做任何发布准备之前，先确认**名字拿得到**。`agent-store` 这种通用词在 crates.io 上
大概率已被占——先查清楚，别等 [181](181-store-publish-prep.md) 做完了才发现要改名，
那时候改要连带动 README、文档链接和所有 `use` 路径。

## 做什么

1. 查这几个名字的占用情况：`agent-store`、`einfach-store`、`einfach-atom`、
   `agent-atoms`（`cargo search` 或直接看 crates.io）。
2. 如果 `agent-store` 被占：**倾向 `einfach-` 前缀**——它有血缘依据
   （上游是 `einfach-core`，见 [../ARCHITECTURE.md](../ARCHITECTURE.md) 与 CLAUDE.md §上游血缘），
   不是硬编的商标词，而且天然给后面可能发的其他 crate 留了命名空间。
3. **顺带查主 crate 名**：将来要不要发 `einfach-agent` 本体？现在只是查，不占。

## 验收

- 每个候选名有明确的「占用 / 可用」结论
- 定下 [181](181-store-publish-prep.md) 要用的名字**和理由**
- 如果要改名，列出改名波及的文件清单（`Cargo.toml` 的 members / 各处 `use` /
  文档里的 crate 名引用），交给 [181](181-store-publish-prep.md)

## 注意

**别在这一步真的注册占坑**。crates.io 不欢迎 name squatting，
而且发布是不可逆的（版本不能删只能 yank）。查清楚、定下来，
真发布在 [182](182-store-publish.md)，由用户执行。
