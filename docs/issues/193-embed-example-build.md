# 193 嵌入样例：实现

**里程碑** L · **依赖** [192](192-embed-example-scope.md) · **模型** sonnet · **独测** ✅ · **估时** **60–90min，要拆成 193a/b/c**（[192](192-embed-example-scope.md) 已估） · **状态** 完成（2026-08-13，四条真机全过）

## 目标

按 [192](192-embed-example-scope.md) 定的场景做出来。

## 状态说明（2026-08-13 更新）

[192](192-embed-example-scope.md) 已拍板：**场景 = 同一个 agent、不同角色不同工具**，
**宿主形态 = 浏览器 wasm**，清单六条见那份 issue。工时诚实估计
**60–90 分钟**，按仓库规矩要拆成 193a（双角色声明跑通一轮）／193b（不可逆屏障 +
恢复后角色不变）／193c（文案与入口）。

~~**但现在刻意不拆、不做。**~~ **推迟被用户推翻了（2026-08-13）。**

我原本按「首发后的真实反馈会改变场景选择」把它压住了，理由是
[021](021-skeleton.md) 那条判据。用户说「全部执行完」——**那是他的决定，不是我的**，
而且他的判断站得住：[192](192-embed-example-scope.md) 的场景选择是靠判据推出来的
（「写死必须是结构上不可能的」），不是靠对反馈的猜测，所以它不依赖首发结果。

也没有拆成 a/b/c：真做起来一次做完更连贯（三段之间共用同一份页面骨架，
拆开等于把同一个文件改三次）。**估的 60–90 分钟仍然是对的**，只是它是一整块。

## 已知的约束（不管选哪个场景都成立）

- 例子要**能跑**，不是伪代码。跑不起来的例子比没有例子更伤
- 例子里的能力声明要**过一遍校验路径**（排序、journal、恢复），
  展示的正是这套机制的价值
- 别在例子里塞太多功能。它要证明一件事，不是展示全部特性
- 例子的 README 要能独立读懂，不要求读者先读完主仓文档

## 验收

待 [192](192-embed-example-scope.md) 填。

---

## 实做记录（2026-08-13）

`crates/agent-wasm/www/roles.html`（242 行）+ `roles-tools.js`（102 行）。
按 [192](192-embed-example-scope.md) 的六条清单做，全部落地。

### 一份 wasm、两份声明，靠角色选

```
viewer     web:orders/search                    1 条，只读
operator   web:orders/search + web:orders/refund  2 条，refund 声明 irreversible
```

两份都是**模块级常量字符串**，不是按角色现拼的——红线 11：现拼的话同一个角色两次
刷新只要拼接顺序有一点不同，前缀缓存就断，而且一声不吭。共用的那条 `search` 是同一个
常量拼进去的，避免改一处忘另一处。

### 四条真机验收（DeepSeek，浏览器直连）

**① viewer 看不到 refund** —— 声明里 1 条工具、无 `refund` 字样。让它退款：

> I can't do that — **I only have a read-only order search tool (no refund capability).**
> Order A-1001 (¥1,280.00, 已付款) would need to be refunded through the actual order
> management system.

**模型自己说出了那句话**，比页面上写「viewer 没有退款权限」有力得多——
这是整个例子存在的意义。

**② operator 能退** —— 同一份 wasm、同一个模型，声明换成 2 条之后：
先 `search` 确认订单存在，再 `refund`，回答「已为订单 A-1001 完成退款。该操作不可撤销」。

**③ undo 撞屏障**：

```json
{"kind":"Blocked","entries":1,"barrierSeq":5,
 "barrier":{"label":"tool_result","tool":"web:orders/refund","callId":"call_00_LDARq…"}}
```

拦住了，并**点名是 `web:orders/refund`**。页面弹确认框时那句文案是有意写的：
*The refund already happened — undoing the conversation does not undo the payment.*

**④ 刷新后 viewer 仍是 viewer** —— 重新加载页面、重开 `roles-viewer`，
声明仍是 1 条，模型对第二次退款请求照样拒绝。

### 「钱撤不回来」是这个例子最好的部分，而且是免费的

已退款的订单号存在**页面内存的一个 `Set` 里**，不在 atom 里。这不是偷懒——
它是红线 3 那条线的自然结果：**状态进 atom，执行现场留在宿主**。

于是这个例子里出现了一个真实的不对称：**账本能撤掉「模型说它退了款」这件事，
撤不掉钱**。可逆性屏障存在的全部理由就是这个不对称，而这里它是可触摸的，
不是文档里的一句话。

### 挂了三处入口

主 demo 页首屏、两份 README 的「宿主可以动态扩展 Agent」一节。README 里那句是：

> One deployment, one agent, capability surface per caller — **there is no fixed tool list
> in a Rust core that expresses that.**

### 验收（[192](192-embed-example-scope.md) 定的那条）

> **读者能说出「把工具写死在 Rust 里，这个例子做不出来」的理由。**

满足：两个角色跑在**同一份 wasm 二进制**上，工具表随会话创建者变。
写死一份就没有第二份——这句话在页面上、README 上、以及模型自己的回答里各出现一次。

### 门禁

`build-wasm.sh` / `check-invariants.sh --all` / `cargo test --workspace` 全绿。
新增两个文件都在红线 9 的 300 行以内（242 / 102）。

### 留的尾巴

- [x] ~~两个页面的语言策略应该统一~~ —— **2026-08-13 统一了，主 demo 向 `roles.html` 看齐。**

      「牵扯 171 那条」的前提**验过之后是错的**：`grep` 全仓，没有任何自动断言
      绑在页面文案上，所谓验收夹具是人手动驱动的页面。于是不必拆页——
      主 demo 的验收控件收进一个 `<details>`（中文与 issue 号逐字保留），
      其余英文化，`lang` 改 `en`，与 `roles.html` 同构。

      决定、做法、真机复验全记在 [171](171-demo-first-screen.md) 末尾，这里不重复。
      **这条当初标的是「别顺手做」，最后也确实没顺手做**——是单独做的，
      带完整的真机复验（口令实验四步 + 控制台零 error + 三门禁），
      而且过程中真的抓到了两个东西（一个我改出来的半截字符串，
      一个 171 当初漏验的手机端首屏问题）。
- [ ] 这个页面没有 GIF。[172](172-demo-gif.md) 那套手法可复用，但首发只需要一张图，
      多了反而分散。等首发之后看要不要补。
