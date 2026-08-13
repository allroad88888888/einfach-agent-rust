# 187 文章：两天把 target 堆到 58GB

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** sonnet · **估时** 20min · **状态** 待开始

## 目标

短文，讲那次构建事故。**每个 Rust 开发者都中过这个坑的某个版本**，
所以传播成本极低——它不需要读者关心你的项目，只需要他们关心自己的构建速度。

**用 sonnet 而不是 opus**：素材具体、结论明确、篇幅短，是执行活。

## 素材

- CLAUDE.md §Workspace：「267 个测试文件曾两天把 target 堆到 58GB/88 万文件，
  2026-08-05 已合并为每 crate 一个 harness」
- `Cargo.toml` 里 `[profile.dev] debug = "line-tables-only"` 那段注释
  （已经写得很好了，基本可以直接改写）

## 写什么

标题方向：*267 test files, 58GB of target/, and rustc spending minutes enumerating directories*

1. **症状**：构建越来越慢，慢到分钟级，但代码没变多少
2. **诊断**：`tests/` 顶层每个 `.rs` 都是**独立链接的二进制**。267 个文件 = 267 个二进制，
   每个都链接整个依赖树
3. **第二个因素**：全量类型调试信息是体积大头；文件数一过几十万，
   rustc 每次启动枚举 deps 目录本身就是分钟级——**构建自己拖慢自己**
4. **两个修法**：
   - 每 crate 一个 `tests/it/main.rs` harness，新测试加一行 `mod`
   - `[profile.dev] debug = "line-tables-only"` + 第三方依赖 `debug = false`
     （panic 栈里它们的帧有函数名就够）
5. 数字对比：前后的 target 体积、文件数、构建时间

## 验收

- 有**真实的前后数字**——没有数字这篇就不值得发
- 两个修法都给出可复制的配置片段
- 短。这篇的优势就是短，别写长

## 顺带

如果发之前能补测一次当前的 target 体积/构建时间，数字更有说服力。
