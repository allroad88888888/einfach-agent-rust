#!/usr/bin/env bash
# 构建浏览器宿主（issue 114c）：agent-wasm → wasm32-unknown-unknown → wasm-bindgen 胶水。
#
# 产物落在 crates/agent-wasm/www/pkg/，跟同目录的 index.html 一起就是一个能直接打开
# 的页面。**必须用 http:// 打开，不能 file://**——ES module 与 wasm 的 MIME 都过不了
# file 协议，随便一个静态服务器即可（脚本最后一行给了一句现成的）。那个服务器只发
# 三种字节，不参与任何一次模型请求：114 验收第一条「没有任何服务端进程」说的是没有
# agent 服务端，静态托管不在此列。
#
# 用法：
#   scripts/build-wasm.sh          # release（体积小，浏览器加载快）
#   scripts/build-wasm.sh --dev    # dev（编译快，产物大，调试用）
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
crate="$root/crates/agent-wasm"

profile="--release"
if [[ "${1:-}" == "--dev" ]]; then
  profile="--dev"
fi

if ! command -v wasm-pack >/dev/null 2>&1; then
  echo "缺 wasm-pack：cargo install wasm-pack（或 https://drager.github.io/wasm-pack/installer/）" >&2
  exit 1
fi

# `--target web`：产出原生 ES module，页面直接 `import init from './pkg/agent_wasm.js'`，
# 不需要 bundler。`--no-typescript` 不加——生成的 .d.ts 顺带就是一份接口文档。
wasm-pack build "$crate" $profile --target web --out-dir www/pkg

echo
echo "产物：$crate/www/pkg"
echo "打开：cd $crate/www && python3 -m http.server 8787   然后访问 http://127.0.0.1:8787/"
