import { invoke } from "@tauri-apps/api/core";
import type {
  AccountMeta,
  AccountRecord,
  AccountStrategy,
  AccountStrategyMap,
  ApiKeyRecord,
  AppStatus,
  AutoRotateConfig,
  CatalogSnapshot,
  CodeBuddyCliInstallResult,
  CodeBuddyCliStatus,
  CodeBuddyCliSwitchResult,
  CodeBuddyCnIdeStatus,
  CodeBuddyCnIdeSwitchResult,
  CheckinConfig,
  CheckinLog,
  CheckinResult,
  CreateApiKeyResult,
  CreditExpiry,
  CreditStatistics,
  TokenStatistics,
  CopyResult,
  MigrateResult,
  GatewayConfig,
  GatewayLogEntry,
  GatewayStatus,
  GithubConfig,
  ImportPreviewAccount,
  ImportResult,
  OAuthPollResult,
  OAuthStartResult,
  Region,
  RegionFilter,
  RotateLog,
  RotateStatus,
  ScheduleConfig,
  Session,
  SwitchResult,
  TravelConfig,
  TravelStatus,
  UpdateInfo,
} from "./types";
import { DEMO_UNAVAILABLE_MESSAGE, demoModeEnabled } from "./demo-mode";
import { screenshotDemoResponse } from "./screenshot-demo";

/**
 * 双通道适配层：
 * - 桌面 App（Tauri）：`invoke` 调用 Rust commands
 * - webui（浏览器）：HTTP fetch 调用本地 workbuddy-switch 服务（127.0.0.1）
 */
const API_BASE = "http://127.0.0.1:57890";

const DEMO_READ_COMMANDS = new Set([
  "get_status", "get_accounts", "get_codebuddy_cli_status", "get_codebuddy_cn_ide_status", "get_checkin_status",
  "get_credit_expiry", "get_credit_statistics", "get_auto_checkin_config",
  "get_token_statistics",
  "get_checkin_logs", "get_auto_rotate_config", "rotate_status", "get_rotate_logs",
  "get_github_config", "check_update", "get_launch_at_login_enabled", "switch_progress",
  "get_travel_status", "get_auto_travel_config", "get_schedule_config",
  // API 网关只读命令（演示站需返回虚构数据，否则 build:demo 报错）
  "get_gateway_config", "gateway_status", "list_api_keys", "get_gateway_models",
  "get_account_strategy", "get_gateway_logs",
]);

export function isDemoMode(): boolean {
  return demoModeEnabled;
}

export function isWebui(): boolean {
  return typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window);
}

/** Tauri mobile 也注入内部 API；用现有平台 UA 约定把桌面宿主与移动宿主区分开。 */
function isMobilePlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  const ua = navigator.userAgent;
  return (
    /Android|iPhone|iPad|iPod/i.test(ua) ||
    (ua.includes("Macintosh") && navigator.maxTouchPoints > 1)
  );
}

/** 是否为提供桌面专属能力的 Tauri 宿主。 */
export function isDesktop(): boolean {
  return !isWebui() && !isMobilePlatform();
}

type Route = { method: "GET" | "POST"; path: string };

/** Tauri command → HTTP 路由映射（webui 模式）。 */
const ROUTES: Record<string, Route> = {
  get_status: { method: "GET", path: "/api/status" },
  get_accounts: { method: "GET", path: "/api/accounts" },
  get_codebuddy_cli_status: { method: "GET", path: "/api/codebuddy-cli/status" },
  install_codebuddy_cli_helper: { method: "POST", path: "/api/codebuddy-cli/install-helper" },
  switch_codebuddy_cli_account: { method: "POST", path: "/api/codebuddy-cli/switch" },
  get_codebuddy_cn_ide_status: { method: "GET", path: "/api/codebuddy-cn-ide/status" },
  switch_codebuddy_cn_ide_account: { method: "POST", path: "/api/codebuddy-cn-ide/switch" },
  detect_codebuddy_cn_ide_account: { method: "POST", path: "/api/codebuddy-cn-ide/detect" },
  delete_account: { method: "POST", path: "/api/delete" },
  oauth_start: { method: "POST", path: "/api/oauth/start" },
  oauth_status: { method: "POST", path: "/api/oauth/status" },
  import_local: { method: "POST", path: "/api/import-local" },
  export_accounts: { method: "POST", path: "/api/export-accounts" },
  export_accounts_to_path: { method: "POST", path: "/api/export-accounts-to-path" },
  preview_import_accounts: { method: "POST", path: "/api/import/preview" },
  import_accounts: { method: "POST", path: "/api/import" },
  switch_account: { method: "POST", path: "/api/switch" },
  list_sessions: { method: "GET", path: "/api/sessions" },
  copy_sessions: { method: "POST", path: "/api/sessions/copy" },
  migrate_account_data: { method: "POST", path: "/api/migrate/account" },
  get_checkin_status: { method: "GET", path: "/api/checkin/status" },
  get_credit_expiry: { method: "POST", path: "/api/credits" },
  get_credit_statistics: { method: "GET", path: "/api/credits/stats" },
  get_token_statistics: { method: "GET", path: "/api/token-stats" },
  checkin: { method: "POST", path: "/api/checkin" },
  checkin_all: { method: "POST", path: "/api/checkin/all" },
  get_auto_checkin_config: { method: "GET", path: "/api/checkin/config" },
  save_auto_checkin_config: { method: "POST", path: "/api/checkin/config" },
  get_checkin_logs: { method: "GET", path: "/api/checkin/logs" },
  get_travel_status: { method: "GET", path: "/api/travel/status" },
  get_auto_travel_config: { method: "GET", path: "/api/travel/config" },
  save_auto_travel_config: { method: "POST", path: "/api/travel/config" },
  get_auto_rotate_config: { method: "GET", path: "/api/rotate/config" },
  save_auto_rotate_config: { method: "POST", path: "/api/rotate/config" },
  get_schedule_config: { method: "GET", path: "/api/schedule/config" },
  save_schedule_config: { method: "POST", path: "/api/schedule/config" },
  rotate_status: { method: "GET", path: "/api/rotate/status" },
  run_rotate: { method: "POST", path: "/api/rotate/run" },
  get_rotate_logs: { method: "GET", path: "/api/rotate/logs" },
  refresh_account_token: { method: "POST", path: "/api/refresh-token" },
  get_github_config: { method: "GET", path: "/api/update/config" },
  save_github_config: { method: "POST", path: "/api/update/config" },
  check_update: { method: "GET", path: "/api/update/check" },
  switch_progress: { method: "GET", path: "/api/switch/progress" },
  open_accounts_dir: { method: "POST", path: "/api/accounts/open-dir" },
  // API 网关（对照架构设计 A-3.7）
  get_gateway_config: { method: "GET", path: "/api/gateway/config" },
  save_gateway_config: { method: "POST", path: "/api/gateway/config" },
  gateway_status: { method: "GET", path: "/api/gateway/status" },
  list_api_keys: { method: "GET", path: "/api/gateway/keys" },
  create_api_key: { method: "POST", path: "/api/gateway/keys" },
  revoke_api_key: { method: "POST", path: "/api/gateway/keys/revoke" },
  delete_api_key: { method: "POST", path: "/api/gateway/keys/delete" },
  get_gateway_models: { method: "GET", path: "/api/gateway/models" },
  refresh_gateway_models: { method: "POST", path: "/api/gateway/models/refresh" },
  get_account_strategy: { method: "GET", path: "/api/gateway/strategy" },
  save_account_strategy: { method: "POST", path: "/api/gateway/strategy" },
  get_gateway_logs: { method: "GET", path: "/api/gateway/logs" },
  clear_gateway_logs: { method: "POST", path: "/api/gateway/logs/clear" },
};

function queryString(args?: Record<string, unknown>): string {
  if (!args) return "";
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(args)) {
    if (value === undefined || value === null) continue;
    params.set(key, String(value));
  }
  const text = params.toString();
  return text ? `?${text}` : "";
}

async function httpCall<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const route = ROUTES[cmd];
  if (!route) throw new Error(`webui 模式暂不支持该操作: ${cmd}`);
  let res: Response;
  try {
    const url =
      route.method === "GET"
        ? `${API_BASE}${route.path}${queryString(args)}`
        : `${API_BASE}${route.path}`;
    res = await fetch(url, {
      method: route.method,
      headers: { "Content-Type": "application/json" },
      body: route.method === "POST" ? JSON.stringify(args ?? {}) : undefined,
    });
  } catch {
    throw new Error(`无法连接 workbuddy-switch 服务（${API_BASE}），请先运行 \`workbuddy-switch\``);
  }
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(data.message || data.error || `请求失败 (${res.status})`);
  }
  return data as T;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (demoModeEnabled) {
    if (cmd === "get_credit_statistics" && args?.refresh === true) {
      throw new Error(DEMO_UNAVAILABLE_MESSAGE);
    }
    if (!DEMO_READ_COMMANDS.has(cmd)) throw new Error(DEMO_UNAVAILABLE_MESSAGE);
    return screenshotDemoResponse(cmd, args) as T;
  }
  if (!isWebui()) return invoke<T>(cmd, args);
  return httpCall<T>(cmd, args);
}

// ---------------------------------------------------------------------------
// 状态 / 账号
// ---------------------------------------------------------------------------

/** region 作为普通字段放进 args：GET 走 query、POST 走 JSON body（B-8.4）。缺省不传即后端按 cn 处理。 */
function regionArg(region?: Region): Record<string, unknown> {
  return region ? { region } : {};
}

export function getStatus(region?: Region): Promise<AppStatus> {
  return call("get_status", region ? { region } : undefined);
}

export function getAccounts(region?: Region): Promise<{ accounts: AccountMeta[] }> {
  return call("get_accounts", region ? { region } : undefined);
}

export function getCodebuddyCliStatus(): Promise<CodeBuddyCliStatus> {
  return call("get_codebuddy_cli_status");
}

export function installCodebuddyCliHelper(): Promise<CodeBuddyCliInstallResult> {
  return call("install_codebuddy_cli_helper");
}

export function switchCodebuddyCliAccount(accountId: string): Promise<CodeBuddyCliSwitchResult> {
  if (demoModeEnabled) {
    return new Promise((resolve, reject) => {
      window.setTimeout(() => {
        try {
          resolve(screenshotDemoResponse("switch_codebuddy_cli_account", { accountId }) as CodeBuddyCliSwitchResult);
        } catch (error) {
          reject(error);
        }
      }, 1200);
    });
  }
  return call("switch_codebuddy_cli_account", { accountId });
}

export function getCodebuddyCnIdeStatus(): Promise<CodeBuddyCnIdeStatus> {
  return call("get_codebuddy_cn_ide_status");
}

export function switchCodebuddyCnIdeAccount(
  accountId: string,
  restart = true,
): Promise<CodeBuddyCnIdeSwitchResult> {
  return call("switch_codebuddy_cn_ide_account", { accountId, restart });
}

export function detectCodebuddyCnIdeAccount(): Promise<{
  ok: boolean;
  found: boolean;
  matched?: boolean;
  accountId?: string;
  message?: string;
}> {
  return call("detect_codebuddy_cn_ide_account");
}


export function deleteAccount(accountId: string, region?: Region): Promise<{ ok: boolean }> {
  return call("delete_account", { accountId, ...regionArg(region) });
}

export function oauthStart(region?: Region): Promise<OAuthStartResult> {
  return call("oauth_start", region ? { region } : undefined);
}

export function oauthStatus(loginId: string, region?: Region): Promise<OAuthPollResult> {
  return call("oauth_status", { loginId, ...regionArg(region) });
}

export function importLocal(region?: Region): Promise<{ ok: boolean; account: AccountMeta }> {
  return call("import_local", region ? { region } : undefined);
}

export function exportAccounts(accountIds: string[], region?: Region): Promise<{ ok: boolean; accounts: AccountRecord[] }> {
  return call("export_accounts", { accountIds, ...regionArg(region) });
}

/** 桌面端：把完整记录写入用户选择的路径（系统保存对话框产物）。 */
export function exportAccountsToPath(
  accountIds: string[],
  path: string,
  region?: Region,
): Promise<{ ok: boolean; path: string }> {
  return call("export_accounts_to_path", { accountIds, path, ...regionArg(region) });
}

export function previewImportAccounts(
  fileText: string,
  region?: Region,
): Promise<{ accounts: ImportPreviewAccount[]; total: number }> {
  return call("preview_import_accounts", { fileText, ...regionArg(region) });
}

export function importAccounts(fileText: string, indexes: number[], region?: Region): Promise<ImportResult> {
  return call("import_accounts", { fileText, indexes, ...regionArg(region) });
}

export function switchAccount(args: {
  accountId: string;
  region?: Region;
  restart?: boolean;
  shareSessions?: boolean;
  copySessionIds?: string[];
}): Promise<SwitchResult> {
  return call("switch_account", args as unknown as Record<string, unknown>);
}

/** 切换进度（webui 轮询用；桌面端走事件，此函数无副作用）。 */
export function switchProgress(): Promise<{ running: boolean; progress: string | null }> {
  return call("switch_progress");
}

export function listSessions(region?: Region): Promise<{
  sessions: Session[];
  current: string | null;
}> {
  return call("list_sessions", region ? { region } : undefined);
}

export function copySessions(
  targetAccountId: string,
  sessionIds: string[],
  region?: Region,
): Promise<{
  sourceUid: string;
  targetUid: string;
  copied: CopyResult[];
  skipped?: CopyResult[];
  errors?: { id: string; error: string }[];
}> {
  return call("copy_sessions", { targetAccountId, sessionIds, ...regionArg(region) });
}

/**
 * 把源账号的 Memory / Connector 合并到目标账号（带去重）。
 *
 * 只处理普通文件，不触碰 `workbuddy.db`，因此无需关闭 WorkBuddy。
 * `sourceAccountId` 缺省时取当前登录账号；`memory` / `connectors` 缺省均为 true。
 */
export function migrateAccountData(
  targetAccountId: string,
  options?: {
    sourceAccountId?: string;
    memory?: boolean;
    connectors?: boolean;
    region?: Region;
  },
): Promise<MigrateResult> {
  const { region, ...rest } = options ?? {};
  return call("migrate_account_data", {
    targetAccountId,
    ...rest,
    ...regionArg(region),
  });
}

/** 打开系统设置授权面板（桌面端专用；webui 模式由服务进程权限决定，无操作）。 */
export function openPermissionSettings(
  target?: "app_management" | "all_files",
): Promise<void> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  if (isWebui()) return Promise.resolve();
  return call("open_permission_settings", { target: target ?? "app_management" });
}

/** 权限自检：桌面端写探针；webui 模式由服务进程权限决定。 */
export function checkAuthPermission(): Promise<{
  ok: boolean;
  message?: string;
  error?: string;
  dir?: string;
  hint?: string;
}> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  if (isWebui()) {
    return Promise.resolve({
      ok: true,
      message: "webui 模式由服务进程（终端启动）的权限决定，无需额外授权",
      hint: "",
    });
  }
  return call("check_auth_permission");
}

/** 在 Finder 中显示当前 App（桌面端专用；webui 无操作）。 */
export function revealAppInFinder(): Promise<void> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  if (isWebui()) return Promise.resolve();
  return call("reveal_app_in_finder");
}

// ---------------------------------------------------------------------------
// 阶段 3：签到 + token 刷新
// ---------------------------------------------------------------------------

export async function getCheckinStatus(accountId: string, region?: Region): Promise<{
  ok: boolean;
  todayCheckedIn: boolean;
  error?: string;
  raw?: unknown;
}> {
  if (demoModeEnabled) {
    return screenshotDemoResponse("get_checkin_status", { accountId, ...regionArg(region) }) as {
      ok: boolean;
      todayCheckedIn: boolean;
      error?: string;
      raw?: unknown;
    };
  }
  if (isWebui()) {
    // webui 端为批量接口，按 accountId 过滤
    const all = await httpCall<{
      accounts: {
        accountId: string;
        email: string;
        ok: boolean;
        todayCheckedIn: boolean;
        error?: string;
        raw?: unknown;
      }[];
    }>("get_checkin_status", region ? { region } : undefined);
    const one = all.accounts.find((a) => a.accountId === accountId);
    return one
      ? { ok: one.ok, todayCheckedIn: one.todayCheckedIn, error: one.error, raw: one.raw }
      : { ok: false, todayCheckedIn: false, error: "未找到账号" };
  }
  return call("get_checkin_status", { accountId, ...regionArg(region) });
}

export function getCreditExpiry(accountId: string, region?: Region): Promise<CreditExpiry> {
  return call("get_credit_expiry", { accountId, ...regionArg(region) });
}

export function getCreditStatistics(refresh = false, region?: RegionFilter): Promise<CreditStatistics> {
  const args: Record<string, unknown> = {};
  if (refresh) args.refresh = true;
  if (region) args.region = region;
  return call("get_credit_statistics", Object.keys(args).length > 0 ? args : undefined);
}

export function getTokenStatistics(days?: number, region?: RegionFilter): Promise<TokenStatistics> {
  const args: Record<string, unknown> = {};
  if (days) args.days = days;
  if (region) args.region = region;
  return call("get_token_statistics", Object.keys(args).length > 0 ? args : undefined);
}

export function checkin(accountId: string, region?: Region): Promise<CheckinResult> {
  return call("checkin", { accountId, ...regionArg(region) });
}

export function checkinAll(region?: Region): Promise<{
  accounts: { accountId: string; email: string; result: string; error?: string }[];
  status?: string;
  reason?: string;
}> {
  return call("checkin_all", region ? { region } : undefined);
}

export function getAutoCheckinConfig(): Promise<CheckinConfig> {
  return call("get_auto_checkin_config");
}

export function saveAutoCheckinConfig(config: CheckinConfig): Promise<CheckinConfig> {
  return call("save_auto_checkin_config", {
    config: config as unknown as Record<string, unknown>,
  });
}

export function getCheckinLogs(): Promise<{ logs: CheckinLog[] }> {
  return call("get_checkin_logs");
}

export async function getTravelStatus(accountId: string, region?: Region): Promise<TravelStatus> {
  if (demoModeEnabled) {
    return screenshotDemoResponse("get_travel_status", { accountId, ...regionArg(region) }) as TravelStatus;
  }
  if (isWebui()) {
    // webui 端为批量接口，按 accountId 过滤
    const all = await httpCall<{
      accounts: { accountId: string; email: string; label: TravelStatus["label"]; rewardCredit: number | null; locationName?: string | null; arriveAt?: number | null }[];
    }>("get_travel_status", region ? { region } : undefined);
    const one = all.accounts.find((a) => a.accountId === accountId);
    return one
      ? { label: one.label, rewardCredit: one.rewardCredit, locationName: one.locationName ?? null, arriveAt: one.arriveAt ?? null }
      : { label: "untraveled", rewardCredit: null, locationName: null, arriveAt: null };
  }
  return call("get_travel_status", { accountId, ...regionArg(region) });
}

export function getAutoTravelConfig(): Promise<TravelConfig> {
  return call("get_auto_travel_config");
}

export function saveAutoTravelConfig(config: TravelConfig): Promise<TravelConfig> {
  return call("save_auto_travel_config", {
    config: config as unknown as Record<string, unknown>,
  });
}

export function getAutoRotateConfig(): Promise<AutoRotateConfig> {
  return call("get_auto_rotate_config");
}

export function saveAutoRotateConfig(config: AutoRotateConfig): Promise<AutoRotateConfig> {
  return call("save_auto_rotate_config", {
    config: config as unknown as Record<string, unknown>,
  });
}

// ---------------------------------------------------------------------------
// 定时任务排程（六类任务，全局单份，无需 region）
// ---------------------------------------------------------------------------

export function getScheduleConfig(): Promise<ScheduleConfig> {
  return call("get_schedule_config");
}

export function saveScheduleConfig(config: ScheduleConfig): Promise<ScheduleConfig> {
  return call("save_schedule_config", {
    config: config as unknown as Record<string, unknown>,
  });
}

export function getRotateStatus(): Promise<RotateStatus> {
  return call("rotate_status");
}

export function runRotate(): Promise<{ status: string; reason?: string; error?: string; to?: string }> {
  return call("run_rotate");
}

export function getRotateLogs(): Promise<{ logs: RotateLog[] }> {
  return call("get_rotate_logs");
}

export function refreshAccountToken(accountId: string, region?: Region): Promise<AccountMeta> {
  return call("refresh_account_token", { accountId, ...regionArg(region) });
}

// ---------------------------------------------------------------------------
// API 网关（对照架构设计 A-3.7）
// ---------------------------------------------------------------------------

export function getGatewayConfig(): Promise<GatewayConfig> {
  return call("get_gateway_config");
}

export function saveGatewayConfig(config: GatewayConfig): Promise<GatewayConfig> {
  return call("save_gateway_config", { config: config as unknown as Record<string, unknown> });
}

export function gatewayStatus(): Promise<GatewayStatus> {
  return call("gateway_status");
}

export function listApiKeys(): Promise<{ keys: ApiKeyRecord[] }> {
  return call("list_api_keys");
}

export function createApiKey(name: string, region: Region): Promise<CreateApiKeyResult> {
  return call("create_api_key", { name, region });
}

export function revokeApiKey(id: string): Promise<{ ok: boolean }> {
  return call("revoke_api_key", { id });
}

export function deleteApiKey(id: string): Promise<{ ok: boolean }> {
  return call("delete_api_key", { id });
}

export function getGatewayModels(region: Region): Promise<CatalogSnapshot> {
  return call("get_gateway_models", { region });
}

export function refreshGatewayModels(region: Region): Promise<CatalogSnapshot> {
  return call("refresh_gateway_models", { region });
}

export function getAccountStrategy(): Promise<AccountStrategyMap> {
  return call("get_account_strategy");
}

export function saveAccountStrategy(region: Region, strategy: AccountStrategy): Promise<{ ok: boolean }> {
  return call("save_account_strategy", {
    region,
    strategy: strategy as unknown as Record<string, unknown>,
  });
}

export function getGatewayLogs(): Promise<{ logs: GatewayLogEntry[] }> {
  return call("get_gateway_logs");
}

export function clearGatewayLogs(): Promise<{ ok: boolean }> {
  return call("clear_gateway_logs");
}

/** 在系统文件管理器中打开该版本的账号库所在目录（设置页）。 */
export function openAccountsDir(region?: Region): Promise<{ ok: boolean }> {
  return call("open_accounts_dir", region ? { region } : undefined);
}

// ---------------------------------------------------------------------------
// 阶段 4：自动更新
// ---------------------------------------------------------------------------

export function getGithubConfig(): Promise<GithubConfig> {
  return call("get_github_config");
}

export function saveGithubConfig(config: GithubConfig): Promise<GithubConfig> {
  return call("save_github_config", {
    config: config as unknown as Record<string, unknown>,
  });
}

export function checkUpdate(proxy?: string, force?: boolean): Promise<UpdateInfo> {
  return call("check_update", { proxy: proxy?.trim() || null, force: force ?? false });
}

/** 重启 App（桌面端专用；webui 无操作）。守卫在 wrapper 内部，保证「webui 不可达」由本函数自证。 */
export function relaunchApp(): Promise<void> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  if (isWebui()) return Promise.resolve();
  return call("relaunch_app");
}

// ---------------------------------------------------------------------------
// 开机自启（仅桌面端；webui 不提供同名接口，卡片也不在 webui 渲染）
// ---------------------------------------------------------------------------

/** 查询系统当前的开机自启注册状态（桌面端）。 */
export function getLaunchAtLoginEnabled(): Promise<boolean> {
  if (demoModeEnabled) return call("get_launch_at_login_enabled");
  if (!isDesktop()) return Promise.resolve(false);
  return call("get_launch_at_login_enabled");
}

/** 注册 / 移除系统开机自启，返回回读后的权威状态（桌面端）。 */
export function setLaunchAtLoginEnabled(enabled: boolean): Promise<boolean> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  if (!isDesktop()) return Promise.resolve(false);
  return call("set_launch_at_login_enabled", { enabled });
}

/** 把 Tauri command / HTTP 抛出的错误统一为 Error。 */
export function asError(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e ?? "未知错误");
}
