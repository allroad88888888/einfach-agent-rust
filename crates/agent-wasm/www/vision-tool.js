// vision-tool.js —— `web:source/vision` 这一条工具的执行体。
//
// issue 130。三步接线，三个零件都是别人做完的：
//
//   input.image → resolveImage(link)            （129，image-store.js）
//                 → host.inspectImage(bytes, mime, question)   （127，Rust）
//                 → 识别结果文本
//
// 本文件不碰 DOM、不碰 IndexedDB、不认识 AgentHost——两个能力都是构造参数传进来
// 的函数。它真正独有的那一件事是**失败怎么说话**：把浏览器这条路上三个不同来源
// 的失败，翻成 `agent-tools/src/vision_inspect.rs` 已经在用的那套错误码。
//
// # 为什么错误码必须跟 native 那条对齐
//
// `srv:vision/inspect`（native）与 `web:source/vision`（浏览器）是同一件事的两种
// 形态。两边错误码不一致的话，同一个故障在 CLI 和浏览器里长得不一样，排查时会
// 以为是两个 bug。所以这里**不新造错误码**，只用 vision_inspect.rs 已有的四个：
//
// | 码 | 什么时候 | native 的同款位置 |
// |---|---|---|
// | `bad_input` | image 缺失/为空/不是 `/uploads/<id>` 形状/id 非法 | `vision_inspect.rs::parse_input`、`vision_source.rs::resolve_bytes` |
// | `not_found` | 链接形状对，但这张图在会话里没有了 | `vision_source.rs::read_uploaded` |
// | `upload_failed` | Kimi files 上传那一跳失败 | `vision_source.rs::upload` |
// | `provider_error` | Kimi chat completions 那一跳失败 | `vision_source.rs::chat_completion` |
//
// # 模型看到的文本形状也对齐：`[code] message`
//
// native 侧工具失败经 `agent-runtime/src/tool_exec.rs:50` 拼成 `[{code}] {message}`
// 才进模型的 tool_result。浏览器这条路的失败是**工具回调 throw 出去**（121：
// reject → `RemoteToolOutput::Failure`，正文取异常的 message），中间没有那一层
// 拼装，所以这里手工拼同一个形状。模型因此在两种形态下看到逐字同构的失败文本。
//
// # 一处**故意的**措辞差异
//
// native 的 `not_found` 正文用的是裸 id（`上传的图片不存在：{id}`），这里用完整
// 链接。理由：模型给出去的是链接，回它链接它才知道是哪一次调用错了；而复述裸 id
// 需要在本文件再抄一份 `/uploads/` 前缀常量——重复一个**形状常量**比重复一句话
// 危险得多（哪天两边被改歪一个，症状是链接解析静默失配）。前缀只有
// image-store.js 一处知道。

/**
 * 一次识图工具失败。`message` 就是模型会看到的那一行，形如
 * `[not_found] 上传的图片不存在：/uploads/up-…`。
 */
export class VisionToolError extends Error {
  constructor(code, detail) {
    super(`[${code}] ${detail}`);
    this.name = "VisionToolError";
    this.code = code;
    this.detail = detail;
  }
}

/** 缺省问题，跟 `vision_inspect.rs::parse_input` 逐字一样。 */
const DEFAULT_QUESTION = "这张图片里有什么？";

/** 129 的 `ImageLinkError.code` → vision 这条工具的错误码 + 正文。 */
const LINK_FAILURES = {
  bad_format: (link) => [
    "bad_input",
    `image 必须是本地上传链接（/uploads/<id>），收到：${link}`,
  ],
  bad_id: (link) => ["bad_input", `上传链接 id 非法：${link}`],
  not_found: (link) => ["not_found", `上传的图片不存在：${link}`],
};

/**
 * `inspectImage` reject 出来的 message → 错误码。
 *
 * 契约是 **`<码>：<细节>`**，由 `agent-wasm/src/vision.rs` 的 `inspect` 文档注释
 * 定死：每一条失败路径都以码开头。所以这里只需要一条正则，不需要认任何中文散文。
 *
 * ⚠️ **130 落地时这里曾经是一张中文前缀表**（「Kimi 图片上传失败：」→
 * `upload_failed` 之类），因为当时 `vision.rs` 只有一半路径带码。那种耦合的症状
 * 极其隐蔽：改一句 Rust 侧的提示语，这边就把 `upload_failed` 静默重分类成兜底的
 * `provider_error`——不报错、不断编译、没有测试盯着。主会话复核时把缺的三个码补进
 * `vision.rs`，这张表随之删除。**不要因为「多认几种写法更稳妥」把它加回来**，
 * 那等于把契约换成猜测。
 */
const ALREADY_CODED = /^([a-z_]+)：([\s\S]*)$/;

/**
 * 执行一次 `web:source/vision`。
 *
 * @param {object} deps
 * @param {string} deps.inputJson 模型给的入参 JSON（transient-source 路径下这已经是
 *   认领之后的**真值**，不是历史里那个 `{"transient_source":"redacted"}` 占位符）
 * @param {(link: string) => Promise<{bytes: Uint8Array, mime: string}>} deps.resolveImage
 * @param {(bytes: Uint8Array, mime: string, question: string) => Promise<string>} deps.inspectImage
 * @returns {Promise<string>} 识别结果文本
 * @throws {VisionToolError} 失败一律是它，`message` 已经是 `[code] …` 形状
 */
export async function runVisionTool({ inputJson, resolveImage, inspectImage }) {
  const { image, question } = parseInput(inputJson);
  const { bytes, mime } = await resolveBytes(resolveImage, image);
  return await inspect(inspectImage, bytes, mime, question);
}

/** 入参校验，逐条对着 `vision_inspect.rs::parse_input`。 */
function parseInput(inputJson) {
  let input;
  try {
    input = JSON.parse(inputJson);
  } catch (err) {
    throw new VisionToolError(
      "bad_input",
      `web:source/vision 的入参不是合法 JSON：${err.message ?? err}`,
    );
  }
  const image = input?.image;
  if (typeof image !== "string") {
    throw new VisionToolError(
      "bad_input",
      "web:source/vision 缺少必填参数 image（本地图片链接）",
    );
  }
  if (image === "") {
    throw new VisionToolError("bad_input", "image 不能为空");
  }
  const question =
    typeof input.question === "string" && input.question !== ""
      ? input.question
      : DEFAULT_QUESTION;
  return { image, question };
}

/** 129 的 `resolveImage`：三种坏链接分开翻码，别的异常（库打不开之类）落
 * `not_found`——对模型来说「这张图现在拿不到」是同一件可自纠的事。 */
async function resolveBytes(resolveImage, image) {
  try {
    return await resolveImage(image);
  } catch (err) {
    const failure = LINK_FAILURES[err?.code];
    if (failure) {
      throw new VisionToolError(...failure(image));
    }
    throw new VisionToolError(
      "not_found",
      `取不到这张图片的字节：${image}（${err?.message ?? err}）`,
    );
  }
}

/** 127 的 `inspectImage`：两跳网络，失败按上面那张前缀表分类。 */
async function inspect(inspectImage, bytes, mime, question) {
  try {
    return await inspectImage(bytes, mime, question);
  } catch (err) {
    throw classifyInspectFailure(err?.message ?? String(err));
  }
}

function classifyInspectFailure(message) {
  const coded = ALREADY_CODED.exec(message);
  if (coded) {
    return new VisionToolError(coded[1], coded[2]);
  }
  // 认不出来的失败宁可报 provider_error 也不静默吞掉：它是「识图那一步没成」的
  // 兜底类别，跟 native 把一切 chat 阶段失败归到 provider_error 是同一个粒度。
  return new VisionToolError("provider_error", `识图失败：${message}`);
}
