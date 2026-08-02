# 013 内置工具最小集

**里程碑** M1 · **依赖** 004 · **模型** sonnet · **独立测试 agent** ✅ · **状态** 完成

## 目标

三个真工具，让 agent 真的能干活而不只是聊天。

## 做什么

| 工具 | `Location` | `Reversibility` | 说明 |
|---|---|---|---|
| `srv:fs/read` | Server | `Pure` | 读文件，带行范围 |
| `srv:fs/list` | Server | `Pure` | 列目录 |

**M1 只做这两个读工具。** `shell/exec` 挪到 [020](020-shell-tool.md)（M2）——
它是 `Irreversible` 的，而 M1 没有 undo 屏障挡不住它。M1 写了它也只能默认关着，
那就等于在仓库里留一段**从没跑过的代码**，是负债。

## 不依赖 loop

工具的实现只需要 `ToolSpec` 类型（已存在），跟 loop 无关。**第一天就能和 001 并行开工**，
只要 004 的上限先定下来。

## 验收

- 两个工具能独立调用并返回正确结果（不依赖 loop，直接调 executor 测）
- 结果超过 004 定的上限时正确截断并带可见标记
- 路径越界被拒绝——**不能读仓库外的文件**
- 工具表的序列化**逐字节稳定**：同一份表编码两次字节相同（红线 11：用 `Vec` 不用 `HashMap`）

## 注意

**红线 11**：工具表进 prompt 最前面，序列化顺序一漂每轮都全价（DeepSeek 上 120 倍）。
用 `Vec` 不用任何无序容器，schema 用 `serde_json::Value`（其 `Map` 默认是 `BTreeMap`，
key 有序）。这是本 issue 要派独立测试 agent 的原因。

## 实做记录（2026-08-01）

实现与测试由两个互相看不见的 agent 并行完成（WORKFLOW §三），合并一次全过：
实现方 34 个内联单测 + 独测方 30 个验收测试（7 个文件），64/64 绿。

- **路径监狱一条路径挡三种逃逸**：`root.join(rel)` 后 canonicalize + `starts_with`
  同时接住 `../`、绝对路径顶替（`Path::join` 语义下绝对路径会替换 root，但监狱
  检查照样打中）、symlink 穿透（canonicalize 先解链再比对）。目标不存在时上溯到
  最近存在的祖先判监狱——**越界判定先于存在性判定**，否则错误码会泄露 root 外
  文件存在与否（独测 agent 主动测了这一点，两边不谋而合）。
- **规格没列举的三个兜底**，都归到已有错误码不新增：权限错误等非 NotFound 的 IO
  失败 → `bad_input`；非 UTF-8 文件 → `bad_input`；`fs/list` 遍历中单个 entry
  读取失败跳过不整体失败。
- `fs/list` 排序按**原始名**排再补目录尾 `/`，保证 `foo.txt` 排在 `foo_dir/`
  前面——先补后排会被 `/` 干扰字典序。
- executor 返回原始输出不截断，截断验证走 `agent_core::truncate_tool_output`
  组合（决策 19：executor 不知道 prompt 预算）。
