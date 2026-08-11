#!/usr/bin/env bash
# 跑 wasm32 目标的真实执行验证：起 server.mjs，等它监听好，跑
# `wasm-pack test --node`（真实 Node fetch/AbortController/ReadableStream，
# 不是 mock），最后无论成败都杀掉 server。
#
# 端口固定写死（不用 `PORT=0` 随机分配再回传）：wasm-bindgen-test 里的 Rust
# 测试代码在编译期就要知道连哪个端口，没有运行时从 shell 传参数进 wasm 测试
# 二进制的机制，两边只能靠一个提前约定好的常量对齐——server.mjs 与
# tests/wasm_smoke.rs 都硬编码 18391，改一处要跟着改另一处。
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

PORT=18391

node tests/wasm_node/server.mjs "$PORT" &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 50); do
  if curl -s -o /dev/null "http://127.0.0.1:$PORT/payment-required"; then
    break
  fi
  sleep 0.1
done

wasm-pack test --node
