# 126 把 vision 的纯逻辑从 IO 里摘出来

**里程碑** M14 · **依赖** 无 · **模型** sonnet · **独测** 是 · **状态** 完成

## 目标

`agent-tools/src/vision_inspect.rs` 里三段**纯逻辑**今天埋在同步 IO 函数里。
浏览器侧要复用它们（[127](127-agent-host-inspect-image.md)），但**不能复用它们外面
那层 `std::fs` + 阻塞 `Client`**。

摘出来，native 侧行为逐字不变。

**无依赖，第一天就能开工。这条存在的唯一理由是
[119](119-browser-host-capability-decision.md) §八「native 可测优先」**
——摘出来之后 Kimi 的请求体形状就能在 `cargo test --workspace` 里钉死，
不必留到浏览器真机去看。

## 做什么

三段，都在 `vision_inspect.rs` 里，都是私有的：

| 现在 | 摘成 | 纯在哪 |
|---|---|---|
| `chat_completion` 里那个 `json!({...})` | `pub fn chat_body(model, file_ref, question) -> Value` | 无 IO、无时钟、无随机 |
| `parse_content(&str)` | 提 `pub` | 已经是纯函数 |
| `upload` 里那个 mime → 扩展名 `match` | `pub fn extension_for(mime) -> &'static str` | 同上 |

`inspect()` 与 `chat_completion()` 改成调这三个，**行为逐字不变**。

放哪：由实现者定。`vision_inspect.rs` 今天 487 行（含测试）、约 300 行实现，
再塞三个 `pub fn` 大概率顶破上限——**拆分是本次改动的一部分**（红线 9）。
建议切法：「Kimi 的线格式」与「按链接取字节 + 发请求」是两件事。

## 验收

- `cargo test --workspace` 全绿。现有的
  `end_to_end_uploads_bytes_then_chats_with_ms_reference` **一个字符都不改**
  仍然过——这是「行为逐字不变」的主证据。
- **新增 native 单测**（这条 issue 的产出）：
  - `chat_body` 对固定入参**逐字节确定**：转 1000 次序列化结果相同；
    且 `content[0]` 是 `image_url`、`content[1]` 是 `text`，**顺序定死**
    （Kimi 那边对顺序敏不敏感没验过，我们自己先别漂）。
  - `parse_content` 的四条路：正常、缺 `choices`、缺 `content`、不是合法 JSON——
    各自落到正确的错误码。
  - `extension_for` 覆盖四种 mime + 兜底 `bin`。
- `cargo check --target wasm32-unknown-unknown -p agent-tools` 过
  （摘出来的东西不能引入任何 native-only 依赖）。

## 注意

- **只摘，不改**。`chat_body` 产出的 JSON 必须跟今天 `chat_completion` 里那个
  字面量逐字节相同，包括字段顺序。这条 issue 里任何「顺手优化」都是在改一个
  已经真机验过的请求体。
- `serde_json` 在本仓**不开 `preserve_order`**（`Map` 是 BTreeMap 后端），
  所以 `json!` 出来的对象键序是字典序、稳定的。**别为了"保证顺序"去引 IndexMap**。
- 摘出来的三个 `pub fn` 会成为跨 crate 契约（`agent-wasm` 要用）。
  文档注释里写清楚它们是纯函数、以及为什么 `chat_body` 的字段顺序不能动。

## 实做记录

**拆法**：`vision_inspect.rs`（487 行）按「Kimi 的线格式」vs「按链接取字节 +
发请求」一切二，外加把两处测试各自挪进同名 `_tests.rs`（本 crate 既有惯例，
见 `apply_patch_spec.rs` / `apply_patch_spec_tests.rs`），最终五个文件：

| 文件 | 行数 | 职责 |
|---|---|---|
| `vision_inspect.rs` | 201 | 声明 + 编排：`VisionRuntime`/`VisionLinkSource`/`vision_inspect_spec`/`inspect()`/`parse_input`；模块文档（安全模型 + 131 的措辞订正） |
| `vision_inspect_tests.rs` | 182 | 原有测试**逐字节原样迁移**（用 `sed` 提取+去缩进，不是手抄，杜绝转写误差） |
| `vision_kimi_wire.rs` | 66 | issue 验收三件套：`chat_body`/`parse_content`/`extension_for`，纯函数 |
| `vision_kimi_wire_tests.rs` | 149 | 独立测试 agent 产出（见下） |
| `vision_source.rs` | 140 | 按链接取字节（`resolve_bytes`/`read_uploaded*`/`mime_from_path`）+ 发请求（`upload`/`chat_completion`），native-only IO |

`lib.rs` 新增 `mod vision_kimi_wire;` `mod vision_source;` 两行 + 三个转发
`pub fn`（`chat_body`/`parse_content`/`extension_for`，跟既有 `vision_inspect_spec`
同一个转发惯例），一共 +23/-0 行。

**独测怎么派的**：`vision_kimi_wire.rs` 写完接口（三个 `pub fn` 签名 + doc
注释，不含调用方）后，派了一个独立 general-purpose agent（sonnet），只给它三
份材料——签名+doc注释（无函数体）、issue 验收三条、INVARIANTS.md 红线 11 原文
——让它在不读 `vision_kimi_wire.rs`/`vision_source.rs`/`vision_inspect.rs`
实现体的前提下写 `vision_kimi_wire_tests.rs`。产出 12 个测试：`chat_body` 三个
（形状+数组顺序、1000 次序列化确定性、特殊字符转义往返）、`parse_content` 七个
（验收要求的四条路 + 三个边界：空 `choices` 数组、`content` 非字符串、空输入）、
`extension_for` 两个（四种已知 mime、三种回退到 `bin`）。全部通过，我 review 过
一遍确认没有读到实现体的痕迹（不是照抄 match 分支写的断言）。

**只摘不改的证据**：`end_to_end_uploads_bytes_then_chats_with_ms_reference`
一个字符没动（`sed` 机械迁移，非手工转写），依然绿——这是「`chat_body` 产出
的 JSON 跟迁移前字面量逐字节相同」的主证据（129 行的 e2e 测试仍走真实
`inspect()` → `vision_source::chat_completion` → `vision_kimi_wire::chat_body`
这条完整链路，不是绕过实现直接测 `chat_body`）。

**顺带做了 131 在 `vision_inspect.rs` 里的那一半**（131 收工时明确交回，
「这两句只有 126 改 vision_inspect.rs 时才有地方落笔」）：模块文档改了
安全模型那一段——

1. 把「不进任何持久化」删掉，换成「这条承诺的边界必须说清楚：它管的是
   『字节不进模型上下文』……不是『字节不落盘』」，并给了三种形态的字节
   落点对照表（server 上传目录 / CLI 本机文件 / 浏览器 IndexedDB）。
2. 留了一句「这条边界曾经写得更绝对……别把它改回去」，引用
   `docs/issues/099-send-plan.md` §「主会话复核修正的一处」作为同类教训。
   **故意没有逐字引用旧措辞**（没有把「不进任何持久化」这个精确串写进新文字
   里）——否则会让 131 的验收命令 `rg "不进任何持久化" docs/ crates/` 在我
   自己的订正文字里产生一次新命中，自相矛盾。
   落点：`crates/agent-tools/src/vision_inspect.rs` 第 13–40 行（模块文档
   「安全模型」一节）。
3. 复核：`rg "不进任何持久化" crates/agent-tools/` 现在零命中；全仓范围
   （`docs/ crates/`）只剩 131 自己的 issue 文件、119 §六（订正后的准确引用）、
   `docs/issues/README.md` 的索引行——三处都是 131 已经判定过的「引用/已订正」，
   不是断言位置。

**最终验证**（前台跑完）：

```
$ cargo test --workspace 2>&1 | grep -E "FAILED|^error"
（无输出，全绿；workspace 中途撞过一次 agent-runtime 的瞬时失败，
 系并行跑活的另一个 agent 改动导致，单独重跑 cargo test -p agent-runtime --test it
 复现为全绿，与本改动无关）

$ cargo check --target wasm32-unknown-unknown -p agent-tools
Finished `dev` profile [unoptimized + debuginfo] target(s)（1 条 pre-existing
警告，workspace_process_lock.rs 的 dead_code，与本改动无关）

$ bash scripts/check-invariants.sh --all
exit=0；vision_inspect.rs 的红线 9 违规消失，五个新/改文件全部
＜300 行（201/182/66/149/140），无新增警告

$ cargo clippy -p agent-tools --all-targets -- -D warnings
Finished（clean；顺手修了 vision_inspect_tests.rs 迁移带来的一个
`useless_format!` pre-existing 警告，验证过原文件在改动前就有这条，
不是本次引入）
```

**没做到的部分**：无。issue 验收四条全部满足；131 交回的两句措辞订正已经
落进 `vision_inspect.rs`。131 自己范围内的「抄到别处」核查（`docs/HOST-
CAPABILITIES.md`/`docs/IMAGES.md`/其余 issue 文件/`uploads.rs`）由 131 的
agent 完成，不在本条职责内，未重复核实。
