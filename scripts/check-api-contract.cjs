#!/usr/bin/env node
"use strict";

// API 契约 / 演示数据一致性护栏（零依赖，仅用 Node 标准库）。
//
// 本项目的双通道适配层有若干处「纯字符串」必须逐字对齐，且全部没有编译期保护：
//   ① src/lib/api.ts                       —— call("<cmd>") 的 cmd 字符串
//   ② src/lib/api.ts                       —— ROUTES 表的键名与 { method, path }
//   ③ crates/buddy-switch-server/src/api.rs   —— 路由表的 path + method（跨语言，编译器无法互校）
//   ④ src-tauri/src/lib.rs                 —— invoke_handler 登记的命令名
//   ⑤ src/lib/screenshot-demo.ts           —— screenshotDemoResponse 的 switch case 分支
//                                             （须覆盖 DEMO_READ_COMMANDS 的每个只读命令）
//
// 任一处漂移，症状都是「构建全绿，但某端静默失效」（桌面端 command not found /
// webui 抛「暂不支持」/ method 写反打到错的 handler / 演示模式运行时报缺只读数据）。
// 本脚本在构建前做这些一致性校验，发现差异立即以非零退出码失败，并打印具体差异。
//
// 有意不做的一项：webui POST body 形状（`{ config: … }` vs 扁平）。后端对二者都接受
// （`body.get("config").unwrap_or(&body)`），且 body 形状在源码里没有机器可读的声明，
// 无法用稳健的静态规则校验——强行用脆弱正则只会降低整条护栏的可信度，故不做。
//
// 用法：node scripts/check-api-contract.cjs   （npm run check:api）

const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "..");
const API_TS = path.join(ROOT, "src", "lib", "api.ts");
const SCREENSHOT_DEMO_TS = path.join(ROOT, "src", "lib", "screenshot-demo.ts");
const SERVER_RS = path.join(ROOT, "crates", "buddy-switch-server", "src", "api.rs");
const TAURI_LIB_RS = path.join(ROOT, "src-tauri", "src", "lib.rs");

// webui 未提供、仅桌面端可用的命令。它们「有意」没有 ROUTES 路由条目——豁免的依据是
// 「无 ROUTES 路由」，而不是「没守卫」：这些命令的 wrapper **内部**都自带
// `demoModeEnabled` / `isWebui()` 早退守卫（见 api.ts 的 openPermissionSettings /
// checkAuthPermission / revealAppInFinder / relaunchApp / get|setLaunchAtLoginEnabled），
// 因此「webui 不可达」由函数自己保证，不依赖调用方自律。
// 除此之外的任何 call("<cmd>") 都必须有 ROUTES 条目、且必须已在桌面端登记。
const ROUTE_EXEMPT_COMMANDS = new Set([
  "open_permission_settings",
  "check_auth_permission",
  "reveal_app_in_finder",
  "relaunch_app",
  "get_launch_at_login_enabled",
  "set_launch_at_login_enabled",
]);

/** 读文件并去掉可能存在的 UTF-8 BOM。 */
function read(file) {
  return fs.readFileSync(file, "utf8").replace(/^\uFEFF/, "");
}

/**
 * 去掉行注释与块注释。
 *
 * 必要性（真实踩过）：注释里写 `call("<cmd>")` 这类示例会把扫描器骗过去，
 * 报出「未登记的命令 <cmd>」，而真正的调用点全都没问题——护栏于是从
 * 「保护构建」变成「制造噪声」，最后被开发者绕过。
 * 用 `(^|[^:])` 排除 `://`，避免把 URL 里的 `//` 当成行注释起点。
 */
function stripComments(text) {
  return text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/[^\n]*/g, "$1");
}

/**
 * 从 openIndex（指向 "{"/"("/"["）开始，返回与之配对的括号**内部**文本。
 *
 * 只对同一种括号计数，足以覆盖本项目里的对象字面量与函数实参（不含嵌套异种括号）。
 */
function sliceBalanced(text, openIndex) {
  const open = text[openIndex];
  const close = open === "{" ? "}" : open === "(" ? ")" : open === "[" ? "]" : null;
  if (!close) throw new Error(`sliceBalanced: 起始字符不是括号 (${JSON.stringify(open)})`);
  let depth = 0;
  for (let i = openIndex; i < text.length; i += 1) {
    const ch = text[i];
    if (ch === open) {
      depth += 1;
    } else if (ch === close) {
      depth -= 1;
      if (depth === 0) return text.slice(openIndex + 1, i);
    }
  }
  throw new Error("sliceBalanced: 括号未闭合");
}

/** 从 api.ts 的 ROUTES 表提取 { cmd, method, path } 列表。 */
function extractRoutes(apiTs) {
  const anchor = apiTs.indexOf("const ROUTES");
  if (anchor < 0) throw new Error("api.ts 中找不到 `const ROUTES`");
  const openIndex = apiTs.indexOf("{", anchor);
  if (openIndex < 0) throw new Error("api.ts 中找不到 ROUTES 对象字面量");
  const block = sliceBalanced(apiTs, openIndex);
  const re =
    /([A-Za-z0-9_]+)\s*:\s*\{\s*method\s*:\s*"([A-Za-z]+)"\s*,\s*path\s*:\s*"([^"]+)"\s*\}/g;
  const routes = [];
  let m;
  while ((m = re.exec(block)) !== null) {
    routes.push({ cmd: m[1], method: m[2].toUpperCase(), path: m[3] });
  }
  if (routes.length === 0) throw new Error("未能从 ROUTES 解析出任何路由条目");
  return routes;
}

/** 提取 api.ts 中所有裸 call("<cmd>") 的 cmd（排除 httpCall 等与函数定义）。 */
function extractCallCommands(apiTs) {
  // 先剥注释：注释中的示例调用不构成契约（见 stripComments 的说明）。
  const source = stripComments(apiTs);
  const re = /(?<![\w.$])call\s*\(\s*"([^"]+)"/g;
  const cmds = new Set();
  let m;
  while ((m = re.exec(source)) !== null) cmds.add(m[1]);
  if (cmds.size === 0) throw new Error("未能从 api.ts 解析出任何 call(\"<cmd>\")");
  return cmds;
}

/** 提取 api.ts 中 DEMO_READ_COMMANDS 集合包含的只读命令名。 */
function extractDemoReadCommands(apiTs) {
  const anchor = apiTs.indexOf("DEMO_READ_COMMANDS");
  if (anchor < 0) throw new Error("api.ts 中找不到 DEMO_READ_COMMANDS");
  const openIndex = apiTs.indexOf("[", anchor);
  if (openIndex < 0) throw new Error("DEMO_READ_COMMANDS 不是数组字面量");
  const block = sliceBalanced(apiTs, openIndex);
  const cmds = new Set();
  const re = /"([^"]+)"/g;
  let m;
  while ((m = re.exec(block)) !== null) cmds.add(m[1]);
  if (cmds.size === 0) throw new Error("未能从 DEMO_READ_COMMANDS 解析出任何命令");
  return cmds;
}

/** 提取 screenshot-demo.ts 中 screenshotDemoResponse 的 switch case 分支命令名。 */
function extractDemoCaseCommands(screenshotTs) {
  const anchor = screenshotTs.indexOf("export function screenshotDemoResponse");
  if (anchor < 0) throw new Error("screenshot-demo.ts 中找不到 screenshotDemoResponse");
  const openIndex = screenshotTs.indexOf("{", anchor);
  if (openIndex < 0) throw new Error("screenshotDemoResponse 函数体未找到");
  const body = sliceBalanced(screenshotTs, openIndex);
  const cmds = new Set();
  const re = /case\s+"([^"]+)"/g;
  let m;
  while ((m = re.exec(body)) !== null) cmds.add(m[1]);
  if (cmds.size === 0) throw new Error("screenshotDemoResponse 中解析不到任何 case 分支");
  return cmds;
}

/** 从 server api.rs 提取 path → 支持的 method 集合。 */
function extractServerRoutes(serverRs) {
  const map = new Map();
  const routeRe = /\.route\s*\(/g;
  let m;
  while ((m = routeRe.exec(serverRs)) !== null) {
    const openIndex = m.index + m[0].length - 1;
    const inner = sliceBalanced(serverRs, openIndex);
    const pathMatch = inner.match(/"([^"]+)"/);
    if (!pathMatch) continue;
    const routePath = pathMatch[1];
    const methods = map.get(routePath) || new Set();
    const methodRe = /\b(get|post|put|patch|delete|head|options)\s*\(/g;
    let mm;
    while ((mm = methodRe.exec(inner)) !== null) methods.add(mm[1].toUpperCase());
    map.set(routePath, methods);
  }
  if (map.size === 0) throw new Error("未能从 server api.rs 解析出任何 .route(...)");
  return map;
}

/** 从 src-tauri lib.rs 的 generate_handler![...] 提取登记的命令名。 */
function extractInvokeCommands(libRs) {
  const block = libRs.match(/generate_handler!\s*\[([\s\S]*?)\]/);
  if (!block) throw new Error("lib.rs 中找不到 generate_handler![...]");
  const cmds = new Set();
  const re = /commands::([A-Za-z0-9_]+)/g;
  let m;
  while ((m = re.exec(block[1])) !== null) cmds.add(m[1]);
  if (cmds.size === 0) throw new Error("lib.rs invoke_handler 中解析不到命令名");
  return cmds;
}

function main() {
  const apiTs = read(API_TS);
  const screenshotTs = read(SCREENSHOT_DEMO_TS);
  const serverRs = read(SERVER_RS);
  const libRs = read(TAURI_LIB_RS);

  const routes = extractRoutes(apiTs);
  const callCmds = extractCallCommands(apiTs);
  const demoReadCmds = extractDemoReadCommands(apiTs);
  const demoCaseCmds = extractDemoCaseCommands(screenshotTs);
  const serverRoutes = extractServerRoutes(serverRs);
  const invokeCmds = extractInvokeCommands(libRs);

  const routeCmds = new Set(routes.map((r) => r.cmd));
  const errors = [];
  const hints = [];

  // 1) 每个 call("<cmd>") 必须已在桌面端 invoke_handler 登记。
  for (const cmd of [...callCmds].sort()) {
    if (!invokeCmds.has(cmd)) {
      errors.push(
        `[desktop] call("${cmd}") 未在 src-tauri/src/lib.rs 的 invoke_handler 登记 → 桌面端会 command not found`,
      );
    }
  }

  // 2) 每个 call("<cmd>")（桌面专属豁免除外）必须有 ROUTES 条目。
  for (const cmd of [...callCmds].sort()) {
    if (!routeCmds.has(cmd) && !ROUTE_EXEMPT_COMMANDS.has(cmd)) {
      errors.push(
        `[webui] call("${cmd}") 缺少 ROUTES 路由条目 → webui 会抛「暂不支持该操作」`,
      );
    }
  }

  // 3) ROUTES 不得有死路由（没有任何 call("<cmd>") 使用它）。
  for (const cmd of [...routeCmds].sort()) {
    if (!callCmds.has(cmd)) {
      errors.push(`[webui] ROUTES["${cmd}"] 是死路由：没有任何 call("${cmd}") 使用它`);
    }
  }

  // 4) 每个 ROUTES 的 cmd 必须已在桌面端 invoke_handler 登记。
  for (const cmd of [...routeCmds].sort()) {
    if (!invokeCmds.has(cmd)) {
      errors.push(
        `[desktop] ROUTES["${cmd}"] 未在 src-tauri/src/lib.rs 的 invoke_handler 登记 → 桌面端会 command not found`,
      );
    }
  }

  // 5) 每个 ROUTES 的 path + method 必须与服务端路由表一致。
  for (const { cmd, method, path: routePath } of routes) {
    const methods = serverRoutes.get(routePath);
    if (!methods) {
      errors.push(
        `[server] ROUTES["${cmd}"] 的 path "${routePath}" 在 crates/buddy-switch-server/src/api.rs 中不存在`,
      );
      continue;
    }
    if (!methods.has(method)) {
      errors.push(
        `[server] ROUTES["${cmd}"] 以 ${method} 访问 "${routePath}"，但服务端该路由仅支持 [${[...methods]
          .sort()
          .join(", ")}]`,
      );
    }
  }

  // 6) 每个演示只读命令都必须有 screenshotDemoResponse 的 case 分支（漏 case 只在运行时才炸）。
  for (const cmd of [...demoReadCmds].sort()) {
    if (!demoCaseCmds.has(cmd)) {
      errors.push(
        `[demo] DEMO_READ_COMMANDS 含 "${cmd}"，但 src/lib/screenshot-demo.ts 的 screenshotDemoResponse 没有对应 case → 演示模式运行时会抛「演示模式缺少只读数据」`,
      );
    }
  }
  // 反向：case 分支不在 DEMO_READ_COMMANDS 中（可能由 call() 之外的路径演示化）—— 提示级，不失败。
  for (const cmd of [...demoCaseCmds].sort()) {
    if (!demoReadCmds.has(cmd)) {
      hints.push(
        `（提示）screenshot-demo 的 case "${cmd}" 不在 DEMO_READ_COMMANDS 中（可能由 call() 之外的路径演示化）`,
      );
    }
  }

  if (hints.length > 0) {
    for (const hint of hints) process.stdout.write(`${hint}\n`);
  }

  if (errors.length > 0) {
    process.stderr.write(`API 契约校验失败（共 ${errors.length} 项）：\n`);
    for (const error of errors) process.stderr.write(`  - ${error}\n`);
    process.exit(1);
  }

  process.stdout.write(
    `API 契约校验通过：routes=${routes.length}，call=${callCmds.size}，` +
      `server=${serverRoutes.size}，invoke=${invokeCmds.size}，` +
      `demoRead=${demoReadCmds.size}，demoCase=${demoCaseCmds.size}，` +
      `桌面专属豁免=${ROUTE_EXEMPT_COMMANDS.size}\n`,
  );
}

try {
  main();
} catch (error) {
  process.stderr.write(`API 契约校验无法执行：${error && error.message ? error.message : error}\n`);
  process.exit(1);
}
