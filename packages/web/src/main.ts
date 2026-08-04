// 唯一职责：启动装配——建会话、开 SSE、把输入框/按钮接到 api.ts 的请求上。
// 不含渲染逻辑（render/）、不含协议解析（connection.ts）、不含工具执行
// （tool-exec.ts）,纯接线,跟 `crates/agent-cli/src/main.rs` 的自我定位
// （「纯接线」）是同一个判据。
import { cancelBtn, composerEl, inputEl, redoBtn, statusEl, undoBtn } from "./dom";
import { createSession, fetchAgentTree, sendCancel, sendInput, sendRedo, sendUndo } from "./api";
import { webCapabilities } from "./capabilities";
import { connect, type ConnectionState } from "./connection";
import { createRenderer } from "./render/dispatch";
import { createToolExecutor } from "./tool-exec";
import { renderAgentTree } from "./render/agent_tree";
import { connectMcpServers, describeStatus, registerMcpTools } from "./mcp";
import { parseMcpServers } from "./mcp-config";

async function main(): Promise<void> {
  // 067 + 068：**先连 MCP、再建会话**。这个顺序不是风格问题——注入只有建会话
  // 那一次机会（接缝 §三：不做运行时增删），晚一行这些工具就赶不上这个会话了。
  // 配置从地址栏来（`?mcp=<id>=<url>`，见 `./mcp-config`）；不带参数就一条都不连，
  // `webCapabilities()` 与 067 之前逐字节相同。
  const mcpConfigs = parseMcpServers(window.location.search);
  if (mcpConfigs.length > 0) {
    statusEl.textContent = `连接 ${mcpConfigs.length} 个 MCP server…`;
    const mcp = await connectMcpServers(mcpConfigs);
    // 连不上的不致命：`connectMcpServers` 逐个报状态、失败的那个不贡献工具，
    // 会话照常建（跟 044 的「失败隔离」同精神）。状态打进控制台，人看得见。
    for (const server of mcp.servers) console.info(`[mcp] ${describeStatus(server)}`);
    registerMcpTools(mcp);
  }

  statusEl.textContent = "创建会话…";
  // 065：建会话这一次把本前端的能力声明一起发出去（`./capabilities`）——只有
  // 这一次机会（接缝 §三：不做运行时增删）。067 接 MCP 后要在这一行**之前**
  // 把翻译好的工具 `registerWebTool` 进去,晚了就赶不上这个会话。
  const sessionId = await createSession(webCapabilities());

  // 同一条 SSE 上并排挂两个消费者：**渲染**（画出来）和**执行**（066：模型
  // 点名的 `web:` 工具在这个浏览器里跑掉、结果 `POST /tool_result` 回去）。
  // 两件事分开而不是把执行塞进 `render/dispatch.ts` 的 `switch`——那个文件的
  // 唯一职责是「一帧 → 该调渲染层哪个函数」,执行工具不是渲染。
  // 顺序：先渲染再执行,让卡片在工具真跑起来之前就出现在时间线上。
  const render = createRenderer(sessionId);
  const executeTools = createToolExecutor(sessionId);
  connect(sessionId, (frame) => {
    render(frame);
    executeTools(frame);
  }, (state) => {
    updateStatus(sessionId, state);
    // 049：`"open"` 既覆盖首次连接也覆盖 `EventSource` 每次自动重连（两者在
    // 这个回调里长得一样,`connection.ts` 不区分）——每次都补一次 GET 重新
    // 做种,是 issue 范围条款「断开重连 → 树面板恢复成当前正确的树（GET 做种
    // + Last-Event-ID 补帧）」要的双保险：`agent_tree` 帧本身也走 hub 的
    // 环形缓冲、靠 Last-Event-ID 补发,但补发跟不上（`gap`）时这次 GET 兜底
    // 拉到当下真实的树。`renderAgentTree` 整棵重画,多调一次没有副作用。
    if (state === "open") {
      void fetchAgentTree(sessionId).then(renderAgentTree).catch(reportError);
      // 072：同一个钩子再挂一件事——拉一次待办投影，把还欠着的 `web:` 工具补
      // 执行掉。跟上面那次 GET 是同一个道理（**推和拉两条路给出同一份事实**），
      // 但这一条是**正确性**不是双保险：帧可能根本到不了（ring 被挤爆的 `gap`、
      // 断线期间派下来的活），而「活还欠着」只有服务端知道。
      void executeTools.sweep().catch(reportError);
    }
  });

  composerEl.addEventListener("submit", (event) => {
    event.preventDefault();
    const text = inputEl.value.trim();
    if (!text) return;
    inputEl.value = "";
    void sendInput(sessionId, text).catch(reportError);
  });

  // Undo/Redo 按钮只发默认档（turn 粒度、非 force）——`undo_blocked` 的确认
  // 弹层由收到的 `SessionEvent::Undo` 帧触发（`render/undo.ts`），不是按钮
  // 点击本身：031 的四个命令端点都是 fire-and-forget（202,不等结果),按钮
  // 点击这一刻还不知道会不会撞屏障。
  undoBtn.addEventListener("click", () => void sendUndo(sessionId, "turn", false).catch(reportError));
  redoBtn.addEventListener("click", () => void sendRedo(sessionId).catch(reportError));
  cancelBtn.addEventListener("click", () => void sendCancel(sessionId).catch(reportError));
}

function updateStatus(sessionId: string, state: ConnectionState): void {
  const label = state === "open" ? "已连接" : state === "error" ? "断线，自动重连中…" : "连接中…";
  statusEl.textContent = `session ${sessionId} · ${label}`;
}

function reportError(e: unknown): void {
  statusEl.textContent = `出错：${e instanceof Error ? e.message : String(e)}`;
}

void main();
