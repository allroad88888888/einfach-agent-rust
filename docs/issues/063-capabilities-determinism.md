# 063 红线 11 的字节确定性锁（独测，与 062 并行）

**里程碑** M10 · **依赖** 061（协议形状） · **模型** opus · **独测** ✅ **本 issue 就是那个独测**
· **状态：完成**（2026-08-04，实做记录见文末）

M10 唯一的**静默失败点**：客户端第一次能影响 prompt 字节，排序错了**不报错、功能正常**，
只是每一轮都全价（DeepSeek v4-pro 上 120 倍的钱，红线 11 原文）。

**与 [062](062-capabilities-assembly.md) 并行做**——照本仓「接口先定 → 实现与测试并行 →
合并」的既有模式（043 的注意）。协议形状由 061 定死，你不必等 062 写完。

## 要钉死的三条

1. **同一份声明两次渲染 → 工具表字节完全相同。**
2. **打乱声明数组的顺序再渲染 → 字节仍然完全相同。**
   （实现必须内部按名字排序；客户端的数组顺序**不可以**泄漏进 prompt 字节。）
3. **注入排在表尾、前缀不动**：拿一个**不带声明**的会话的工具表做基线，
   带声明那个的前 N 项与基线**逐项字节相同**——所有会话共有的那一段不因为某个客户端
   注入了东西而移位。

## 验收（可判定）

- 三条断言各有测试，**全部作用在真正进 prompt 的那份字节上**（不是对 `Vec<ToolSpec>` 的
  `Debug` 比较）。找到工具表被序列化进请求的那一点（`wire/tools.rs` 的 `build`，
  以及前缀镜像 `SegmentBytes.tools` 的 `canonical`），断言那里的字节。
- **突变验证（必须做，贴真实红/绿输出）**：把实现里的排序那行删掉/改成保留客户端顺序 →
  **第 2 条立刻红** → 改回 → 绿。照 051（`a_shuffled_node_order_renders_to_the_very_same_bytes`）
  与 052/053 的先例。**没红过的确定性测试不算护栏。**
- skill 注入的索引顺序同理（若 064 已合入则一并覆盖；否则留 TODO 指向 064）。

## 注意

- **`serde_json::Map` 是 `BTreeMap`**（根 `Cargo.toml` 显式不开 `preserve_order`），
  所以 `schema` 的 key 序天然确定——**但别假设**，也写一条断言（现有
  `value/tool.rs` 里有同款实检可参考）。
- **不要改实现去迁就测试**。若发现 062 的实现确实不确定，**报告**并让 062 改，
  你只负责把它变成会红的东西。
- **不要碰** `crates/agent-tools/`。
- 红线 9：≤300 行；测试文件超了就按场景拆成多个 `tests/*.rs`。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（2026-08-04，062 合入之后）

### 结论先行：两条路**是同一份字节**，而且现在有东西看着它

`wire/tools.rs` 的 `build` 一次产出 `Built.value`，三家的 `encode` 拿它**用两次**：
前缀镜像 `canonical(&built.value)`、请求体 `built.value.clone()`——
`deepseek/encode.rs:58` 与 `:112`、`glm/encode.rs:50` 与 `:102`、`kimi/encode.rs:48` 与
`:116`。同一个 `Value`、同一个 `serde_json::to_vec`，**没有第二条序列化路径**，
所以工具顺序不可能在两条路之间分叉。

**这条是「今天成立」，不是结构上不可能**：谁想给镜像单独加一步处理（重排、裁剪、
去掉 description……），改一行就能让它分叉，而症状是红线 11 的经典形态——请求体一切正常，
只有账单不对。所以没有把它当白拿的结论写进注释了事，而是变成一条会红的断言：
`the_prefix_mirror_hashes_exactly_the_bytes_that_go_on_the_wire`。

### 三条断言落在哪

062 已经在**假上游收到的请求体**上覆盖了 wire 那一侧
（`crates/agent-server/tests/http_capabilities_scoped_to_one_session.rs`），
所以本 issue 新加的全部钉在**前缀镜像**那一侧（`Encoded::prefix` 的 Tools 段，
`SegmentImage { bytes, hash }`），顺带把 wire 那一段原始字节一起断上：

| # | 断言 | 文件 |
|---|---|---|
| 1 | 同一份声明装两次表、渲染两次 → 镜像与 wire 字节全等 | `crates/agent-runtime/tests/host_tools_prefix_is_byte_deterministic.rs` |
| 2 | 声明数组倒序 / 轮转 5 位 → 字节全等，且 `drift != Some(Tools)` | 同上 |
| 3 | 不带声明的表做基线，带声明的前 N 项逐项字节相同 + 整段字节前缀 | `crates/agent-runtime/tests/host_tools_prefix_head_never_moves.rs` |
| tie | 镜像哈希的正是 wire 上那一段（长度 **和** 哈希都比） | `..._is_byte_deterministic.rs` |
| key 序 | schema 的 key 序在文本里换一换 → 字节不变，且把那一项字节原样钉死 | 同上 |

共用夹具在 `crates/agent-runtime/tests/host_tools_bytes_support/mod.rs`。三条都**对
deepseek/glm/kimi 各跑一遍**——镜像那段字节在三个 `encode.rs` 里各写了一遍，只测一家
等于放着另外两家不管。

几个构造上的判断：

- **落在 `agent-runtime` 的集成测试**：这条链两头分别是
  `ToolTable::with_host_tools`（agent-runtime）和 `Provider::encode`（agent-providers），
  只有这个 crate 同时看得见两边。
- **第 2 条先断 `drift`**：镜像不等只是形态，`drift == Some(Tools)` 才是那笔钱——
  宿主重连时把同一份声明按另一个顺序报上来，就该判前缀没变。
- **tie 那条必须比哈希**：`wire::prefix::hash` 是 `pub(crate)`，测试里照 `DefaultHasher`
  复制了一份同款。只比长度不够——「镜像那边把数组倒过来」这种改法长度一个字节不差
  （突变 D 实测：长度断言通过，哈希断言才红）。哪天 `wire/prefix.rs` 换哈希算法，
  这条会红，改这里跟上即可。
- **声明从 JSON 文本解**、不用 `json!` 宏造 `Value`：客户端交上来的就是文本，
  schema 的 key 序确定性只有从文本进来才测得到。
- **夹具 12 个工具**而不是 2 个：突变 B（换 `HashMap`）在 2 个元素上有一半概率碰巧同序。

### 突变验证（五条，每条都真红过、都改回来了）

| 突变 | 改哪一行 | 红的是谁 | 红色输出 |
|---|---|---|---|
| A | `tool_table_host.rs:59` 删掉 `sort_by` | 第 2 条 + 第 3 条的「表尾按名字排序」 | `shuffling_...`：`assertion left != right failed: deepseek/倒序：同一份声明换个数组顺序就被判成前缀漂了…… left: Some(Tools) right: Some(Tools)`；`an_injection_...`：`注入的那一段没按名字排序 left: ["web:crm/lookup", "desk:clipboard/write", …]` |
| B | 同一处换成 `HashMap` 迭代 | 第 1 条 + 第 2 条 | `the_same_declaration_...`：`前缀镜像的 Tools 段不一样 left: SegmentImage { segment: Tools, bytes: 4074, hash: 16282260031076374679 } right: SegmentImage { …, bytes: 4074, hash: 8042776674797360869 }`（**字节数一样、哈希不同**——顺序泄漏的标准签名） |
| C | `tool_table_host.rs:62` `push` → `insert(0, ..)` | 第 3 条 | `共有那一段的第 0 项被注入挤动了：字节从第 28 位起就不一样了（左 918 字节 / 右 129 字节）` |
| D | `deepseek/encode.rs:58` 镜像另算一份（把数组倒过来） | 只有 tie | `前缀镜像哈希的不是 wire 上那一段字节——两条路分叉了 left: 13403176452419038593 right: 8372582116628578840`；**长度那条断言是通过的** |
| E | 根 `Cargo.toml` 给 `serde_json` 开 `preserve_order` | 只有 key 序那条 | `声明渲染出来的字节不是按 key 字典序排的 left: {"type":"function","function":{"name":…,"description":…}} right: {"function":{"description":…,"name":…},"type":"function"}` |

突变 E 在 `scratchpad` 里的**整份 workspace 副本**上做（`rsync crates + Cargo.toml`），
没动共享工作树的 `Cargo.toml`/`Cargo.lock`——当时另外两个 agent 正在同一棵树上跑
cargo，改根 manifest 会连累它们。A–D 是就地改、当场改回；收工 `grep -rn "MUTATION"
crates/` 确认干净（本仓出过 `guard.rs` 留 `// MUTATION:` 残留的事故）。

### 没发现真的不确定性

062 的实现这一侧是干净的：排序在 `with_host_tools` 里、追加在表尾、可逆性另挂
`BTreeMap`，镜像与 wire 同源。**一行实现都没改**（唯一的代码改动是测试自己的
clippy：`&Vec<u8>` → `&[u8]`）。

### 两个坑

1. **突变 A 第一次跑出来的失败信息滚了两屏**——一张真实工具表 4000 多字节，
   `assert_eq!` 两大坨字符串出来根本看不出哪儿变了。于是加了
   `assert_same_bytes`（只打第一处分歧前后各 60 字节）并把**镜像那条断言排在字节断言
   之前**（`SegmentImage` 的 `Debug` 一行就说清了「长度一样、哈希不同」）。
   护栏会不会红是一回事，红了能不能读是另一回事。
2. **收工跑全量时 `session_store_jsonl_crash_recovery` 红过一次，单跑与复跑都绿**
   ——当时另一个 agent 正在改 `agent-mcp`、整棵树在重编译，判定是并发下的偶发。
   记在这里是免得下一个人看见它红就以为是自己的改动。
   同一时段还撞上过 `agent-mcp` 编译不过（别人的在飞改动），等它绿了才继续。
3. **`cargo clippy --all-targets -- -D warnings` 现在在 `crates/agent-tools` 上是红的**
   （`tests/perf_fs_read.rs:50/65` 的 `needless_borrow`），那个 crate **没有任何未提交
   改动**——是既有提交里带进来的，且 063 明令不许碰它。谁下一个动 `agent-tools` 顺手修。
