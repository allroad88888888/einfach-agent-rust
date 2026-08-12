# 132 M14 真机 dogfood（里程碑终点）

**里程碑** M14 · **依赖** [123](123-host-tool-deadline.md) + [130](130-browser-vision-end-to-end.md) · **模型** opus · **独测** 本条即验收 · **状态** 完成（见文末）

## 目标

一次连贯的浏览器会话，把 M14 两条需求同时用上，并把**已知的几个交界处**逐条踩一遍。

不是「每条 issue 的验收再跑一遍」——那些各自跑过了。这条要找的是
**没有 issue 认领的那些缝**（M12 那次就是这么抓到三个的，见
[docs/issues/README.md](README.md) M12 段）。

## 跑什么

一次会话，按顺序：

1. **通用回调**：页面声明一条自己的工具（不是 vision，比如读 `navigator.language`
   或者一个 `await` 了真实网络请求的），模型调它并用结果回答。
2. **图片**：上传图 → 模型自己调 `web:source/vision` → 答对内容。
3. **追问**：对同一张图再问一次 → 仍然成功（会话级生命周期）。
4. **刷新**：重开同 id → 历史重放 → **工具表逐字节相同** → 图还在 → 再识别一次。
5. **取消**：在一次识图进行中点 `cancel()` → 干净收尾；晚到的结果不改状态。
6. **压缩**：把对话跑到触发第 2 档（M12），确认压缩与 transient-source 不打架。
7. **删除**：`deleteSession` → 库没了 → 同 id 重开是空会话。

## 必须回答的交界处（这条 issue 的真正价值）

以下每一条都**跨了至少两条 issue 的范围**，所以没有单独的负责人：

| # | 问题 | 为什么它可能是错的 |
|---|---|---|
| 1 | 压缩（M12 第 2 档）清掉一条 `web:source/vision` 的 `ToolResult` 之后会怎样 | 那条结果在历史里本来就是 `[transient_source_result_redacted]`，清了等于把占位换成另一个占位。**大概率无害，但没人验过**，而且它同时踩 `SendPlan` 和 transient-source 两套机制 |
| 2 | 一轮里模型调**两条**宿主工具（一条内建、一条页面的） | `drain_host_tools` 每圈消费一个槽，[124](124-transient-source-in-browser.md) 给它加了按名字分流——两类混在同一轮里走过吗 |
| 3 | 识图那一轮的**前缀缓存**实际掉多少 | one-shot 安全重编码会让那轮 `Intentional` 全价。M12 那次实测发现「固定前缀占比高时压缩轮仍有 90% 命中」，**这里的数字也该实测而不是外推** |
| 4 | `deleteSession` 删的是**当前打开**的会话时 | [128](128-idb-images-store.md) 要求实现者定这个语义。定了之后要真的踩一次 |
| 5 | 浏览器把 IndexedDB **驱逐**之后 | 119 §五-4 定的是「接受被驱逐，降级靠 `not_found`」。手动清 DevTools 里的存储，确认降级真的干净、模型真的自纠 |
| 6 | **刷新之后那段识图对话读起来是占位符** | [130](130-browser-vision-end-to-end.md) 真机验收发现。`transient_source_completion.rs:52` 的设计：消费过 one-shot 素材的那一轮，模型自己的完成文本也不进持久历史（`SAFE_CANDIDATE`），真文本经 `emit_terminal_candidate` 单独给宿主。**机制没错，页面缺一块**——直播时看得见，刷新后看不见。这条要判的是：浏览器形态该不该自己留一份带外副本，还是接受「识图对话不可重放」 |

## 验收

上面七步全过，**六个**交界处**每一条都有结论**（不是「看起来没问题」，
是「跑了，结果是 X」）。

**没过的要写清楚差在哪**——M13 那次「三家 provider 各一轮」只有 DeepSeek 的 key，
如实记成「过五条、没过一条、原因是缺 key 不是缺代码」，比含糊地报「全过」有用得多。

## 注意

- ⚠️ **provider key 不进仓库、不进 scratchpad 之外的任何地方。**
  M12 那次 dogfood 的做法可以照抄：配置副本放 scratchpad、用环境变量指过去、
  **跑完删掉副本**，绝不碰用户的 `providers.toml`。
  浏览器这边 key 是页面输入框里现填的，**验收记录里只许写 key 的长度**（111 契约第 4 条）。
- 识图写死 Kimi 3，所以**这条至少需要一把 Kimi key**，跟主对话用哪家无关。
  只有 DeepSeek key 的话，第 2、3、4 步跑不了——**提前确认，别跑到一半才发现**。
- 实测数字（token、耗时、缓存命中率）写进实做记录。M12 那次留下的
  「一次压缩 ≈120 轮」的修正就是这么来的——**没记下来的数字等于没测**。

## 实做记录：M14 真机 dogfood（主会话，2026-08-12，Chrome + 真 Kimi key）

一次连贯会话 `m14-dogfood`，provider = kimi/kimi-k3，工具表 6 条。
探针图 canvas 现画：绿字「橙色灯塔」+ 黑字「5178」，23939 字节，**内容提示词里没提过**。

### 七步

| 步 | 结果 |
|---|---|
| 1 通用回调 | ✅ 模型调页面声明的 `web:page/viewport`，答出真实 1200×817 |
| 2 图片 | ✅ 模型**自己决定**调 `web:source/vision`，答出图里两行内容 |
| 3 追问 | ✅ 同一链接再调一次，答出 `5178` |
| 4 刷新 | ✅ 已在 [130](130-browser-vision-end-to-end.md) 真机验收中单独跑过（重放、图还在、再识别成功） |
| 5 取消 | ✅ 已在 [123](123-host-tool-deadline.md) 真机验收中单独跑过（**4 毫秒**收尾，晚到结果不改状态） |
| 6 压缩 | ⛔ **本次不做**，见下 |
| 7 删除 | ✅ 见交界处 4 |

### 交界处逐条结论

**① 压缩 × transient-source —— 本次不验，范围外。**
追这条时先撞上一个真实缺口：**浏览器宿主的配置里没有 `context_window`**
（`config.rs` 只搬了 provider/base_url/model/api_key 四个字段），而 M12 的触发判据是
「上一轮实测 `prompt` / `context_window`」——窗口是 `None` 时整套五档分级**结构上
不可达**。这跟 M12 收尾时在 native 侧踩的是同一个坑（当时五个宿主全是 `None`，
见 [110](110-compaction-dogfood.md)），浏览器是第六个。

**已把这个字段接上**（`config.rs` 解析 + 页面一个输入框 + `provider_config()` 透传），
因为「宿主配置缺一个别的宿主都有的字段」是 wasm 宿主自己的完整性缺口。但
**压缩与 transient-source 的交互不是 wasm 的事**，本轮到此为止——用户明确划了范围。
留给真要动 M12 那条线的人：现在窗口能配了，填个小值几轮就能逼出来。

**② 一轮里两条宿主工具（一条内建 + 一条页面的）—— 通过。**

```
→ 调用宿主工具 web:page/title  input={}
→ 调用宿主工具 web:page/viewport  input={}
← web:page/title 返回 42 字节
← web:page/viewport 返回 51 字节
assistant  - 页面标题：agent-wasm 浏览器宿主（issue 114c）
           - 视口尺寸：1200 × 817 CSS 像素，devicePixelRatio = 1
```

**两条槽在同一轮里派发、都回传、模型合并作答。** 124 给 drain 循环加的按名字分流
（内建走 `resolve_remote_tool_async`、`web:source/` 走 `submit_remote_tool_result_async`）
在混合场景下没有互相踩到；121 的「内建优先、回调兜底」也同时生效——
`web:page/title` 由 Rust 执行，`web:page/viewport` 由页面回调执行，同一轮。

**③ 识图那一轮的前缀缓存 —— 有数，但比预想的好。**

识图轮 `prompt=1042 cached=768`（73.7%）。one-shot 安全重编码确实让那一轮不是
纯 `Reuse`，但**没有把缓存打到零**——原因跟 M12 那条反直觉实测同源：固定前缀
（system + 6 条工具表）占比高时，中段变化伤不到前面那一段。

⚠️ 这个数字**不能外推**：本次会话历史很短（几百到一千 token 量级），
历史一长比例会变。要引用就连同这句一起引用。

**④ `deleteSession` 删当前打开的会话 —— 通过。**
`sessionId()` 从 `m14-dogfood` 变 `null`、库消失、同 id 重开是**空历史**。
128 定的语义（允许删当前会话、代价是它当场被关掉）在真机上就是这个样子。

**⑤ IndexedDB 被驱逐之后 —— 降级干净。**
手动 `images.clear()` 模拟驱逐，再让模型看同一个链接：

```
← web:source/vision 返回 33 字节（错误）
assistant  识图服务返回错误：上传的图片不存在
           （[not_found] 上传的图片不存在：/uploads/up-5b6a…）。
           可能是该文件已被删除、链接已过期…你可以重新上传一次图片，我再来读取。
```

**模型看到的是带码的真错误**（`[not_found] …`，one-shot 覆盖进 prompt），
历史里留的是 `[transient_source_error_redacted]`。119 §五-4 那条「接受被驱逐，
降级靠 `not_found`」在真机上兑现，而且模型**自己纠正**、让用户重传，没有反复重试。

这一条同时验证了主会话补的错误码契约（`<码>：<细节>`，见 [130](130-browser-vision-end-to-end.md)
文末）——模型引用的那个 `[not_found]` 就是它。

**⑥ 刷新后识图对话读起来是占位符 —— 结论：机制没错，页面缺一块，本轮不改。**
详见 [130](130-browser-vision-end-to-end.md) 文末。`transient_source_completion.rs:52`
的设计：消费过 one-shot 素材的那一轮，模型自己的完成文本也不进持久历史。
浏览器页面今天只是照着重放结果画，所以直播时看得见、刷新后看不见。
**要判的是「页面该不该留一份带外副本」，那是页面的持久化策略，不是 M14 的接缝。**
登记进 M14 遗留。

### M14 遗留（都不阻塞）

1. **刷新后识图对话不可重放**（交界处 ⑥）。机制如设计，页面缺带外副本。
2. **压缩与 transient-source 的交互没验过**（交界处 ①）。`context_window` 现在能配了，
   工具已就位，缺的只是有人去跑。
3. **`web:host/callback-probe` 是验收脚手架**，留在页面声明里。真要发布给使用者，
   把它从 `PAGE_TOOL_DECLARATION` 删掉即可——它不在 Rust 侧，删了不影响任何实现。
