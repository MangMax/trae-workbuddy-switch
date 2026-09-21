import { useCallback, useEffect, useState } from "react";
import {
  AlertTriangle,
  Copy,
  Download,
  Eraser,
  ExternalLink,
  FileText,
  HardDriveDownload,
  HardDriveUpload,
  Info,
  Loader2,
  RefreshCw,
  RotateCcw,
  Save,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { toast } from "sonner";

import { DemoAction } from "@/components/demo-action";
import { TraeVariantSwitch } from "@/components/trae-variant-switch";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { SettingsFieldRow, SettingsGroup, SettingsRow } from "@/components/settings-primitives";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import { normalizeTraeGatewayLogs } from "@/lib/trae-gateway";
import { isAutoDetected, traeProductLabel } from "@/lib/trae-client";
import { useTraeVariant } from "@/lib/use-trae-variant";
import type {
  TraeCapabilities,
  TraeDeviceResetReport,
  TraeEnvStatus,
  TraeGatewayLogEntry,
  TraeLogKind,
  TraeLogsResponse,
  TraeProfileInfo,
  TraeProfilesOverview,
  TraeSettings,
  TraeVariantId,
} from "@/lib/trae-types";
import { cn } from "@/lib/utils";
import { useCachedResource } from "@/lib/use-cached-resource";

// ---------------------------------------------------------------------------
// 板块骨架：与 WorkBuddy 的 SettingsPage 共用同一实现
// （`SettingsGroup` / `SettingsRow` / `SettingsFieldRow` 从共享模块导入）
// ---------------------------------------------------------------------------

/** 能力徽章。 */
function CapabilityBadge({ label, supported }: { label: string; supported: boolean }) {
  return (
    <Badge variant={supported ? "secondary" : "outline"} className={cn(!supported && "text-muted-foreground")}>
      {label}
      {supported ? "" : "（不支持）"}
    </Badge>
  );
}

// ---------------------------------------------------------------------------
// 运行日志（原「系统日志」页的运行日志 Tab）
// ---------------------------------------------------------------------------

const KIND_OPTIONS = [
  { value: "all", label: "全部" },
  { value: "app", label: "运行" },
  { value: "checkin", label: "签到" },
  { value: "switch", label: "切换" },
] as const;

const KIND_TONE: Record<TraeLogKind, "default" | "success" | "warning"> = {
  app: "default",
  checkin: "success",
  switch: "warning",
};

const KIND_LABEL: Record<TraeLogKind, string> = {
  app: "运行",
  checkin: "签到",
  switch: "切换",
};

/**
 * 运行日志分组。
 *
 * 原先是一个独立的「系统日志」导航项；既然侧栏收敛到与 WorkBuddy 同构的五项，
 * 日志作为「排障用的设置类信息」下沉到设置页，能力本身完整保留
 * （类型/日期/关键字筛选、自动刷新、复制、导出 CSV）。
 */
function RuntimeLogsSection() {
  // 运行日志按产品线分家（`checkin` / `switch` 两条来源各读各的，`app` 刻意共用）。
  // 变体取自侧栏分区，不由探测推导 —— 探测回答「本机哪条线最近活跃」，
  // 不回答「用户此刻想管哪条线」。
  const [variant] = useTraeVariant();
  const [kind, setKind] = useState<string>("all");
  const [date, setDate] = useState<string>("all");
  const [keywordDraft, setKeywordDraft] = useState("");
  const [keyword, setKeyword] = useState("");
  const [autoRefresh, setAutoRefresh] = useState(true);

  /**
   * 键里带上四个筛选条件（含变体）：任何一个变了都是**另一份**结果，
   * 漏在键外就会出现「换了日期、列表还是旧的」。
   */
  const load = useCallback(
    () =>
      api.getTraeLogs({
        kind,
        date: date === "all" ? undefined : date,
        keyword: keyword || undefined,
        variant,
      }),
    [kind, date, keyword, variant],
  );

  const {
    data,
    loading,
    error,
    refresh,
  } = useCachedResource<TraeLogsResponse>(
    `trae:logs:${variant}:${kind}:${date}:${keyword}`,
    load,
  );

  // 自动刷新只在「没有未提交的输入」时跑：否则用户正在输入关键字，
  // 每次刷新都会把列表换成旧条件的结果，看起来像在抖。
  const dirty = keywordDraft !== keyword;
  useEffect(() => {
    if (!autoRefresh || dirty) return;
    const timer = setInterval(() => void refresh(), 2000);
    return () => clearInterval(timer);
  }, [autoRefresh, dirty, refresh]);

  const entries = data?.entries ?? [];
  const counts = data?.counts;

  const copyAll = async () => {
    if (entries.length === 0) return;
    await copyText(entries.map((entry) => `[${entry.time}] [${entry.kind}] ${entry.message}`).join("\n"));
  };

  const exportCsv = () => {
    if (entries.length === 0) return;
    const header = "时间\t类型\t内容\n";
    const body = entries
      .map((entry) => `${entry.time}\t${KIND_LABEL[entry.kind] ?? entry.kind}\t${entry.message}`)
      .join("\n");
    // BOM 前缀：Excel 不带它会按本地代码页解读，中文全乱。
    const blob = new Blob([`\ufeff${header}${body}`], { type: "text/csv;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = `trae-logs-${new Date().toISOString().slice(0, 10)}.csv`;
    anchor.click();
    URL.revokeObjectURL(url);
  };

  return (
    <SettingsGroup id="trae-settings-logs" title="运行日志">
      <CardContent className="space-y-0 p-0">
      <div className="p-4 sm:p-5">
        <div className="flex flex-wrap items-center gap-2">
          <Select value={kind} onValueChange={setKind}>
            <SelectTrigger size="sm" className="w-28">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {KIND_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Select value={date} onValueChange={setDate}>
            <SelectTrigger size="sm" className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">全部日期</SelectItem>
              {(data?.dates ?? []).map((day) => (
                <SelectItem key={day} value={day}>
                  {day}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <div className="relative min-w-[180px] flex-1">
            <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={keywordDraft}
              onChange={(event) => setKeywordDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") setKeyword(keywordDraft.trim());
              }}
              placeholder="搜索关键字…"
              className="h-8 pl-8"
            />
          </div>

          <Button variant="outline" size="sm" onClick={() => setKeyword(keywordDraft.trim())}>
            <Search />
            查询
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              setKeywordDraft("");
              setKeyword("");
              setKind("all");
              setDate("all");
            }}
          >
            <X />
            重置
          </Button>
        </div>

        <div className="mt-3 flex flex-wrap items-center justify-between gap-3 border-t border-border/60 pt-3">
          <div className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
            <span className="flex items-center gap-1.5">
              <Switch checked={autoRefresh} onCheckedChange={setAutoRefresh} aria-label="自动刷新日志" />
              自动刷新
            </span>
            {counts && (
              <span className="flex flex-wrap items-center gap-1.5">
                <span>共 {counts.all} 条</span>
                <Badge variant="secondary">运行 {counts.app}</Badge>
                <Badge variant="success">签到 {counts.checkin}</Badge>
                <Badge variant="warning">切换 {counts.switch}</Badge>
              </span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <Button variant="ghost" size="sm" disabled={entries.length === 0} onClick={() => void copyAll()}>
              <Copy />
              复制
            </Button>
            <Button variant="ghost" size="sm" disabled={entries.length === 0} onClick={exportCsv}>
              <Download />
              导出 CSV
            </Button>
            <Button variant="outline" size="sm" disabled={loading} onClick={() => void refresh()}>
              {loading ? <Loader2 className="animate-spin" /> : <RefreshCw />}
              刷新
            </Button>
          </div>
        </div>
      </div>

      {error && (
        <div className="px-4 pb-4 sm:px-5">
          <Alert variant="destructive">
            <AlertTriangle />
            <AlertTitle>读取日志失败</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        </div>
      )}

      <div className="border-t border-border/60">
        <div className="flex items-center justify-between border-b border-border/60 px-4 py-2.5 sm:px-5">
          <span className="flex items-center gap-2 text-[13px] font-medium">
            <FileText className="size-4 text-muted-foreground" />
            日志明细
          </span>
          <span className="text-xs text-muted-foreground">
            {data ? `显示 ${entries.length} / ${data.total} 条` : "加载中…"}
          </span>
        </div>

        <div className="max-h-[420px] overflow-auto">
          {loading && !data ? (
            <div className="space-y-2 p-4">
              {Array.from({ length: 6 }, (_, index) => (
                <Skeleton key={index} className="h-5 w-full" />
              ))}
            </div>
          ) : entries.length === 0 ? (
            <div className="flex flex-col items-center gap-2 px-4 py-12 text-center">
              <Trash2 className="size-7 text-muted-foreground/50" />
              <p className="text-sm font-medium">暂无日志</p>
              <p className="max-w-md text-xs text-muted-foreground">
                日志文件还不存在或当前筛选条件下没有匹配行。运行一次签到或切换账号后，
                这里会开始出现记录。
              </p>
            </div>
          ) : (
            <ul className="divide-y divide-border/60">
              {entries.map((entry, index) => (
                <li
                  key={`${entry.time}-${index}`}
                  className="flex items-start gap-3 px-4 py-2 font-mono text-xs hover:bg-muted/40 sm:px-5"
                >
                  <span className="shrink-0 text-muted-foreground">{entry.time || "（无时间）"}</span>
                  <Badge variant={KIND_TONE[entry.kind] ?? "secondary"} className="shrink-0 font-sans">
                    {KIND_LABEL[entry.kind] ?? entry.kind}
                  </Badge>
                  <span className="break-all leading-5">{entry.message}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      {/* 边界说明与文件状态：不写清楚，用户会把「日志是空的」当成 bug。 */}
      <div className="border-t border-border/60 px-4 py-3 sm:px-5">
        <p className="text-xs leading-5 text-muted-foreground">
          {data?.note ?? "只读取本机纯文本运行日志。"}
        </p>
        <ul className="mt-2 space-y-1">
          {(data?.sources ?? []).map((source) => (
            <li key={source.kind} className="flex flex-wrap items-center gap-2">
              <Badge variant={source.exists ? "success" : "outline"}>
                {source.exists ? "存在" : "未生成"}
              </Badge>
              <span className="break-all font-mono text-[11px] text-muted-foreground">{source.path}</span>
            </li>
          ))}
        </ul>
        {data?.logDir && (
          <p className="mt-2 break-all font-mono text-[11px] text-muted-foreground">
            日志目录：{data.logDir}
          </p>
        )}
      </div>
      </CardContent>
    </SettingsGroup>
  );
}

// ---------------------------------------------------------------------------
// 网关请求日志（原「系统日志」页的网关 Tab）
// ---------------------------------------------------------------------------

const STATUS_OPTIONS = [
  { value: "all", label: "全部状态" },
  { value: "ok", label: "成功 (2xx)" },
  { value: "client", label: "客户端错误 (4xx)" },
  { value: "server", label: "服务端错误 (5xx)" },
] as const;

function formatClock(ts: number): string {
  if (!ts) return "—";
  const date = new Date(ts);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

function formatTokens(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  if (value >= 1000) return `${(value / 1000).toFixed(1)}K`;
  return String(value);
}

function statusTone(status: number): string {
  if (status >= 200 && status < 300) return "text-emerald-600 dark:text-emerald-400";
  if (status >= 400) return "text-destructive";
  return "text-muted-foreground";
}

/**
 * 网关请求日志分组。
 *
 * 与「Token 统计」页同源（同一份 `api_gateway_logs.json`），但用途不同：
 * 统计页回答「用了多少」，这里回答「刚刚那次请求发生了什么」。
 * 提供状态码筛选、关键字搜索、逐条详情抽屉与清空。
 */
function GatewayLogsSection() {
  const [statusFilter, setStatusFilter] = useState<string>("all");
  const [keyword, setKeyword] = useState("");
  const [detail, setDetail] = useState<TraeGatewayLogEntry | null>(null);
  const [clearing, setClearing] = useState(false);
  /**
   * 「清空日志」失败的文案。
   *
   * 读侧的错误归快照缓存所有（`error` 只读），而清空是**动作**、它的失败没有快照
   * 可挂，因此单独一个本地状态。两者共用同一个提示位，与改造前的表现一致。
   */
  const [actionError, setActionError] = useState<string | null>(null);

  /** 日志已在加载器里归一化：消费方拿到的一定是数组，不必各自兜底。 */
  const load = useCallback(async () => normalizeTraeGatewayLogs(await api.getTraeGatewayLogs()), []);

  const {
    data,
    loading,
    error,
    refresh,
    patch,
  } = useCachedResource<TraeGatewayLogEntry[]>("trae:gateway-logs", load);
  const logs = data ?? [];
  const failure = error ?? actionError;

  const clear = async () => {
    setClearing(true);
    try {
      await api.clearTraeGatewayLogs();
      setActionError(null);
      patch(() => []);
      toast.success("日志已清空");
    } catch (e) {
      setActionError(api.asError(e));
    } finally {
      setClearing(false);
    }
  };

  const visible = logs
    .filter((entry) => {
      if (statusFilter === "ok") return entry.status >= 200 && entry.status < 300;
      if (statusFilter === "client") return entry.status >= 400 && entry.status < 500;
      if (statusFilter === "server") return entry.status >= 500;
      return true;
    })
    .filter((entry) => {
      const needle = keyword.trim().toLowerCase();
      if (!needle) return true;
      return [entry.account, entry.model, entry.endpoint, entry.error]
        .filter((value): value is string => typeof value === "string")
        .some((value) => value.toLowerCase().includes(needle));
    })
    // 最新在前：排查问题看的是「刚刚那次」，而不是最早那次。
    .sort((left, right) => right.ts - left.ts);

  const totalTokens = visible.reduce(
    (sum, entry) => sum + (entry.promptTokens ?? 0) + (entry.completionTokens ?? 0),
    0,
  );

  return (
    <SettingsGroup id="trae-settings-gateway-logs" title="网关请求日志">
      <CardContent className="space-y-0 p-0">
      <div className="p-4 sm:p-5">
        <div className="flex flex-wrap items-center gap-2">
          <Select value={statusFilter} onValueChange={setStatusFilter}>
            <SelectTrigger size="sm" className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {STATUS_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <div className="relative min-w-[180px] flex-1">
            <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={keyword}
              onChange={(event) => setKeyword(event.target.value)}
              placeholder="搜索账号 / 模型 / 端点 / 错误…"
              className="h-8 pl-8"
            />
          </div>
          <Button variant="outline" size="sm" disabled={loading} onClick={() => void refresh()}>
            {loading ? <Loader2 className="animate-spin" /> : <RefreshCw />}
            刷新
          </Button>
          <DemoAction>
            <Button
              variant="ghost"
              size="sm"
              disabled={clearing || logs.length === 0}
              onClick={() => void clear()}
            >
              {clearing ? <Loader2 className="animate-spin" /> : <Eraser />}
              清空
            </Button>
          </DemoAction>
        </div>
        <p className="mt-3 border-t border-border/60 pt-3 text-xs text-muted-foreground">
          共 {visible.length} 条
          {visible.length !== logs.length && `（已从 ${logs.length} 条中筛选）`}
          {totalTokens > 0 && ` · 合计 ${formatTokens(totalTokens)} Token`}
          。网关未启用时这里是空的——请求日志只由网关写入。
        </p>
      </div>

      {failure && (
        <div className="px-4 pb-4 sm:px-5">
          <Alert variant="destructive">
            <AlertTriangle />
            <AlertTitle>读取网关日志失败</AlertTitle>
            <AlertDescription>{failure}</AlertDescription>
          </Alert>
        </div>
      )}

      <div className="max-h-[420px] overflow-auto border-t border-border/60">
        {loading && logs.length === 0 ? (
          <div className="space-y-2 p-4">
            {Array.from({ length: 5 }, (_, index) => (
              <Skeleton key={index} className="h-6 w-full" />
            ))}
          </div>
        ) : visible.length === 0 ? (
          <div className="flex flex-col items-center gap-2 px-4 py-12 text-center">
            <Trash2 className="size-7 text-muted-foreground/50" />
            <p className="text-sm font-medium">暂无网关请求日志</p>
            <p className="max-w-md text-xs text-muted-foreground">
              在「API 服务」页启用 Trae 网关后，外部客户端发来的每一次请求都会记在这里。
            </p>
          </div>
        ) : (
          <table className="w-full text-sm">
            <thead className="sticky top-0 bg-muted/60 text-xs text-muted-foreground backdrop-blur">
              <tr>
                <th className="px-4 py-2 text-left font-medium">时间</th>
                <th className="px-3 py-2 text-left font-medium">账号</th>
                <th className="px-3 py-2 text-left font-medium">模型</th>
                <th className="px-3 py-2 text-left font-medium">状态</th>
                <th className="px-3 py-2 text-right font-medium">耗时</th>
                <th className="px-3 py-2 text-right font-medium">Token</th>
              </tr>
            </thead>
            <tbody>
              {visible.map((entry, index) => (
                <tr
                  key={`${entry.ts}-${index}`}
                  className="cursor-pointer border-t border-border/60 hover:bg-muted/40"
                  onClick={() => setDetail(entry)}
                >
                  <td className="whitespace-nowrap px-4 py-2 font-mono text-xs text-muted-foreground">
                    {formatClock(entry.ts)}
                  </td>
                  <td className="px-3 py-2 text-xs">{entry.account ?? "—"}</td>
                  <td className="px-3 py-2 text-xs">{entry.model ?? "—"}</td>
                  <td className={cn("px-3 py-2 font-mono text-xs", statusTone(entry.status))}>
                    {entry.status || "—"}
                  </td>
                  <td className="px-3 py-2 text-right text-xs tabular-nums text-muted-foreground">
                    {entry.latencyMs ? `${entry.latencyMs} ms` : "—"}
                  </td>
                  <td className="px-3 py-2 text-right text-xs tabular-nums text-muted-foreground">
                    {entry.promptTokens === null && entry.completionTokens === null
                      ? "—"
                      : `${formatTokens(entry.promptTokens)} / ${formatTokens(entry.completionTokens)}`}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="border-t border-border/60 px-4 py-2.5 text-xs text-muted-foreground sm:px-5">
        点击任意一行查看完整字段。网关默认只记元数据（模型、状态、耗时、token 数）。
      </div>

      <Dialog open={detail !== null} onOpenChange={(open) => !open && setDetail(null)}>
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>请求详情</DialogTitle>
            <DialogDescription>
              网关只记录元数据。若需要正文，请到「API 服务」页开启「记录请求正文」。
            </DialogDescription>
          </DialogHeader>
          <pre className="max-h-[50vh] overflow-auto whitespace-pre-wrap break-all rounded-lg bg-muted/50 p-3 font-mono text-xs">
            {detail ? JSON.stringify(detail, null, 2) : ""}
          </pre>
          <DialogFooter>
            <Button
              variant="outline"
              size="sm"
              onClick={() => detail && void copyText(JSON.stringify(detail, null, 2), "详情已复制")}
            >
              <Copy />
              复制
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      </CardContent>
    </SettingsGroup>
  );
}

// ---------------------------------------------------------------------------
// 登录态快照（原「登录态快照」页）
// ---------------------------------------------------------------------------

/**
 * 登录态快照分组。
 *
 * 快照保存的是 Trae 客户端 userData 下 9 类核心文件，用于把账号的完整登录态
 * 在「当前」与「目标」之间搬运。原先是一个独立导航项，现下沉到设置页。
 *
 * **恢复是高级操作**：它会直接覆盖客户端当前登录态，因此这里保留行内风险标注
 * 与二次确认 Dialog；常规路径应使用「账号管理」页的「切换」（切换会先自动保存快照）。
 */
function ProfilesSection({
  data,
  loading,
  busy,
  variant,
  onReload,
  onRun,
}: {
  data: TraeProfilesOverview | null;
  loading: boolean;
  busy: string | null;
  /** 当前产品线：快照按变体分家，备份/恢复/删除都必须带上它。 */
  variant: TraeVariantId;
  onReload: () => void;
  onRun: (key: string, label: string, action: () => Promise<unknown>) => Promise<void>;
}) {
  const [backupSlot, setBackupSlot] = useState("");
  const [pendingDelete, setPendingDelete] = useState<TraeProfileInfo | null>(null);
  const [pendingRestore, setPendingRestore] = useState<TraeProfileInfo | null>(null);

  const profiles = data?.profiles ?? [];

  return (
    <SettingsGroup id="trae-settings-profiles" title="登录态快照">
      <CardContent className="space-y-0 p-0">
      <SettingsRow className="flex-wrap gap-y-1">
        <div className="flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
          <span className="text-muted-foreground">
            当前账号
            <span className="ml-2 font-medium text-foreground">{data?.currentAccount ?? "未知"}</span>
          </span>
          <span className="flex items-center gap-2">
            客户端
            <span
              className={cn(
                "inline-flex items-center gap-1.5 font-medium",
                data?.clientRunning ? "text-emerald-600" : "text-muted-foreground",
              )}
            >
              <span
                className={cn(
                  "size-2 rounded-full",
                  data?.clientRunning ? "bg-emerald-500" : "bg-muted-foreground/50",
                )}
              />
              {data?.clientRunning ? "运行中" : "未运行"}
            </span>
          </span>
          <span className="text-muted-foreground">
            快照 <span className="font-medium text-foreground">{profiles.length}</span> 份
          </span>
        </div>
        <Button variant="ghost" size="sm" onClick={onReload} disabled={loading}>
          <RefreshCw className={cn(loading && "animate-spin")} />
          刷新
        </Button>
      </SettingsRow>

      {data?.dataDir && (
        <div className="border-b border-border/50 px-4 py-2.5 text-xs text-muted-foreground sm:px-5">
          客户端数据目录：<code className="font-mono">{data.dataDir}</code>
        </div>
      )}

      <SettingsFieldRow
        label="备份当前登录态"
        description={`把客户端此刻的登录态存到指定槽位；每个账号一份快照，精确复制 ${data?.coreEntryCount ?? 9} 类登录态核心文件。槽位名建议用账号 UID；last 是切换流程自动使用的兜底槽位。`}
        htmlFor="trae-backup-slot"
      >
        <div className="flex w-full flex-wrap items-center justify-end gap-2 sm:w-auto">
          <Input
            id="trae-backup-slot"
            className="h-8 sm:w-56"
            value={backupSlot}
            onChange={(event) => setBackupSlot(event.target.value)}
            placeholder="槽位名（账号 UID）"
          />
          <DemoAction>
            <Button
              size="sm"
              variant="outline"
              disabled={!backupSlot.trim() || busy === "backup"}
              onClick={() =>
                void onRun("backup", "备份", () => api.traeBackupProfile(backupSlot.trim(), variant)).then(() =>
                  setBackupSlot(""),
                )
              }
            >
              {busy === "backup" ? <Loader2 className="animate-spin" /> : <HardDriveUpload />}
              备份
            </Button>
          </DemoAction>
        </div>
      </SettingsFieldRow>

      {loading && !data ? (
        <div className="space-y-2 p-4 sm:p-5">
          <Skeleton className="h-14 w-full" />
          <Skeleton className="h-14 w-full" />
        </div>
      ) : profiles.length === 0 ? (
        <p className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">
          还没有快照。切换账号时会自动为当前账号保存一份。
        </p>
      ) : (
        <div className="divide-y divide-border/50">
          {profiles.map((profile) => (
            <div key={profile.slot} className="flex flex-wrap items-center justify-between gap-3 px-4 py-3 sm:px-5">
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="truncate text-[13px] font-medium">{profile.slot}</span>
                  {profile.slot === "last" && <Badge variant="outline">兜底槽位</Badge>}
                  {profile.slot === data?.currentAccount && <Badge variant="secondary">当前账号</Badge>}
                </div>
                <div className="mt-1 flex flex-wrap items-center gap-x-5 gap-y-1 text-xs text-muted-foreground">
                  <span>{profile.sizeText}</span>
                  <span>{profile.fileCount} 个文件</span>
                  <span>更新于 {profile.lastModified}</span>
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-1.5">
                <DemoAction>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy === `restore-${profile.slot}`}
                    onClick={() => setPendingRestore(profile)}
                  >
                    {busy === `restore-${profile.slot}` ? (
                      <Loader2 className="animate-spin" />
                    ) : (
                      <HardDriveDownload />
                    )}
                    恢复
                  </Button>
                </DemoAction>
                <DemoAction>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-destructive hover:text-destructive"
                    disabled={busy === `delete-${profile.slot}`}
                    onClick={() => setPendingDelete(profile)}
                  >
                    <Trash2 />
                    删除
                  </Button>
                </DemoAction>
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="border-t border-border/60 px-4 py-3 text-xs leading-5 text-muted-foreground sm:px-5">
        快照保存在 BuddySwitch 自己的数据目录下，与 Trae 客户端目录分离，因此删除快照不会影响正在使用的登录态。
      </div>

      {/* 恢复确认：明确告知会覆盖当前登录态 */}
      <Dialog open={pendingRestore !== null} onOpenChange={(open) => !open && setPendingRestore(null)}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>恢复到该快照</DialogTitle>
            <DialogDescription>
              将用「{pendingRestore?.slot}」的快照覆盖 Trae 客户端当前的登录态。
              <strong className="font-medium text-foreground">当前登录态不会被自动保存</strong>
              ，如需保留请先在上面备份。建议先关闭 Trae 客户端。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setPendingRestore(null)}>
              取消
            </Button>
            <Button
              onClick={() => {
                const target = pendingRestore;
                if (!target) return;
                setPendingRestore(null);
                void onRun(`restore-${target.slot}`, "恢复", () => api.traeRestoreProfile(target.slot, variant));
              }}
            >
              确认恢复
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 删除确认 */}
      <Dialog open={pendingDelete !== null} onOpenChange={(open) => !open && setPendingDelete(null)}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>删除快照</DialogTitle>
            <DialogDescription>
              将删除槽位「{pendingDelete?.slot}」的 {pendingDelete?.fileCount ?? 0} 个文件（
              {pendingDelete?.sizeText}）。账号记录不受影响，但该槽位将无法再恢复。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setPendingDelete(null)}>
              取消
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                const target = pendingDelete;
                if (!target) return;
                setPendingDelete(null);
                void onRun(`delete-${target.slot}`, "删除", () => api.traeDeleteProfile(target.slot, variant));
              }}
            >
              删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      </CardContent>
    </SettingsGroup>
  );
}

// ---------------------------------------------------------------------------
// 页面
// ---------------------------------------------------------------------------

/**
 * 设置页除「设置项」之外的只读快照。
 *
 * 设置项本身不在这里：它与账号页的「跳过今日已签到」开关是同一份
 * `get_trae_settings`，因此单独一把键 `trae:settings`、两个页面共用。
 */
interface SettingsPageSnapshot {
  env: TraeEnvStatus;
  capabilities: TraeCapabilities;
  profiles: TraeProfilesOverview;
}

/**
 * 「设置」页（Trae 分区）。
 *
 * 板块顺序与 WorkBuddy 设置页同构（外观 → 客户端 → 端口与网络 → 策略 → 高级）。
 * 原先独立的「登录态快照」与「系统日志」两个导航项作为高级板块下沉到这里，
 * 能力完整保留——侧栏收敛的是入口数量，不是功能范围。
 */
export default function TraeSettingsPage() {
  // 快照按产品线分家（`paths::profiles_dir_for`），故这里也必须带上变体。
  const [variant] = useTraeVariant();
  const [saving, setSaving] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [resetOpen, setResetOpen] = useState(false);
  const [resetBusy, setResetBusy] = useState(false);
  const [resetReport, setResetReport] = useState<TraeDeviceResetReport | null>(null);

  /**
   * 设置页除「设置项」之外的只读快照。
   *
   * 设置项**不在这里**：它与账号页的「跳过今日已签到」开关是同一份
   * `get_trae_settings`，因此单独一把键 `trae:settings`、两个页面共用 ——
   * 一处改完，另一处下次挂载拿到的就是新值。
   */
  const loadSnapshot = useCallback(async (): Promise<SettingsPageSnapshot> => {
    const [envData, capabilityData, profileData] = await Promise.all([
      api.getTraeEnv(),
      api.getTraeCapabilities(),
      api.getTraeProfiles(variant),
    ]);
    return { env: envData, capabilities: capabilityData, profiles: profileData };
  }, [variant]);

  const {
    data: pageSnapshot,
    loading,
    error,
    refresh: load,
  } = useCachedResource<SettingsPageSnapshot>(`trae:settings-page:${variant}`, loadSnapshot);

  /** 设置项本身：与账号页共用 `trae:settings`，写入一律走 `patchSettings`（乐观更新）。 */
  const { data: settings, patch: patchSettings } = useCachedResource<TraeSettings>(
    "trae:settings",
    api.getTraeSettings,
  );

  const env = pageSnapshot?.env ?? null;
  const capabilities = pageSnapshot?.capabilities ?? null;
  const profiles = pageSnapshot?.profiles ?? null;

  async function patch(next: Partial<TraeSettings>) {
    if (!settings) return;
    setSaving(true);
    // 乐观更新：设置项是单值开关/输入，本地先落再回读，避免每次拖动开关都等一轮往返。
    patchSettings((prev) => ({ ...prev, ...next }));
    try {
      const saved = await api.saveTraeSettings(next);
      patchSettings(() => saved);
    } catch (e) {
      toast.error("保存失败", { description: api.asError(e) });
      await load();
    } finally {
      setSaving(false);
    }
  }

  /** 快照动作：加忙标记、提示、随后刷新设置页的全部聚合数据。 */
  async function runProfileAction(key: string, label: string, action: () => Promise<unknown>) {
    setBusy(key);
    try {
      await action();
      toast.success(`${label}完成`);
      await load();
    } catch (e) {
      toast.error(`${label}失败`, { description: api.asError(e) });
    } finally {
      setBusy(null);
    }
  }

  async function runResetDevice() {
    setResetBusy(true);
    try {
      const report = await api.traeResetDevice(variant);
      setResetReport(report);
      toast.success("设备标识重置完成", { description: `${report.resetCount} / ${report.totalLayers} 项生效` });
      await load();
    } catch (e) {
      toast.error("重置失败", { description: api.asError(e) });
    } finally {
      setResetBusy(false);
    }
  }

  if (loading && !settings) {
    return (
      <div className="mx-auto min-w-0 w-full max-w-3xl space-y-3 px-4 py-6 sm:px-6 sm:py-8">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-40 w-full" />
        <Skeleton className="h-40 w-full" />
      </div>
    );
  }

  // 探测到的是哪条产品线 / 是否为自动探测。产品名由路径推断，推断不出就省略标签。
  const productLabel = traeProductLabel(env);
  const autoDetected = isAutoDetected(env);

  return (
    <div className="mx-auto min-w-0 w-full max-w-3xl px-4 py-6 sm:px-6 sm:py-8">
      <header className="mb-10 sm:mb-12">
        <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-3">
          <div className="min-w-0">
            <h1 className="text-2xl font-semibold tracking-tight">设置</h1>
            <p className="mt-2 text-sm leading-6 text-muted-foreground">
              客户端定位、本地端口、签到策略，以及登录态快照与运行日志。Trae 的配置与 WorkBuddy 分开保存，互不影响。
            </p>
          </div>
          {/* 产品线切换器：设置项本身按产品线分家，切到这里改的就是对应那条线的配置。 */}
          <TraeVariantSwitch className="shrink-0" />
        </div>
      </header>

      {error && (
        <Alert variant="destructive" className="mb-6">
          <AlertTriangle />
          <AlertTitle>无法读取 Trae 配置</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      <div className="min-w-0 space-y-12">
        {/* ---- 客户端 ---- */}
        <SettingsGroup id="trae-settings-client" title="Trae 客户端">
          <CardContent className="space-y-0 p-0">
          <SettingsRow className="flex-wrap gap-y-2">
            <div className="flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
              <span className="flex items-center gap-2">
                安装
                <span className={cn("font-medium", env?.installed ? "text-emerald-600" : "text-muted-foreground")}>
                  {env?.installed ? `已检测到${env.version ? ` v${env.version}` : ""}` : "未检测到"}
                </span>
                {env?.installed && productLabel && <Badge variant="secondary">{productLabel}</Badge>}
              </span>
              <span className="flex items-center gap-2">
                运行
                <span className={cn("font-medium", env?.running ? "text-emerald-600" : "text-muted-foreground")}>
                  {env?.running ? "运行中" : "未运行"}
                </span>
              </span>
              <Badge variant="outline">平台 {capabilities?.platform ?? env?.platform ?? "未知"}</Badge>
            </div>
          </SettingsRow>

          {/* 探测结果：同时装了两个 Trae 时，用户靠这块确认切换器管的是哪一个 */}
          {autoDetected && (env?.path || env?.dataDir) && (
            <div className="border-b border-border/50 px-4 py-3 sm:px-5">
              <div className="space-y-1.5 rounded-md border bg-muted/40 px-3 py-2.5">
                <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1 text-xs">
                  <span className="shrink-0 text-muted-foreground">自动探测到的安装</span>
                  <code className="break-all font-mono text-foreground">{env?.path ?? "—"}</code>
                </div>
                <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1 text-xs">
                  <span className="shrink-0 text-muted-foreground">自动探测到的数据目录</span>
                  <code className="break-all font-mono text-foreground">{env?.dataDir ?? "—"}</code>
                  {env?.dataDir && !env.dataDirExists && <Badge variant="destructive">目录不存在</Badge>}
                </div>
                <p className="text-xs leading-5 text-muted-foreground">
                  同机装有多个 Trae 产品线（<code className="font-mono">TRAE SOLO CN</code>、
                  <code className="font-mono">Trae CN</code> 等）时，按最近活跃的那个自动选定。
                </p>
              </div>
            </div>
          )}

          <SettingsFieldRow
            label="客户端可执行文件路径"
            description="留空即自动探测，会覆盖系统盘与各非系统盘的常见安装目录，无需手动填写。仅在装在非常规位置时指定。"
            htmlFor="trae-path"
          >
            <div className="flex w-full flex-wrap items-center justify-end gap-2 sm:w-auto">
              <Input
                id="trae-path"
                className="h-8 sm:w-80"
                value={settings?.traePath ?? ""}
                onChange={(event) => patchSettings((prev) => ({ ...prev, traePath: event.target.value }))}
                placeholder={env?.path ?? "留空则自动探测常见安装位置"}
              />
              <Button
                size="sm"
                variant="outline"
                disabled={saving}
                onClick={() => void patch({ traePath: settings?.traePath?.trim() ? settings.traePath.trim() : null })}
              >
                {saving ? <Loader2 className="animate-spin" /> : <Save />}
                保存
              </Button>
            </div>
          </SettingsFieldRow>
          </CardContent>
        </SettingsGroup>

        {/* ---- 端口与网络 ---- */}
        <SettingsGroup id="trae-settings-network" title="端口与网络">
          <CardContent className="space-y-0 p-0">
          <SettingsFieldRow
            label="本地代理端口"
            description="本地 MITM 代理的监听端口；登录态捕获依赖它。"
            htmlFor="trae-proxy-port"
          >
            <Input
              id="trae-proxy-port"
              className="h-8 w-full sm:w-40"
              inputMode="numeric"
              value={settings?.proxyPort ?? ""}
              onChange={(event) =>
                patchSettings((prev) => ({ ...prev, proxyPort: Number(event.target.value) || 0 }))
              }
              onBlur={() => void patch({ proxyPort: settings?.proxyPort ?? 8899 })}
            />
          </SettingsFieldRow>
          <SettingsFieldRow
            label="API 网关端口"
            description="默认 7864，与 WorkBuddy 网关（57891）错开——两者都占用 /v1/chat/completions。"
            htmlFor="trae-api-port"
          >
            <Input
              id="trae-api-port"
              className="h-8 w-full sm:w-40"
              inputMode="numeric"
              value={settings?.apiPort ?? ""}
              onChange={(event) =>
                patchSettings((prev) => ({ ...prev, apiPort: Number(event.target.value) || 0 }))
              }
              onBlur={() => void patch({ apiPort: settings?.apiPort ?? 7864 })}
            />
          </SettingsFieldRow>
          <SettingsFieldRow
            label="代理解密域名"
            description="逗号分隔。只有这些域名会被本地代理解密以捕获登录态；其余流量走加密隧道直连。"
            htmlFor="trae-domains"
          >
            <Input
              id="trae-domains"
              className="h-8 w-full font-mono text-xs sm:w-80"
              value={settings?.proxyDomains ?? ""}
              onChange={(event) =>
                patchSettings((prev) => ({ ...prev, proxyDomains: event.target.value }))
              }
              onBlur={() => void patch({ proxyDomains: settings?.proxyDomains ?? "" })}
            />
          </SettingsFieldRow>
          </CardContent>
        </SettingsGroup>

        {/* ---- 签到策略 ---- */}
        <SettingsGroup id="trae-settings-checkin" title="签到策略">
          <CardContent className="space-y-0 p-0">
          <SettingsFieldRow
            label="跳过今日已签到账号"
            description="避免对同一账号重复请求 claim，减少配额浪费与风控暴露。"
          >
            <Switch
              checked={settings?.checkinSkipChecked ?? true}
              disabled={saving}
              onCheckedChange={(checked) => void patch({ checkinSkipChecked: checked })}
              aria-label="跳过今日已签到账号"
            />
          </SettingsFieldRow>
          <SettingsFieldRow
            label="跳过 JWT 已过期账号"
            description="失效凭据必然返回 401；跳过可避免把账号打入永久冷却。"
          >
            <Switch
              checked={settings?.checkinSkipExpired ?? true}
              disabled={saving}
              onCheckedChange={(checked) => void patch({ checkinSkipExpired: checked })}
              aria-label="跳过 JWT 已过期账号"
            />
          </SettingsFieldRow>
          <SettingsFieldRow
            label="网络失败重试次数"
            description="仅对网络层异常重试；业务失败（如额度限制）不会重试。"
            htmlFor="trae-retry"
          >
            <Input
              id="trae-retry"
              className="h-8 w-full sm:w-24"
              inputMode="numeric"
              value={settings?.retry ?? 1}
              onChange={(event) =>
                patchSettings((prev) => ({ ...prev, retry: Number(event.target.value) || 0 }))
              }
              onBlur={() => void patch({ retry: settings?.retry ?? 1 })}
            />
          </SettingsFieldRow>
          <SettingsFieldRow
            label="日志保留天数"
            description="超期的日志行会在启动时裁剪；无日期前缀的外部输出一律保留。"
            htmlFor="trae-log-retention"
          >
            <Input
              id="trae-log-retention"
              className="h-8 w-full sm:w-24"
              inputMode="numeric"
              value={settings?.logRetentionDays ?? 30}
              onChange={(event) =>
                patchSettings((prev) => ({ ...prev, logRetentionDays: Number(event.target.value) || 0 }))
              }
              onBlur={() => void patch({ logRetentionDays: settings?.logRetentionDays ?? 30 })}
            />
          </SettingsFieldRow>
          </CardContent>
        </SettingsGroup>

        {/* ---- 登录态快照 ---- */}
        <ProfilesSection
          data={profiles}
          loading={loading}
          busy={busy}
          variant={variant}
          onReload={() => void load()}
          onRun={runProfileAction}
        />

        {/* ---- 设备标识 ---- */}
        <SettingsGroup id="trae-settings-device" title="设备标识">
          <CardContent className="space-y-0 p-0">
          <SettingsFieldRow
            label="重置设备标识"
            description="依次处理 6 层客户端设备标识：machineid 文件、storage.json 遥测与设备 ID、aha/TinyStorage、注册表 MachineGuid（仅 Windows）、WebView 追踪数据。操作前请确保已保存登录态快照。"
            operational
          >
            <Button
              variant="outline"
              size="sm"
              disabled={resetBusy || !capabilities?.clientDetection}
              onClick={() => setResetOpen(true)}
            >
              {resetBusy ? <Loader2 className="animate-spin" /> : <RotateCcw />}
              重置
            </Button>
          </SettingsFieldRow>
          {resetReport && (
            <div className="space-y-1.5 px-4 py-3 sm:px-5">
              <div className="text-xs text-muted-foreground">
                上次结果：{resetReport.resetCount} / {resetReport.totalLayers} 项生效
              </div>
              {resetReport.steps.map((step) => (
                <div key={step.layer} className="flex items-center gap-2 text-xs">
                  <Badge variant={step.status === "ok" ? "success" : "outline"}>
                    {step.status === "ok" ? "已处理" : step.status === "unsupported" ? "平台不支持" : "已跳过"}
                  </Badge>
                  <span className="text-muted-foreground">
                    {step.label}
                    {step.reason ? ` — ${step.reason}` : ""}
                  </span>
                </div>
              ))}
            </div>
          )}
          </CardContent>
        </SettingsGroup>

        {/* ---- 平台能力 ---- */}
        <SettingsGroup id="trae-settings-capabilities" title="平台能力">
          <CardContent className="space-y-0 p-0">
          <div className="px-4 py-3.5 sm:px-5">
            <div className="flex flex-wrap gap-2">
              <CapabilityBadge label="客户端检测" supported={capabilities?.clientDetection ?? false} />
              <CapabilityBadge label="进程控制" supported={capabilities?.processControl ?? false} />
              <CapabilityBadge label="定时任务" supported={capabilities?.scheduledTask ?? false} />
              <CapabilityBadge label="MachineGuid 重置" supported={capabilities?.machineGuidReset ?? false} />
            </div>
            <p className="mt-3 flex items-start gap-1.5 text-xs leading-5 text-muted-foreground">
              <Info className="mt-0.5 size-3.5 shrink-0" />
              标记为「不支持」的能力会明确给出「在哪支持 + 为什么这里不行」，而不是伪装成功。
            </p>
          </div>
          {capabilities && capabilities.unsupported.length > 0 && (
            <div className="space-y-1.5 border-t border-border/50 px-4 py-3 sm:px-5">
              {capabilities.unsupported.map((item) => (
                <div key={item.capability} className="text-xs text-muted-foreground">
                  <span className="font-medium text-foreground">{item.label}</span>
                  （仅 {item.supportedOn}）：{item.reason}
                </div>
              ))}
            </div>
          )}
          </CardContent>
        </SettingsGroup>

        {/* ---- 运行日志 ---- */}
        <RuntimeLogsSection />

        {/* ---- 网关请求日志 ---- */}
        <GatewayLogsSection />

        {/* ---- 关于 ---- */}
        <SettingsGroup id="trae-settings-about" title="关于">
          <CardContent className="space-y-0 p-0">
          <SettingsRow className="flex-wrap gap-y-2">
            <div className="min-w-0">
              <div className="text-[13px]">Trae 支持项目</div>
              <p className="mt-0.5 text-xs leading-4 text-muted-foreground/75">
                账号切换、签到与设备标识能力参考开源实现，并按本工具的架构重写为原生 Rust。
              </p>
            </div>
            <Button variant="ghost" size="sm" asChild>
              <a href="https://www.trae.cn" target="_blank" rel="noreferrer">
                <ExternalLink />
                trae.cn
              </a>
            </Button>
          </SettingsRow>
          </CardContent>
        </SettingsGroup>
      </div>

      {/* 重置确认 */}
      <Dialog open={resetOpen} onOpenChange={setResetOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>重置设备标识</DialogTitle>
            <DialogDescription>
              将改写 Trae 客户端的机器码与设备标识文件。建议先关闭 Trae，并确认已保存登录态快照。
              在非 Windows 平台上，注册表 MachineGuid 一层会明确标记为「平台不支持」，不会伪装成功。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setResetOpen(false)}>
              取消
            </Button>
            <Button
              onClick={() => {
                setResetOpen(false);
                void runResetDevice();
              }}
            >
              确认重置
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
