/**
 * Trae 模块前端类型。
 *
 * **命名约定**：字段名与 Rust 侧响应体逐字一致（camelCase）。Trae 模块刻意让
 * 「磁盘、Tauri 响应、HTTP 响应」三处同形，因此这里不需要任何字段转换——
 * 若某天后端把某个字段改回 snake_case，TypeScript 不会报错但页面会读到
 * `undefined`，所以后端有专门的护栏测试钉住 camelCase（见 core 的
 * `settings_use_camel_case_on_disk_and_on_wire` / `account_view_exposes_camel_case_wire_fields`）。
 */

/** JWT 到期状态。 */
export type TraeJwtStatus = "ok" | "warn" | "expired" | "unknown";

/** 客户端环境状态（`get_trae_env`）。 */
export interface TraeEnvStatus {
  installed: boolean;
  running: boolean;
  version: string | null;
  path: string | null;
  dataDir: string | null;
  dataDirExists: boolean;
  platform: string;
  configuredPath: string | null;
  /**
   * 自动探测到的**产品线变体**稳定标识（`"trae_work"` / `"trae_cn"`）。
   *
   * 自动探测横跨全部变体挑最近活跃的那一个，所以界面必须说得出挑中的是谁。
   * 推不出来时为 `null`（调用方可省略标签，不要显示猜测值）。
   */
  variant: TraeVariantId | null;
  /**
   * 产品线**展示名**（`"Trae Work"` / `"Trae CN"`），可直接上界面。
   *
   * 与 Rust 侧 `variant::TraeVariant::display_name()` 同源；`Trae Work` 这个名字
   * 来自客户端 `product.json` 的 `nameAlias`（`TRAE SOLO CN` 自称 `TraeWork CN`）。
   */
  variantLabel: string | null;
}

/** 产品线变体标识。 */
export type TraeVariantId = "trae_work" | "trae_cn";

/**
 * `TraeVariantId` → 展示名（`"Trae Work"` / `"Trae CN"`）。
 *
 * 与 Rust 侧 `variant::TraeVariant::display_name()` 同源。
 *
 * **什么场景用它**：手上只有变体标识、需要**立刻**显示产品线名时
 * （例如登录弹窗要说清「正在为哪条产品线登录」）。
 * 这类场景**不能**等后端探测结果 —— 变体标识本身就是我们真正要发给后端的东西，
 * 是权威值；后端的 `variantLabel` 只是同一事实的另一种表述。
 *
 * **什么场景不要用它**：能拿到探测结果时优先用后端返回的 `variantLabel`
 * （见 `trae-client.ts` 的 `traeProductLabel`），避免前端多维护一份文案。
 */
export function traeVariantLabel(variant: TraeVariantId): string {
  return variant === "trae_cn" ? "Trae CN" : "Trae Work";
}

/**
 * 单条产品线的独立环境状态（`get_trae_variants().variants[]`）。
 *
 * 与 [`TraeEnvStatus`] 的区别：`TraeEnvStatus` 是**自动挑中的那一条**（单一视角），
 * 本类型是**每一条各自的状态**（并排视角）。界面右上角要同时显示两个产品图标时
 * 用这个，而不是拿 `TraeEnvStatus` 推断另一条——后者根本不知道另一条的存在。
 */
export interface TraeVariantStatus {
  variant: TraeVariantId;
  /** 展示名（`"Trae Work"` / `"Trae CN"`），与 Rust `display_name()` 同源。 */
  variantLabel: string;
  /** 客户端 `product.json` 的官方别名（`"TraeWork CN"` / `"TraeCode CN"`）。 */
  nameAlias: string | null;
  installed: boolean;
  running: boolean;
  version: string | null;
  path: string | null;
  dataDir: string | null;
  dataDirExists: boolean;
}

/** 全部产品线的环境状态（`get_trae_variants`）。 */
export interface TraeVariantsStatus {
  platform: string;
  /** **顺序稳定**（与 Rust `TraeVariant::all()` 一致），可直接按序渲染图标。 */
  variants: TraeVariantStatus[];
}

/** 平台受限能力说明（`get_trae_capabilities`）。 */
export interface TraeUnsupported {
  capability: string;
  label: string;
  supportedOn: string;
  reason: string;
}

/** 平台能力清单。 */
export interface TraeCapabilities {
  platform: string;
  processControl: boolean;
  clientDetection: boolean;
  userDataDir: string | null;
  machineGuidReset: boolean;
  scheduledTask: boolean;
  unsupported: TraeUnsupported[];
}

/** 账号视图。 */
export interface TraeAccount {
  userId: string;
  name: string;
  groupId: string | null;
  jwt: string;
  jwtExpHours: number | null;
  jwtExpTimestamp: number | null;
  jwtStatus: TraeJwtStatus;
  checkedToday: boolean;
  credits: number | null;
  remainingCredits: number | null;
  creditsExpireAt: number | null;
  deviceIdMasked: string | null;
  cooldownType: string | null;
  cooldownUntil: number | null;
  cooldownReason: string | null;
  hasRefreshToken: boolean;
  jwtAutoRefresh: boolean;
  addedAt: string | null;
  updatedAt: string | null;
}

/** 分组视图（含成员数）。 */
export interface TraeGroup {
  id: string;
  name: string;
  color: string;
  order: number;
  count: number;
}

/** 账号页聚合数据（`get_trae_accounts`）。 */
export interface TraeAccountsOverview {
  accounts: TraeAccount[];
  groups: TraeGroup[];
  total: number;
  cooling: number;
  ungrouped: number;
}

/** 单账号签到结果。 */
export interface TraeCheckinOutcome {
  name: string;
  userId: string;
  ok: boolean;
  code: number | null;
  message: string;
  action: string;
  credits: number | null;
  delta: number;
  errorType: string | null;
  cooldownUntil: number | null;
}

/** 签到摘要。 */
export interface TraeCheckinSummary {
  time: string | null;
  results: TraeCheckinOutcome[];
  totalOk: number;
  already: number;
  failed: number;
  warnings: string[];
}

/** 冷却条目。 */
export interface TraeCooldown {
  userId: string;
  type: string;
  until: number;
  reason: string;
  permanent: boolean;
}

/** 签到状态（`get_trae_checkin_status`）。 */
export interface TraeCheckinStatus {
  summary: TraeCheckinSummary;
  summaryIsToday: boolean;
  cooldowns: TraeCooldown[];
  cooldownCount: number;
  logFile: string;
}

/** 一条签到积分明细。 */
export interface TraeCreditRecord {
  date: string;
  userId: string;
  credits: number;
  delta: number;
}

/** 每日积分快照。 */
export interface TraeDailySnapshot {
  date: string;
  total: number;
  earned: number;
  consumed: number;
}

/** 积分总览（`get_trae_credits`）。 */
export interface TraeCreditsOverview {
  remaining: Record<string, number>;
  expireTimes: Record<string, number>;
  updatedAt: string | null;
  balances: { userId: string; credits: number; date: string }[];
  records: TraeCreditRecord[];
  daily: TraeDailySnapshot[];
  todayEarned: number;
  historyDays: number;
}

/** 登录态快照信息。 */
export interface TraeProfileInfo {
  slot: string;
  sizeBytes: number;
  fileCount: number;
  lastModified: string;
  sizeText: string;
}

/** 快照总览（`get_trae_profiles`）。 */
export interface TraeProfilesOverview {
  profiles: TraeProfileInfo[];
  currentAccount: string | null;
  dataDir: string | null;
  clientRunning: boolean;
  coreEntryCount: number;
}

/** Trae 模块设置。 */
export interface TraeSettings {
  proxyPort: number;
  theme: string;
  launchMinimized: boolean;
  autoStartProxy: boolean;
  tray: boolean;
  language: string;
  checkinSkipChecked: boolean;
  checkinSkipExpired: boolean;
  retry: number;
  notify: string;
  traePath: string | null;
  browserPath: string | null;
  logRetentionDays: number;
  proxyDomains: string;
  apiPort: number;
  apiKey: string;
  apiDefaultModel: string;
}

/** 签到报告（`trae_checkin`）。 */
export interface TraeCheckinReport {
  total: number;
  totalOk: number;
  already: number;
  failed: number;
  warnings: string[];
  results: TraeCheckinOutcome[];
}

/** 切换过程中的一步。 */
export interface TraeSwitchStep {
  stage: string;
  status: "ok" | "skip" | "fail";
  message: string;
  time: string;
}

/** 切换结果。 */
export interface TraeSwitchOutcome {
  success: boolean;
  steps: TraeSwitchStep[];
  error: string | null;
}

/** 设备标识重置报告。 */
export interface TraeDeviceResetReport {
  resetCount: number;
  machineId: string;
  guid: string;
  totalLayers: number;
  steps: {
    layer: number;
    label: string;
    status: "ok" | "skip" | "unsupported";
    reason?: string;
    removed?: number;
    cleared?: number;
  }[];
}

// ---------------------------------------------------------------------------
// OAuth 登录（浏览器授权 + 本地回调监听）
// ---------------------------------------------------------------------------
//
// 与 WorkBuddy 的 `OAuthStartResult` / `OAuthPollResult`（`lib/types.ts`）刻意同构：
// 前端交互骨架完全一致，差异只在「多一个 `port` 字段」——Trae 是本机自建回调监听，
// 把端口回传出来便于用户排障（例如防火墙弹窗时能看到到底占了哪个口）。
// **不要为了「统一」把 `port` 去掉**：它是排障时唯一的抓手。

/** 发起登录的结果：前端拿 `verificationUri` 去开浏览器。 */
export interface TraeOAuthStartResult {
  loginId: string;
  /** 授权页 URL（`https://www.trae.cn/authorization?…`）。 */
  verificationUri: string;
  /** 会话有效期（秒）。 */
  expiresIn: number;
  /** 本机回调监听端口（`127.0.0.1:<port>/authorize`）。 */
  port: number;
}

/** 轮询结果：`done` 之后二选一（`account` 或 `error`）。 */
export interface TraeOAuthPollResult {
  done: boolean;
  /** 成功时的账号视图（形状同 {@link TraeAccount}）。 */
  account?: TraeAccount;
  /** 成功时的最新全量账号视图，省掉前端再拉一次列表。 */
  accounts?: TraeAccount[];
  error?: string;
  port?: number;
}

/** 签到进度事件（Tauri 事件 `trae-checkin-progress`）。 */
export type TraeCheckinEvent =
  | { type: "start"; total: number }
  | {
      type: "account";
      index: number;
      userId: string;
      name: string;
      status: "success" | "already" | "fail";
      code?: number;
      message?: string;
      credits?: number | null;
      delta?: number | null;
      errorType?: string | null;
      cooldownUntil?: number | null;
    }
  | { type: "done"; ok: number; already: number; failed: number; total: number };

// ---------------------------------------------------------------------------
// API 网关（OpenAI 兼容）——与 WorkBuddy 网关平行的第二套
// ---------------------------------------------------------------------------
//
// 字段名与 Rust `buddy_switch_gateway::trae` 的序列化输出逐字一致：
// - `TraeGatewayConfig` 为 snake_case（与 WorkBuddy 网关同约定）；
// - `TraeGatewayStatus` 亦为 snake_case（`TraeGatewayStatusView`），
//   由 `normalizeTraeGatewayStatus` 归一为下面这个 camelCase 前端形状。

/** 网关运行配置（落盘 `~/.buddy-switch/trae/api_gateway.json`）。 */
export interface TraeGatewayConfigRaw {
  enabled: boolean;
  bind_addr: string;
  port: number;
  allow_non_loopback: boolean;
  log_keep: number;
  log_bodies: boolean;
  max_body_mb: number;
  default_model: string;
  max_rotate: number;
}

/** 归一化后的网关配置（前端统一用 camelCase）。 */
export interface TraeGatewayConfig {
  enabled: boolean;
  bindAddr: string;
  port: number;
  allowNonLoopback: boolean;
  logKeep: number;
  logBodies: boolean;
  maxBodyMb: number;
  defaultModel: string;
  maxRotate: number;
}

/** 账号池摘要（`pool` 字段）。 */
export interface TraeGatewayPoolSummary {
  total: number;
  available: number;
  cooling: number;
  disabled: number;
  expired: number;
  zeroCredits: number;
  totalCredits: number;
}

/** 池内单个账号的可路由状态。 */
export interface TraeGatewayAccountStatus {
  uid: string;
  name: string;
  status: "available" | "cooling" | "disabled" | "expired" | "no_credits";
  credits: number | null;
  creditsExpireAt: number | null;
  cooling: boolean;
  cooldownUntil: number | null;
  cooldownReason: string | null;
  disabled: boolean;
  deviceIdMasked: string | null;
}

/** 网关运行状态（`trae_gateway_status` 归一化后）。 */
export interface TraeGatewayStatus {
  enabled: boolean;
  running: boolean;
  addr: string | null;
  baseUrl: string;
  bindAddr: string;
  port: number;
  allowNonLoopback: boolean;
  version: string;
  totalRequests: number;
  lastError: string | null;
  /** API Key 脱敏展示（`sk-trae-0123…cdef`），**不含**可用明文。 */
  apiKeyPrefix: string;
  pool: TraeGatewayPoolSummary;
  accounts: TraeGatewayAccountStatus[];
  /** 逐账号「为什么不能路由」的可读串。 */
  diagnose: string[];
  /** 上游主机（`https://trae-api-cn.mchost.guru`）。 */
  upstream: string;
}

/** 网关请求日志（仅元数据；默认不记录正文）。 */
export interface TraeGatewayLogEntry {
  ts: number;
  endpoint: string;
  method: string;
  account: string | null;
  model: string | null;
  status: number;
  latencyMs: number;
  promptTokens?: number | null;
  completionTokens?: number | null;
  stream: boolean;
  error?: string | null;
}

/** 对外暴露的模型条目（`get_trae_gateway_models`）。 */
export interface TraeGatewayModel {
  id: string;
  object: string;
  created: number;
  owned_by: string;
}

// ---------------------------------------------------------------------------
// Token 统计（聚合本机网关请求日志）
// ---------------------------------------------------------------------------

/** 一个统计桶（summary 无 `key`，分组项有）。 */
export interface TraeTokenBucket {
  key?: string;
  /** 输入 + 输出。 */
  total: number;
  input: number;
  output: number;
  records: number;
  errors: number;
  streamRequests: number;
  avgLatencyMs: number;
  p95LatencyMs: number;
}

/** 账号维度的桶（带回表得到的显示名）。 */
export interface TraeTokenAccountBucket extends TraeTokenBucket {
  key: string;
  name: string;
  shortId: string;
}

/**
 * Trae Token 统计（`get_trae_token_statistics`）。
 *
 * **边界**：数据源只有本机网关的请求日志，因此
 * 1. 只统计经过网关的调用，直接在 IDE 里对话不计入；
 * 2. 网关未启用或日志被清空时全为 0；
 * 3. 中途断流的请求没有 `token_usage`，只体现在 `records` 里。
 */
export interface TraeTokenStatistics {
  source: string;
  label: string;
  generatedAt: number;
  rangeDays: number | null;
  logFile: string;
  summary: TraeTokenBucket;
  models: TraeTokenBucket[];
  accounts: TraeTokenAccountBucket[];
  daily: TraeTokenBucket[];
  hours: TraeTokenBucket[];
  statuses: { key: string; records: number }[];
  filesScanned: number;
  parseErrors: number;
  coverageStartAt: number | null;
  coverageEndAt: number | null;
  note: string;
}

// ---------------------------------------------------------------------------
// 运行日志（系统日志页的「运行日志」标签页）
// ---------------------------------------------------------------------------

/** 日志类型：应用级 / 签到 / 登录态切换。 */
export type TraeLogKind = "app" | "checkin" | "switch";

/** 一条运行日志。 */
export interface TraeLogEntry {
  kind: TraeLogKind;
  /** `YYYY-MM-DD HH:MM:SS`；无日期前缀的行为空串。 */
  time: string;
  /** `YYYY-MM-DD`；无前缀的行为空串。 */
  date: string;
  message: string;
}

/** 日志文件状态（用于在页面上标出「文件还不存在」）。 */
export interface TraeLogSource {
  kind: string;
  label: string;
  path: string;
  exists: boolean;
}

/**
 * 运行日志查询参数。
 *
 * `variant` 由 [`getTraeLogs`] 可选注入（与 `kind` / `date` / `keyword` 同层），
 * 缺省时后端按默认产品线读，老调用点行为不变。
 */
export interface TraeLogQuery {
  kind?: string;
  date?: string;
  keyword?: string;
  limit?: number;
  variant?: TraeVariantId;
}

/**
 * 运行日志响应（`get_trae_logs`）。
 *
 * **边界**：只读本机 `logs/` 下的纯文本日志（app / checkin / switcher）。
 * 网关的请求日志是另一份数据（JSON、面向统计），在「网关请求日志」标签页。
 */
export interface TraeLogsResponse {
  entries: TraeLogEntry[];
  /** 过滤后的总条数（可能大于 `entries.length`，因为响应被 `limit` 截断）。 */
  total: number;
  limit: number;
  /** 可选日期，倒序。 */
  dates: string[];
  counts: { all: number; app: number; checkin: number; switch: number };
  sources: TraeLogSource[];
  logDir: string;
  note: string;
}

// ---------------------------------------------------------------------------
// 账号迁移（导出 / 导入）
// ---------------------------------------------------------------------------

/**
 * 导出文件中的一条账号记录。
 *
 * **键名与 Trae 参考实现（`checkin_accounts.json`）一致**，含非常规大写 `UserID`：
 * 本工具既是消费者也是生产者，导出文件要能被参考实现 `device_proxy.py` 读回。
 * 因此这里不是 camelCase —— 与页面内数据形状（`TraeAccount`）刻意不同。
 */
export interface TraeExportRecord {
  UserID?: string;
  name?: string;
  refresh_token?: string;
  jwt?: string;
  added_at?: number;
  [key: string]: unknown;
}

/** 导入文件账号的脱敏预览：**不含 JWT / refresh_token 明文**，只报有无。 */
export interface TraeImportPreviewAccount {
  index: number;
  userId: string | null;
  name: string | null;
  hasJwt: boolean;
  hasRefreshToken: boolean;
}

/** 导入预览响应。 */
export interface TraeImportPreview {
  accounts: TraeImportPreviewAccount[];
  total: number;
}

