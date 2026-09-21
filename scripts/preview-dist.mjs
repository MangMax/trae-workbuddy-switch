// ---------------------------------------------------------------------------
// dist 预览服务器（仅供 UI 核对 / 受控演示断言）
//
// 为什么不直接用 `npm run preview`（vite preview）：
//   本仓库同时存在多个产物目录（dist / dist-demo / dist-ui-keys / dist-ui-region …），
//   vite preview 只服务 vite.config 里那一个 outDir，换目录要改配置。
//   这里接受任意目录参数，并**带 SPA 回退** —— 浏览器直开深链（如 /trae/accounts）
//   也能命中 index.html，这样 `_serve` + Playwright 就能直接断言路由页。
//
// usage:
//   node scripts/preview-dist.mjs <dist目录> [端口]      # 端口缺省 4174
//   node scripts/preview-dist.mjs dist-ui-keys 4188
//
// 只监听 127.0.0.1；不是产品运行时组件，不要用于对外服务。
// ---------------------------------------------------------------------------
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";

const root = process.argv[2];
const port = Number(process.argv[3] ?? 4174);

if (!root) {
  console.error("usage: node scripts/preview-dist.mjs <dist目录> [端口]");
  process.exit(2);
}

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webp": "image/webp",
  ".woff2": "font/woff2",
  ".ico": "image/x-icon",
};

createServer(async (req, res) => {
  const url = new URL(req.url, "http://127.0.0.1");
  let rel = decodeURIComponent(url.pathname);
  if (rel.endsWith("/")) rel += "index.html";
  const file = join(root, normalize(rel).replace(/^(\.\.[/\\])+/, ""));
  try {
    const body = await readFile(file);
    res.writeHead(200, { "content-type": MIME[extname(file)] ?? "application/octet-stream" });
    res.end(body);
  } catch {
    // SPA 回退：未命中的路径一律交给 index.html，由前端路由接管。
    const body = await readFile(join(root, "index.html"));
    res.writeHead(200, { "content-type": MIME[".html"] });
    res.end(body);
  }
}).listen(port, "127.0.0.1", () => {
  console.log(`serving ${root} on http://127.0.0.1:${port}`);
});
