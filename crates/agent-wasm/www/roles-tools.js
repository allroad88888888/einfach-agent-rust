// roles-tools.js —— 193 的工具面：**同一个部署、同一个 agent，工具集随调用者变**。
//
// 这个例子存在的唯一理由，是让人一眼看到「宿主声明工具」比「把工具写死在 Rust 里」
// 强在哪。判据是 192 定的：**那个场景里写死必须是结构上不可能的**，否则读者会想
// 「那我直接写死不就行了」。
//
// 这里是：两个角色，同一份 wasm，工具集不同。写死一份就没有第二份。
//
// ⚠️ 红线 11 照旧：工具表进 prompt 最前面，逐字节漂一次前缀缓存就全断。所以两份
// 声明都是**模块级常量字符串**，不是按角色现拼的——现拼的话，同一个角色两次刷新
// 只要拼接顺序有一点不同，缓存就没了，而且一声不吭（DeepSeek 上 120 倍差价）。
//
// 假数据，几十行，不连任何真实系统：这个例子要证明的是**声明机制**，
// 不是订单系统怎么写。

/** 两个角色都有的只读工具。**两份声明里逐字一样**——不是复制粘贴出来的两份，
 * 是同一个常量拼进去的，避免哪天改一处忘另一处。 */
const SEARCH_TOOL = `{"name":"web:orders/search",
   "description":"按订单号或客户名查订单，返回订单号、客户、金额、状态。只读。",
   "schema":{"type":"object","properties":{"query":{"type":"string","description":"订单号或客户名"}},"required":["query"],"additionalProperties":false},
   "reversibility":"pure"}`;

/** 只有 operator 有。**`irreversible` 是这条例子的第二个看点**：退款是花钱的动作，
 * undo 撞上它必须停下来问，而不是悄悄回滚一个「已经打出去的钱」。
 * 判据同 `vision_inspect.rs:66-68`（调第三方 API 计费，undo 不该重放）。 */
const REFUND_TOOL = `{"name":"web:orders/refund",
   "description":"给一笔订单退款。这个操作会真的打钱，不可撤销。order：订单号，必填。",
   "schema":{"type":"object","properties":{"order":{"type":"string","description":"订单号"}},"required":["order"],"additionalProperties":false},
   "reversibility":"irreversible"}`;

/** viewer：只读。 */
export const VIEWER_TOOLS = `{"tools":[
  ${SEARCH_TOOL}
]}`;

/** operator：只读 + 退款。 */
export const OPERATOR_TOOLS = `{"tools":[
  ${SEARCH_TOOL},
  ${REFUND_TOOL}
]}`;

export function toolsForRole(role) {
  return role === "operator" ? OPERATOR_TOOLS : VIEWER_TOOLS;
}

/** 假订单库。故意小且固定——例子要可复现，不要随机数据。 */
const ORDERS = [
  { id: "A-1001", customer: "Acme Corp", amount: "¥ 1,280.00", status: "已付款" },
  { id: "A-1002", customer: "Acme Corp", amount: "¥ 340.00", status: "已发货" },
  { id: "B-2071", customer: "Wombat Ltd", amount: "¥ 9,900.00", status: "已付款" },
];

/** 已退款的订单号。**只活在页面内存里**——它是「执行现场」，不是 agent 状态。
 * 这正是红线 3 说的那条线：状态进 atom，执行现场留在宿主。
 * 于是 undo 能撤掉「模型说它退了款」这件事，但撤不掉钱——所以才需要屏障。 */
const refunded = new Set();

/**
 * 工具执行回调。签名同 121 的 `onToolCall(handler)`。
 *
 * ⚠️ 回调里只能干「不经过这个 AgentHost 的活」（见 `onToolCall` 的文档注释）。
 * 这里只读常量、写一个页面级 Set、打日志，全部合规。
 */
export function createRolesToolCallback({ log, onRefund }) {
  return async function onToolCall(name, inputJson) {
    const input = JSON.parse(inputJson || "{}");
    log(`→ ${name} ${inputJson}`);

    if (name === "web:orders/search") {
      const q = String(input.query ?? "").toLowerCase();
      const hits = ORDERS.filter(
        (o) => o.id.toLowerCase().includes(q) || o.customer.toLowerCase().includes(q),
      );
      const body = hits.length
        ? hits
            .map(
              (o) =>
                `${o.id} | ${o.customer} | ${o.amount} | ${refunded.has(o.id) ? "已退款" : o.status}`,
            )
            .join("\n")
        : "没有匹配的订单。";
      log(`← ${name} 命中 ${hits.length} 条`);
      return body;
    }

    if (name === "web:orders/refund") {
      const id = String(input.order ?? "");
      const known = ORDERS.some((o) => o.id === id);
      if (!known) {
        // 抛出 = 模型收到 is_error，自己纠正。不崩页面（121 的反向锁同款语义）。
        throw new Error(`没有这笔订单：${id}`);
      }
      refunded.add(id);
      onRefund?.(id);
      log(`← ${name} 已退款 ${id}（页面内存，undo 撤不回来——这正是屏障存在的理由）`);
      return `订单 ${id} 已退款。`;
    }

    // viewer 角色下模型根本看不到 refund，走不到这里；真走到了说明工具表漏了。
    throw new Error(`这个角色没有这条工具：${name}`);
  };
}
