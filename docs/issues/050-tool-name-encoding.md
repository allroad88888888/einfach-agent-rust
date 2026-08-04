# 050 工具名 URL 编码泄漏进模型的 tool-name 参数（M7 真机 dogfood 发现）

**里程碑** 待归类（adapter / spawn） · **依赖** — · **模型** opus · **调查 / 决策类**

## 现象（049 的 M7 真浏览器 dogfood 逮到）

真机验收里，模型第一次 `srv:agent/spawn` 传的 `tools` 子集写成 `srv_3Afs_2Flist`，被
spawn 的 `check_subset` 拒（它比对的是规范名 `srv:fs/list`）。模型的思考链暴露原因：
**它看到的工具名就是 URL 编码过的** `srv_3Afs_2Flist`（`:` → `_3A`、`/` → `_2F`）。
OpenAI 系的 `tools[].function.name` 不许 `:`/`/`，adapter 把工具名 sanitize 过。

## 根因

工具名同时是两种东西：

- **函数名**（wire 上 `tools[].function.name`，必须 sanitize 成 `[A-Za-z0-9_-]`）；
- **参数值**（spawn 的 `tools` 子集，以及任何"吃一个工具名"的工具）。

函数调用那条路 encode↔decode 对称：模型调 `srv_3Afs_2Flist` → adapter 解回
`srv:fs/list` → dispatch 正常。但模型把工具名塞进**自由参数**时，adapter 不知道哪个
arg 是工具名、不会解码 → 编码名直达 `check_subset` → 跟规范名不匹配 → 拒绝。

## 设计张力（要定）

- `check_subset` 在 core / runtime，**不该懂 adapter 的编码**（红线 12：core 无模型相关判断）。
- adapter **无法泛化地**知道哪个 arg 是工具名。
- 模型看到的是编码名，让它"用规范名"反直觉。

候选方向（待拍板，opus）：

1. spawn 截获（dispatch，host 侧）对传入工具名做规范化——但编码是 provider-specific，
   host 侧不该懂 provider 的 sanitize 规则。
2. adapter 换一种**对模型友好且可逆**的 sanitize，并在"工具名参数"处解码——仍要先知道
   哪个 arg 是工具名。
3. **core 的工具名本身就 wire-safe**（不用 `:`/`/`，改成 `_` 分段）——最彻底，但动
   TOOLS.md 的 `srv:fs/list` 命名约定，影响面大（日志、CLI 输出、前缀缓存字节、既有测试）。
4. 接受现状：模型自纠有效，不改，只在 spawn 工具的描述里提示"工具名用列表里的原样"。

## 注意

- 真机上模型**自我纠正成功了**（读到 `is_error` 的 tool_result 列出真实工具名后重试对了）
  ——决策 20 的兜底生效，所以这**不是阻塞项**，是一条独立的可用性摩擦。
- 三家 provider 的 sanitize 可能不同（deepseek 实测 `_3A`/`_2F`；kimi/glm 待查）——方向
  3 之外的方案都要 per-provider 考虑。
- 归属：adapter / spawn 的事，跟 M7 可观测无关，单列。
- 复现时间线见 [049 实做记录](049-web-tree.md) 的真机 dogfood 记录（主会话 playwright 驱动，
  deepseek 真实上游）。

---

## 拍板（2026-08-04，opus）

**选方向 ①：宿主侧对「模型写在参数里的工具名」做归一化**，并捎上 ④ 的描述文案
（两件事本来就该一起做，见下文「④ 单独用不成立」）。方向 ②、③ 否掉。

### 一、三家 sanitize 的实测规则：**不是三条规则，是同一条**

issue 写「三家可能不同（kimi/glm 待查）」——查完了，**这个前提不成立**。转义只有
一份实现，住在三家共享的 wire 骨架里：`crates/agent-providers/src/wire/names.rs`。

| | 规则 |
|---|---|
| 可读档 | `[A-Za-z0-9-]` 与 `_` 原样；其余每个**字节**写成 `_XX`（大写十六进制）。`srv:fs/list` → `srv_3Afs_2Flist` |
| 严格档 | `_` 也写成 `_5F`。仅当可读档 `from_wire` 还原不回原名时启用（名字里本来就带 `_3A` 这种「长得像转义」的片段）。`a_3Ab` → `a_5F3Ab` |
| 选档 | `to_wire` 自校验：先出可读档，`from_wire` 一遍对不上就退严格档。同一个解码器吃两档，wire 上不带档位标记 |

三家共用它的**四个调用点**都验过：顶层 `tools` 数组（`wire/tools.rs::one`，
deepseek/glm/kimi 的 `encode` 全走 `tools::build`，含 kimi 的消息级 late tools）、
历史里的 assistant `tool_calls`（`wire/messages.rs`）、`tool_choice` 指定函数
（`deepseek/encode.rs`、`glm/encode.rs`）；解码侧三家的 `accumulator()` 都挂
`with_name_from_wire(wire::names::from_wire)`，非流式走 `wire/decode.rs`。

**没发真实请求**（不必要且费钱）：以上是读代码 + `probes/` 已有记录得出的。顺带一条
要记住的事实——`probes/results/` 里**没有任何冒号/斜杠能过的证据**，
`probes/PROVIDERS.md` 也没探过 `function.name` 的字符集（三家只用过
`get_weather` 这类朴素名）。所以转义是**按代价不对称主动选的保守做法**
（名字被拒 = 400 整轮废掉；转义的代价只是名字长几个字符），不是实测逼出来的。
`names.rs` 的模块注释原文就是这么写的。

**这条对选型的直接影响**：编码不是厂商差异，是「OpenAI 惯例字符集」这一条共同约束的
产物。于是「宿主侧解码要懂 provider 的 sanitize 规则」这个对方向 ① 的原始反对意见
**失效**——它只需要懂**一条**规则，而那条规则本来就住在 `wire/`（三家共享的骨架），
不住任何一家的目录里。

### 二、两条本次新查出来的事实（改变了权重）

1. **模型同时看得见两种拼法，是我们自己喂的。** 工具 `description` 正文是**原样透传**
   的，`srv:agent/collect` 的描述里就写着「先用 srv:agent/status 看谁已经 Done」，
   而函数列表那一栏是 `srv_3Aagent_2Fstatus`。所以模型拿到的是**同一个工具的两个名字**
   ——它写哪个都有出处。这不是模型犯错，是接口自相矛盾。
2. **今天真正「吃工具名」的参数只有一个**：`srv:agent/spawn` 的 `tools`。051 的
   `srv:agent/status` 和 053 的 `srv:agent/collect` 的 `id` 是 **AgentId**（`root/a1`），
   039 的 skill 工具的 `skill` 是 **SkillId**——这两类都从 tool_result / system 索引里
   来，**从不经过 sanitize**，所以它们没有这个问题。清楚这一点之后，「归一化」的落点
   不需要是一张「哪个字段是工具名」的全局注册表，一个**给调用方主动调的纯函数**就够。

### 三、四个方向逐条

| | 改动面 | 红线 11（前缀字节） | 红线 12（core 无模型判断） | 多 provider | 判 |
|---|---|---|---|---|---|
| ① 宿主侧归一化 | `wire_name` 公开 + 新增 `agent-runtime/src/tool_name.rs` + `check_subset` 返回规范名 + 描述一句 | **零**：一个字节都不进 prompt 的变化（描述那句除外，是刻意的一次性变更） | 不碰 core；宿主按名字分流本来就合法（`dispatch.rs` 已经这么干） | **成立**：一条编码规则三家共用；哪天真出现字符集不同的一家，`from_wire` 变成参数传进去 | **选** |
| ② 换一种「对模型友好」的可逆 sanitize | adapter 编码规则 + 仍然要做 ① 的参数侧解码 | 换编码 = 所有既有会话前缀失效一次 | 同 ① | 要先探清三家的字符集才知道换成什么合法——而本次**不许发真实请求** | 否 |
| ③ core 的工具名本身 wire-safe | `docs/TOOLS.md` 命名约定 + 全部工具常量 + `location_of`/`starts_with("mcp:")` 的解析 + 日志/CLI/前端/既有测试/录制帧 | **所有既有会话的前缀失效一次**（工具表在 prompt 最前面，三家实测确认） | 不碰 core 的判断，但把一个纯 wire 约束焊进了 core 的命名约定 | **不成立**——见下 | 否 |
| ④ 只改描述、接受现状 | 一句话 | 零 | 零 | 成立 | 单独用不成立，见下 |

#### ③ 为什么否（这条最值得写清，因为它看起来最彻底）

- **它并不彻底。** `mcp:<server>/<tool>` 的后两段是**第三方给的**，我们管不着。一个
  叫 `do-a/b` 或带空格的 MCP 工具照样要转义。ROADMAP 的 M6 已经把 MCP 工具喂进同一
  张表，所以转义那条代码路径**删不掉**，只是从「所有工具」缩到「某些 MCP 工具」——
  泄漏面变小，泄漏本身还在。**付全价、买部分。**
- **`_` 分段的语法是二义的。** 现存工具里 `read_file` / `search_files` /
  `find_test_lint_commands` 段内就带 `_`，`srv:fs/list` 和一个假想的 `srv:fs_list`
  会同时映到 `srv_fs_list`。TOOLS.md 立命名空间的初衷正是「企业级多来源一定会撞名，
  冲突策略现在定死」——换成二义的分隔符是把这条初衷退掉。
- **有两处按名字解析在跑**：`tool_table::location_of` 的 `split_once(':')` 和
  `dispatch` 的 `tool.starts_with("mcp:")`。③ 要连它们一起改。
- **一次性缓存代价**：工具表在 prompt 最前面（三家实测：改顶层 tools 后冷轮命中全部
  为 0，连做真前缀匹配的 GLM 也是 0）。改名 = 所有既有会话下一轮**全价一次**，
  DeepSeek v4-pro 上是 120 倍那一档的钱。这个代价本身**不是**否掉它的理由（一次性的、
  可控的），否掉它的是上面三条：**付了这一次代价，问题只解决了一部分**。

#### ④ 为什么单独用不成立

issue 原文的措辞是提示模型「工具名用列表里的原样」——但**列表里的原样就是
`srv_3Afs_2Flist`**，而那正是被 `check_subset` 拒的那个。所以这句提示要么是错的，
要么得改成「把 `_3A` 换回 `:`、`_2F` 换回 `/`」——那是**把我们的转义表写进 prompt**
教模型手工解码，既占前缀字节，又把一层实现细节永久固化成模型契约。

代价那一问也答一下：真机上模型确实自纠成功了（决策 20 的兜底生效），所以不是阻塞项。
但「每次靠模型自纠一轮」的代价不是一轮请求那么简单——那次失败的调用和它的
`is_error` 结果**从此原样躺在历史里进以后每一次请求**，一次摩擦要一直付到会话结束。
而且它是**随机的**：模型也可能不重试、也可能改成省掉 `tools`（于是子 agent 拿到比
模型以为的更大的工具集）。为一句话的修复付这个，不划算。

**但 ④ 的那句描述是对的东西，只是要先让它成为真话**——所以它跟 ① 一起落，
描述改成「照抄你工具列表里的那个名字即可」：现在两种拼法都认，这句话不再是空头支票。

### 四、落地了什么

1. `crates/agent-providers/src/lib.rs`：`pub use wire::names as wire_name;`
   把编解码公开出去（只公开 `names` 一个模块，`wire` 其余部分仍是 `pub(crate)`）。
   文档里写清它为什么住在 `wire/`（不是厂商差异）、为什么要公开（参数侧只能由宿主解）、
   以及哪天字符集真分家了该怎么改。
2. **新增** `crates/agent-runtime/src/tool_name.rs`（一个文件一件事：模型写在参数里的
   一个工具名 → 规范全名）。`resolve(given, known)` 的规则是
   **先精确、后解码，且只往 `known` 里已有的名字上映**：
   - 精确优先，是因为 `from_wire` 对少数规范名不是恒等的（真叫 `a_3Ab` 的工具会被解成
     `a:b`）；精确优先保证它永远先被自己接住。
   - 只映到已有名字上，意味着它**造不出**新工具名——「模型瞎编一个工具名」照旧被拒，
     050 修的只是「名字对、拼法是我们自己编码出来的那种」这一格。
   - 二义不可能：`to_wire` 的自校验保证单射，两个不同的规范名不会有同一个 wire 名。
3. `spawn_tool::check_subset` 从 `Result<(), String>` 改成 `Result<Vec<Arc<str>>, String>`
   ——**归一化 + 提权拦截一次做完**。落进 `ChildConfig` 的必须是规范名：子 agent 的
   工具表是拿它去**精确**过滤宿主那张表的（`subagent::tools_for`），wire 名进去 =
   子 agent 一个工具都没有。拒绝文本里**原样回显模型写的那个字符串**（不回显解码结果），
   它要认出自己写错了什么。
4. `tools` 参数的描述：「工具全名」→「照抄你工具列表里的那个名字即可」。
5. 测试：`tool_name` 5 个（规范名恒等 / wire 名解回来含 `mcp:` / 拿 `to_wire` 现算的
   往返 / 编造的名字仍被拒 / 精确优先），`spawn_tool` 3 个（wire 名被认下且归一化 /
   转义拼法不给编造的名字开后门 / **描述里那句提示锁住**）。

**以后还会有更多吃工具名的参数**：它们调 `tool_name::resolve`，传自己那份权威清单。
这个函数不认识「工具表」这个概念，只认识一个 `&[Arc<str>]`，所以谁都用得上。

### 五、留给以后的一条（不在本次范围）

如果哪天真去探了 `function.name` 的字符集、且三家都收 `:` 和 `/`，那么**直接不转义**
是比 ②、③ 都干净的终局：模型看到的就是规范名，`tool_name::resolve` 退化成一次精确
匹配（但**别删**——历史会话里还躺着 wire 名）。代价同样是一次性的前缀失效。
要做就单开一个 probe issue，别夹在别的改动里。
