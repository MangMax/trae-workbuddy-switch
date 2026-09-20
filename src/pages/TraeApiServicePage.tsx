import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  Check,
  Copy,
  Cpu,
  Eraser,
  KeyRound,
  Loader2,
  Power,
  RefreshCw,
  ShieldAlert,
  Users,
} from "lucide-react";
import { toast } from "sonner";

import { DemoAction } from "@/components/demo-action";
import { TraeVariantSwitch } from "@/components/trae-variant-switch";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
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
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import * as api from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import {
  DEFAULT_TRAE_GATEWAY_CONFIG,
  normalizeTraeGatewayConfig,
  normalizeTraeGatewayLogs,
  normalizeTraeGatewayStatus,
  toTraeGatewayConfigRaw,
  TRAE_POOL_STATUS_LABELS,
} from "@/lib/trae-gateway";
import type {
  TraeGatewayConfig,
  TraeGatewayLogEntry,
  TraeGatewayModel,
  TraeGatewayStatus,
} from "@/lib/trae-types";
import { cn } from "@/lib/utils";

const LOOPBACK = "127.0.0.1";
const LAN = "0.0.0.0";

/** 接入指引里的客户端（Trae 侧无 region 概念，故比 WorkBuddy 少一个维度）。 */
const TOOLS = [
  { key: "cursor", label: "Cursor" },
  { key: "cline", label: "Cline" },
  { key: "continue", label: "Continue" },
  { key: "cherry", label: "Cherry Studio" },
] as const;

type ToolKey = (typeof TOOLS)[number]["key"];

function snippetFor(tool: ToolKey, baseUrl: string, key: string, model: string): string {
  switch (tool) {
    case "cursor":
      return [
        "Cursor → Settings → Models → OpenAI API Key",
        "",
        `Override OpenAI Base URL: ${baseUrl}`,
        `API Key:  ${key}`,
        `Model:    ${model}`,
      ].join("\n");
    case "cline":
      return JSON.stringify(
        {
          apiProvider: "openai",
          openAiBaseUrl: baseUrl,
          openAiApiKey: key,
          openAiModelId: model,
        },
        null,
        2,
      );
    case "continue":
      return [
        "models:",
        "  - name: Trae",
        "    provider: openai",
        `    model: ${model}`,
        `    apiBase: ${baseUrl}`,
        `    apiKey: ${key}`,
      ].join("\n");
    case "cherry":
      return [`API 地址: ${baseUrl}`, `API 密钥: ${key}`, `模型: ${model}`].join("\n");
    default:
      return "";
  }
}

function formatClock(ts: number): string {
  const date = new Date(ts);
  if (Number.isNaN(date.getTime())) return "—";
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}:${String(date.getSeconds()).padStart(2, "0")}`;
}

function formatTokens(value: number | null | undefined): string {
  if (value == null) return "—";
  return new Intl.NumberFormat("zh-CN").format(value);
}

function formatCredits(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return "—";
  return value.toLocaleString("zh-CN", { maximumFractionDigits: 2 });
}

function statusTone(status: number): string {
  if (status >= 200 && status < 300) return "text-emerald-600 dark:text-emerald-400";
  if (status === 429) return "text-amber-600 dark:text-amber-400";
  return "text-destructive";
}

/**
 * 「API 服务」页（Trae 分区）。
 *
 * 与 WorkBuddy 的「API 服务」页是**两套东西**：Trae 网关的上游是
 * `trae-api-cn.mchost.guru`，凭据是 `Cloud-IDE-JWT`，响应是私有 SOLO SSE，
 * 因此它的独立监听端口（默认 7864）与 WorkBuddy 网关（57891）必须分开——
 * 两个网关都占 `/v1/chat/completions`，合并到同一端口会直接冲突。
 *
 * 本页只做四件事：配置、状态、模型清单、请求日志。Key 只有一把，
 * 存在 `TraeSettings::apiKey`，因此没有 WorkBuddy 那套「多 Key + region 绑定」。
 *
 * **骨架与 WorkBuddy 刻意同构**（容器宽度、页头字号、卡片节奏、工具条排布），
 * 但下列控件因 Trae 无对应能力而**不出现**：
 * 「接入地址（按版本）」（Trae 无 region）、`ApiKeyTable` 多 Key 表与吊销、
 * `AccountStrategyCard` 账号策略（Trae 只有 `maxRotate` + 池状态，无后端策略文件）。
 * 分别以单条 Base URL、单 Key 卡、账号池卡替代。
 */
export default function TraeApiServicePage() {
  const [config, setConfig] = useState<TraeGatewayConfig>(DEFAULT_TRAE_GATEWAY_CONFIG);
  const [status, setStatus] = useState<TraeStatus>(null);
  const [models, setModels] = useState<TraeGatewayModel[]>([]);
  const [logs, setLogs] = useState<TraeGatewayLogEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [portDraft, setPortDraft] = useState(String(DEFAULT_TRAE_GATEWAY_CONFIG.port));
  const [riskOpen, setRiskOpen] = useState(false);
  const [keyOpen, setKeyOpen] = useState(false);
  const [newKey, setNewKey] = useState<string | null>(null);
  const [tool, setTool] = useState<ToolKey>("cursor");
  const [copied, setCopied] = useState(false);

  const loadAll = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [configRaw, statusRaw, modelsRaw, logsRaw] = await Promise.all([
        api.getTraeGatewayConfig(),
        api.getTraeGatewayStatus(),
        api.getTraeGatewayModels(),
        api.getTraeGatewayLogs(),
      ]);
      const nextConfig = normalizeTraeGatewayConfig(configRaw);
      setConfig(nextConfig);
      setPortDraft(String(nextConfig.port));
      setStatus(normalizeTraeGatewayStatus(statusRaw));
      setModels(readModels(modelsRaw));
      setLogs(normalizeTraeGatewayLogs(logsRaw));
    } catch (e) {
      setError(api.asError(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  async function persist(next: Partial<TraeGatewayConfig>) {
    const merged = { ...config, ...next };
    setSaving(true);
    try {
      await api.saveTraeGatewayConfig(toTraeGatewayConfigRaw(merged));
      setConfig(merged);
      // 保存会启动/重启监听，状态与日志都要重读。
      const statusRaw = await api.getTraeGatewayStatus();
      setStatus(normalizeTraeGatewayStatus(statusRaw));
      toast.success(merged.enabled ? "网关已保存并启动" : "网关已保存（未启用监听）");
    } catch (e) {
      toast.error("保存失败", { description: api.asError(e) });
    } finally {
      setSaving(false);
    }
  }

  function onBindAddrChange(next: string) {
    if (next === config.bindAddr) return;
    if (next === LOOPBACK) {
      void persist({ bindAddr: LOOPBACK, allowNonLoopback: false });
      return;
    }
    setRiskOpen(true);
  }

  function confirmLan() {
    setRiskOpen(false);
    void persist({ bindAddr: LAN, allowNonLoopback: true });
  }

  function commitPort() {
    const parsed = Number.parseInt(portDraft, 10);
    if (!Number.isFinite(parsed) || parsed < 1 || parsed > 65535) {
      toast.error("端口无效", { description: "请输入 1–65535 之间的整数" });
      setPortDraft(String(config.port));
      return;
    }
    if (parsed === config.port) return;
    void persist({ port: parsed });
  }

  async function onRegenerateKey() {
    try {
      const result = await api.regenerateTraeApiKey();
      setNewKey(result.key ?? null);
      setKeyOpen(true);
      const statusRaw = await api.getTraeGatewayStatus();
      setStatus(normalizeTraeGatewayStatus(statusRaw));
    } catch (e) {
      toast.error("重新生成失败", { description: api.asError(e) });
    }
  }

  async function onClearLogs() {
    setClearing(true);
    try {
      await api.clearTraeGatewayLogs();
      setLogs([]);
      toast.success("日志已清空");
    } catch (e) {
      toast.error("清空失败", { description: api.asError(e) });
    } finally {
      setClearing(false);
    }
  }

  const running = status?.running ?? false;
  const baseUrl = useMemo(() => {
    const host = config.bindAddr === LAN ? LOOPBACK : config.bindAddr;
    return `http://${host}:${config.port}/v1`;
  }, [config.bindAddr, config.port]);

  const keyDisplay = newKey ?? (status?.apiKeyPrefix ? `${status.apiKeyPrefix}` : "sk-trae-…");
  const snippet = snippetFor(tool, baseUrl, keyDisplay, config.defaultModel);

  async function onCopySnippet() {
    await copyText(snippet, "代码已复制");
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  }

  if (loading && !status) {
    return (
      <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
        <Skeleton className="h-9 w-48" />
        <Skeleton className="mt-4 h-32 w-full" />
        <Skeleton className="mt-6 h-64 w-full" />
      </div>
    );
  }

  const pool = status?.pool;

  return (
    <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
      <header className="mb-6 flex flex-wrap items-start justify-between gap-x-6 gap-y-3">
        <div className="min-w-0">
          <h1 className="text-[28px] font-semibold tracking-tight">API 服务</h1>
          <p className="mt-2 text-sm leading-6 text-muted-foreground">
            把 Trae 的模型额度以 OpenAI 兼容接口提供给本机工具。
          </p>
        </div>
        {/* 产品线切换器：Trae 分区的每个页面都可切，位置固定在页头右侧。 */}
        <TraeVariantSwitch className="shrink-0" />
      </header>

      {error && (
        <Alert variant="destructive" className="mb-5">
          <AlertTriangle />
          <AlertTitle>无法读取网关状态</AlertTitle>
          <AlertDescription className="flex flex-col gap-3">
            <span>{error}</span>
            <div>
              <Button variant="outline" size="sm" onClick={() => void loadAll()}>
                <RefreshCw />
                重试
              </Button>
            </div>
          </AlertDescription>
        </Alert>
      )}

      {/* ---- 网关 ---- */}
      <Card className="mb-6 gap-0 py-0">
        <div className="flex items-center justify-between gap-3 border-b border-border/60 px-5 py-4">
          <div className="min-w-0">
            <div className="flex items-center gap-2 text-sm font-medium">
              <Power className="size-4 text-muted-foreground" />
              启用 API 网关
            </div>
            <p className="mt-1 text-xs text-muted-foreground">
              开启后本机 AI 工具可通过下方地址调用 Trae 模型（上游 {status?.upstream || "trae-api-cn.mchost.guru"}）。
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {saving && <Loader2 className="size-3.5 animate-spin text-muted-foreground" />}
            <DemoAction>
              <Switch
                checked={config.enabled}
                disabled={saving}
                onCheckedChange={(checked) => void persist({ enabled: checked })}
                aria-label="启用 API 网关"
              />
            </DemoAction>
            <DemoAction>
              <Button variant="outline" size="sm" onClick={() => void loadAll()}>
                <RefreshCw />
                刷新
              </Button>
            </DemoAction>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-x-6 gap-y-3 px-5 py-4">
          <div className="flex items-center gap-2">
            <span className="text-xs text-muted-foreground">监听地址</span>
            <Select value={config.bindAddr} onValueChange={onBindAddrChange} disabled={saving}>
              <SelectTrigger size="sm" className="w-52" aria-label="监听地址">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={LOOPBACK}>127.0.0.1（仅本机）</SelectItem>
                <SelectItem value={LAN}>0.0.0.0（局域网）</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="flex items-center gap-2">
            <span className="text-xs text-muted-foreground">监听端口</span>
            <DemoAction>
              <Input
                className="h-8 w-28"
                inputMode="numeric"
                value={portDraft}
                onChange={(event) => setPortDraft(event.target.value.replace(/[^\d]/g, ""))}
                onBlur={commitPort}
                onKeyDown={(event) => {
                  if (event.key === "Enter") commitPort();
                }}
                aria-label="监听端口"
              />
            </DemoAction>
          </div>
          <span className="flex items-center gap-1.5 text-xs">
            状态：
            <span
              className={cn(
                "inline-flex items-center gap-1.5 font-medium",
                running ? "text-emerald-600 dark:text-emerald-400" : "text-muted-foreground",
              )}
            >
              <span className={cn("size-2 rounded-full", running ? "bg-emerald-500" : "bg-muted-foreground/50")} />
              {running ? "运行中" : "已停止"}
            </span>
          </span>
        </div>

        <div className="flex flex-wrap items-center gap-2 border-t border-border/60 px-5 py-3 text-xs text-muted-foreground">
          默认端口 7864，与 WorkBuddy 网关（57891）错开——两者都占用 <code className="font-mono">/v1/chat/completions</code>。
          {config.bindAddr === LAN && "局域网模式下，同网段任何设备拿到 Key 都能消耗你的 Trae 积分。"}
        </div>

        {status?.lastError && (
          <div className="border-t border-border/60 px-5 py-3">
            <p className="text-xs text-muted-foreground">最近一次错误</p>
            <p className="mt-1 break-words text-xs text-destructive">{status.lastError}</p>
          </div>
        )}
      </Card>

      {/* ---- 接入地址 ---- */}
      {/* WorkBuddy 此处是「按版本」逐 region 一行；Trae 无 region，故只有一条。 */}
      <Card className="mb-6 gap-0 py-0">
        <div className="border-b border-border/60 px-5 py-3">
          <span className="text-sm font-semibold">接入地址</span>
        </div>
        <div className="px-5 py-4">
          <div className="flex flex-wrap items-center gap-3">
            <span className="text-xs text-muted-foreground">Base URL</span>
            <code className="min-w-0 break-all rounded-md border border-border bg-muted/40 px-2 py-1 font-mono text-xs">
              {baseUrl}
            </code>
            <Button variant="ghost" size="sm" onClick={() => void copyText(baseUrl, "Base URL 已复制")}>
              <Copy />
              复制
            </Button>
          </div>
          <div className="mt-1.5 flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
            <span>API Key</span>
            <code className="font-mono">{keyDisplay}</code>
          </div>
        </div>
      </Card>

      {/* ---- API Key（单 Key） ---- */}
      {/* 替代 WorkBuddy 的 `ApiKeyTable`：Trae 的 Key 存在 `TraeSettings::apiKey`，
          只有一把且与 region 无关，故没有「多 Key + 吊销 + 版本绑定」三件套。 */}
      <Card className="mb-6 gap-0 py-0">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
          <div className="flex items-center gap-2 text-sm font-semibold">
            <KeyRound className="size-4 stroke-[1.75]" />
            API Key
          </div>
          <DemoAction>
            <Button variant="outline" size="sm" onClick={() => void onRegenerateKey()}>
              <RefreshCw />
              重新生成
            </Button>
          </DemoAction>
        </div>
        <div className="px-5 py-3">
          <div className="flex flex-wrap items-center gap-3">
            <code className="min-w-0 flex-1 truncate rounded-md border border-border bg-muted/40 px-3 py-2 font-mono text-sm">
              {keyDisplay}
            </code>
            {newKey && (
              <Button variant="outline" size="sm" onClick={() => void copyText(newKey, "API Key 已复制")}>
                <Copy />
                复制明文
              </Button>
            )}
          </div>
          <p className="mt-2 text-xs leading-5 text-muted-foreground">
            明文只在生成时展示一次；重新生成会立即作废旧 Key，正在使用它的客户端需要同步更新。
          </p>
        </div>
      </Card>

      {/* ---- 账号池 ---- */}
      {/* 替代 WorkBuddy 的 `AccountStrategyCard`：Trae 无 `accountStrategy` 后端契约，
          只有 `pool` 五态计数与逐账号状态，外加后端给出的 `diagnose` 排查串。 */}
      <Card className="mb-6 gap-0 py-0">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
          <div className="flex items-center gap-2 text-sm font-semibold">
            <Users className="size-4 stroke-[1.75]" />
            账号池
          </div>
          <span className="text-xs text-muted-foreground">
            累计请求 {formatTokens(status?.totalRequests ?? 0)}
          </span>
        </div>

        <div className="grid grid-cols-2 gap-0 border-b border-border/60 sm:grid-cols-5">
          {[
            { label: "可路由", value: pool?.available ?? 0, tone: "ok" as const },
            { label: "冷却中", value: pool?.cooling ?? 0, tone: "warn" as const },
            { label: "会话失效", value: pool?.disabled ?? 0, tone: "danger" as const },
            { label: "积分过期", value: pool?.expired ?? 0, tone: "warn" as const },
            { label: "零积分", value: pool?.zeroCredits ?? 0, tone: "muted" as const },
          ].map((item, index) => (
            <div
              key={item.label}
              className={cn(
                "flex flex-col items-center justify-center px-3 py-4 text-center",
                index > 0 && "border-l border-border/60",
              )}
            >
              <span className="text-xs text-muted-foreground">{item.label}</span>
              <span
                className={cn(
                  "mt-2 text-2xl font-semibold tabular-nums tracking-[-0.02em]",
                  item.tone === "ok"
                    ? "text-emerald-600 dark:text-emerald-400"
                    : item.tone === "warn"
                      ? "text-amber-600 dark:text-amber-400"
                      : item.tone === "danger"
                        ? "text-destructive"
                        : "text-muted-foreground",
                )}
              >
                {item.value}
              </span>
            </div>
          ))}
        </div>

        {(status?.accounts.length ?? 0) === 0 ? (
          <p className="px-5 py-6 text-center text-sm text-muted-foreground">
            账号池为空。请先在「账号管理」中添加 Trae 账号。
          </p>
        ) : (
          <div className="divide-y divide-border/60">
            {status?.accounts.map((account) => {
              const meta = TRAE_POOL_STATUS_LABELS[account.status];
              return (
                <div key={account.uid} className="flex flex-wrap items-center gap-3 px-5 py-3">
                  <span className="min-w-0 flex-1 truncate text-sm font-medium">{account.name}</span>
                  <span className="text-xs text-muted-foreground tabular-nums">
                    {formatCredits(account.credits)} 积分
                  </span>
                  <Badge
                    variant={meta.tone === "danger" ? "destructive" : "secondary"}
                    className={cn(
                      "shrink-0",
                      meta.tone === "ok" && "text-emerald-600 dark:text-emerald-400",
                      meta.tone === "warn" && "text-amber-600 dark:text-amber-400",
                    )}
                  >
                    {meta.label}
                  </Badge>
                  {account.cooldownReason && (
                    <span className="w-full truncate text-xs text-muted-foreground sm:w-auto">
                      {account.cooldownReason}
                    </span>
                  )}
                </div>
              );
            })}
          </div>
        )}

        {(status?.diagnose.length ?? 0) > 0 && (
          <div className="border-t border-border/60 px-5 py-3">
            <p className="text-xs text-muted-foreground">
              请求报「没有可用账号」时，按下面的原因逐条排查：
            </p>
            <ul className="mt-2 space-y-1">
              {status?.diagnose.map((line) => (
                <li key={line} className="break-all font-mono text-xs text-muted-foreground">
                  {line}
                </li>
              ))}
            </ul>
          </div>
        )}
      </Card>

      {/* ---- 模型清单 ---- */}
      <Card className="mb-6 gap-0 py-0">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
          <div className="flex items-center gap-2 text-sm font-semibold">
            <Cpu className="size-4 stroke-[1.75]" />
            模型清单
          </div>
          <span className="text-xs text-muted-foreground">
            共 {models.length} 个 · 默认 {config.defaultModel}
          </span>
        </div>
        <div className="px-5 py-4">
          {models.length === 0 ? (
            <p className="py-4 text-sm text-muted-foreground">暂无模型数据。</p>
          ) : (
            <div className="flex flex-wrap gap-2">
              {models.map((model) => (
                <code
                  key={model.id}
                  className={cn(
                    "rounded-md border px-2 py-1 font-mono text-xs",
                    model.id === config.defaultModel
                      ? "border-foreground/30 bg-foreground/[0.06] text-foreground"
                      : "border-border text-muted-foreground",
                  )}
                >
                  {model.id}
                </code>
              ))}
            </div>
          )}
        </div>
      </Card>

      {/* ---- 接入指引 ---- */}
      <Card className="mb-6 gap-0 py-0">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
          <span className="text-sm font-semibold">接入指引</span>
          <Button variant="outline" size="sm" onClick={() => void onCopySnippet()}>
            {copied ? <Check /> : <Copy />}
            复制代码
          </Button>
        </div>
        <Tabs value={tool} onValueChange={(value) => setTool(value as ToolKey)}>
          <div className="border-b border-border/60 px-5 py-2">
            <TabsList className="flex-wrap">
              {TOOLS.map((item) => (
                <TabsTrigger key={item.key} value={item.key}>
                  {item.label}
                </TabsTrigger>
              ))}
            </TabsList>
          </div>
          <div className="px-5 py-4">
            <pre className="min-w-0 overflow-x-auto rounded-lg border border-border bg-muted/40 p-4 font-mono text-xs leading-6">
              {snippet}
            </pre>
            <p className="mt-2 text-xs text-muted-foreground">
              Trae 上游只支持流式；请求 `stream: false` 时由本网关在本地聚合后一次性返回，首字节延迟较长。
            </p>
          </div>
        </Tabs>
      </Card>

      {/* ---- 请求日志 ---- */}
      <Card className="gap-0 py-0">
        <div className="flex items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
          <span className="text-sm font-semibold">请求日志（最近 {logs.length} 条）</span>
          <DemoAction>
            <Button
              variant="ghost"
              size="sm"
              disabled={clearing || logs.length === 0}
              onClick={() => void onClearLogs()}
            >
              {clearing ? <Loader2 className="animate-spin" /> : <Eraser />}
              清空
            </Button>
          </DemoAction>
        </div>
        {logs.length === 0 ? (
          <p className="px-5 py-6 text-center text-sm text-muted-foreground">
            暂无请求。网关启动后，客户端发来的每次调用都会记在这里（默认只记元数据，不记正文）。
          </p>
        ) : (
          <div className="max-h-80 overflow-y-auto">
            <table className="w-full text-sm">
              <thead className="sticky top-0 bg-card text-xs text-muted-foreground">
                <tr className="border-b border-border/60">
                  <th className="px-5 py-2 text-left font-medium">时间</th>
                  <th className="px-3 py-2 text-left font-medium">账号</th>
                  <th className="px-3 py-2 text-left font-medium">模型</th>
                  <th className="px-3 py-2 text-right font-medium">状态</th>
                  <th className="px-3 py-2 text-right font-medium">耗时</th>
                  <th className="px-5 py-2 text-right font-medium">Token</th>
                </tr>
              </thead>
              <tbody>
                {[...logs].reverse().map((entry, index) => (
                  <tr key={`${entry.ts}-${index}`} className="border-b border-border/40 last:border-0">
                    <td className="whitespace-nowrap px-5 py-2 font-mono text-xs">
                      {formatClock(entry.ts)}
                    </td>
                    <td className="max-w-[8rem] truncate px-3 py-2 text-xs text-muted-foreground">
                      {entry.account ? entry.account.slice(-8) : "—"}
                    </td>
                    <td className="max-w-[10rem] truncate px-3 py-2 text-xs text-muted-foreground">
                      {entry.model ?? "—"}
                    </td>
                    <td className={cn("px-3 py-2 text-right font-mono text-xs", statusTone(entry.status))}>
                      {entry.status}
                    </td>
                    <td className="px-3 py-2 text-right font-mono text-xs text-muted-foreground">
                      {entry.latencyMs}ms
                    </td>
                    <td className="px-5 py-2 text-right font-mono text-xs text-muted-foreground">
                      {formatTokens((entry.promptTokens ?? 0) + (entry.completionTokens ?? 0))}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>

      {/* ---- 局域网风险确认 ---- */}
      <Dialog open={riskOpen} onOpenChange={setRiskOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <ShieldAlert className="size-4 text-amber-500" />
              允许局域网访问？
            </DialogTitle>
            <DialogDescription>
              监听 0.0.0.0 后，同网段（含公共 Wi-Fi）的任何设备只要拿到 API Key，就能消耗你的 Trae
              积分。请只在可信网络下开启。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setRiskOpen(false)}>
              取消
            </Button>
            <Button onClick={confirmLan}>
              <Power />
              我已确认，开启
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* ---- 新 Key 一次性展示 ---- */}
      <Dialog
        open={keyOpen}
        onOpenChange={(open) => {
          setKeyOpen(open);
          if (!open) setNewKey(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>新的 API Key</DialogTitle>
            <DialogDescription>
              这是唯一一次展示明文。关闭后只能看到脱敏前缀，需要再次获取请重新生成。
            </DialogDescription>
          </DialogHeader>
          <code className="break-all rounded-md bg-muted/50 px-3 py-2 font-mono text-sm">
            {newKey ?? ""}
          </code>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => newKey && void copyText(newKey, "API Key 已复制")}
            >
              <Copy />
              复制
            </Button>
            <Button onClick={() => setKeyOpen(false)}>我已保存</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** 状态在「未启动」时后端仍会返回完整结构，但 `null` 是防御性兜底。 */
type TraeStatus = TraeGatewayStatus | null;

/** `get_trae_gateway_models` 返回 OpenAI `/v1/models` 形状，这里只取 `data`。 */
function readModels(raw: unknown): TraeGatewayModel[] {
  const record = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const list = Array.isArray(record.data) ? record.data : [];
  return list
    .map((item) => {
      if (!item || typeof item !== "object") return null;
      const entry = item as Record<string, unknown>;
      const id = typeof entry.id === "string" ? entry.id : null;
      if (!id) return null;
      return {
        id,
        object: typeof entry.object === "string" ? entry.object : "model",
        created: typeof entry.created === "number" ? entry.created : 0,
        owned_by: typeof entry.owned_by === "string" ? entry.owned_by : "trae",
      };
    })
    .filter((item): item is TraeGatewayModel => item !== null);
}
