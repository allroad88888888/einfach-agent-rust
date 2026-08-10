// 唯一职责：s5/s6 的 HTTP 请求形状验收——图片字节不进 agent 协议。对真的
// `sendInput` 截获 fetch，断言：无图时请求体逐字节仍是旧的 `{"text":"..."}`；
// 有图时先 POST /uploads 拿链接，链接作为纯文本拼进 text 再发 `/input`，body
// 里没有 images 字段、没有图片字节。
//
//   pnpm --filter web verify:images
//
// 浏览器入口、缩略图与 object URL 生命周期是 087 真浏览器 dogfood 的验收面；此
// 文件只钉住不依赖 DOM 的 API 契约，避免为这几个断言引入一套 DOM 测试框架。
import { sendInput } from "../src/api";

let passed = 0;
const failures: string[] = [];

function equal(label: string, actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) === JSON.stringify(expected)) {
    passed += 1;
    console.log(`  ✓ ${label}`);
    return;
  }
  failures.push(label);
  console.log(`  ✗ ${label} —— 实际 ${JSON.stringify(actual)}，期望 ${JSON.stringify(expected)}`);
}

async function main(): Promise<void> {
  const requests: Array<{ path: string; body: string | undefined }> = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
    requests.push({ path: String(input), body: typeof init?.body === "string" ? init.body : undefined });
    if (String(input) === "/uploads") {
      return new Response(JSON.stringify({ url: "/uploads/up-1" }), { status: 200 });
    }
    return new Response(null, { status: 202 });
  }) as typeof fetch;

  try {
    await sendInput("plain", "北京天气");
    equal("无图 body 逐字节仍是旧形状", requests[0], {
      path: "/sessions/plain/input",
      body: '{"text":"北京天气"}',
    });

    const image = new File([new Uint8Array([137, 80, 78, 71])], "nonce.png", { type: "image/png" });
    await sendInput("image", "读数字", [image]);
    equal("有图时先打上传端点拿链接", requests[1], {
      path: "/uploads",
      body: undefined,
    });
    equal("链接拼进 text 再发 /input，body 无 images 字段", requests[2], {
      path: "/sessions/image/input",
      body: '{"text":"读数字\\n\\n[图片：/uploads/up-1]"}',
    });
    equal("协议 body 不含图片字节（PNG 魔数 137,80,78,71）", !JSON.stringify(requests[2]).includes("137"), true);
  } finally {
    globalThis.fetch = realFetch;
  }

  console.log(`\n=== ${passed} 条通过，${failures.length} 条失败 ===`);
  if (failures.length > 0) process.exitCode = 1;
}

void main();
