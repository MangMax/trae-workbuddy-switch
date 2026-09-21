import { useCallback, useEffect, useMemo, useState } from "react";
import { AlertTriangle, Copy, FolderOpen, Loader2, Power, RefreshCw, ShieldAlert } from "lucide-react";
import { toast } from "sonner";

import { DemoAction } from "@/components/demo-action";
import { TraeVariantSwitch } from "@/components/trae-variant-switch";
import { TraeAccountPoolCard } from "@/components/gateway/trae-account-pool-card";
import { TraeApiKeyTable } from "@/components/gateway/trae-api-key-table";
import { TraeIntegrationGuide } from "@/components/gateway/trae-integration-guide";
import { TraeModelList } from "@/components/gateway/trae-model-list";
import { TraeRequestLog } from "@/components/gateway/trae-request-log";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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
import * as api from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import {
  DEFAULT_TRAE_GATEWAY_CONFIG,
  normalizeTraeGatewayConfig,
  normalizeTraeGatewayLogs,
  normalizeTraeGatewayStatus,
  toTraeGatewayConfigRaw,
} from "@/lib/trae-gateway";
import type { TraeGatewayConfig, TraeGatewayLogEntry, TraeGatewayModel, TraeGatewayStatus } from "@/lib/trae-types";
import { cn } from "@/lib/utils";
import { useTraeVariant } from "@/lib/use-trae-variant";

const LOOPBACK = "127.0.0.1";
const LAN = "0.0.0.0";

/**
 * 「API 服务」页（Trae 分区）。
 *
 * 与 WorkBuddy 的「API 服务」页是**两套东西**：Trae 网关的上游是
 * `trae-api-cn.mchost.guru`，凭据是 `Cloud-IDE-JWT`，响应是私有 SOLO SSE，
 * 因此它的独立监听端口（默认 7864）与 WorkBuddy 网关（57891）必须分开——
 * 两个网关都占 `/v1/chat/completions`，合并到同一端口会直接冲突。
 *
 * ## 本轮结构：容器 + 五个 Trae 专用组件
 *
 * Key 管理已由后端升级为**多 Key 库（含归属产品线）**（`list/create/revoke/delete_trae_api_key`），
 * 因此单 Key 卡与其「重新生成」入口（R6）一并删除，改由 `TraeApiKeyTable` 承担
 * （「归属产品线」列取代 WorkBuddy 的「归属版本」列）。接入指引 / 账号池 / 模型清单 /
 * 请求日志分别下沉为 `TraeIntegrationGuide` / `TraeAccountPoolCard` / `TraeModelList` /
 * `TraeRequestLog`——**页面不再保留任何内联副本**。
 *
 * **骨架与 WorkBuddy 刻意同构**（容器宽度、页头字号、卡片节奏、工具条排布），
 * 但下列控件因 Trae 无对应能力而**不出现**：`Region` Tabs / 「按版本」双条目 /
 * `AccountStrategyCard`（Trae 无 `accountStrategy`）——分别以单条 Base URL、
 * 账号池卡替代。模型区**不加刷新按钮**（模型名是客户端常量，刷新永不改变结果）。
 */
export default function TraeApiServicePage() {
  /** 当前产品线：决定读哪个账号池（`trae_gateway_status(variant)`）与归属列默认值。 */
  const [variant] = useTraeVariant();
  const [config, setConfig] = useState<TraeGatewayConfig>(DEFAULT_TRAE_GATEWAY_CONFIG);
  const [status, setStatus] = useState<TraeGatewayStatus | null>(null);
  const [models, setModels] = useState<TraeGatewayModel[]>([]);
  const [logs, setLogs] = useState<TraeGatewayLogEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [portDraft, setPortDraft] = useState(String(DEFAULT_TRAE_GATEWAY_CONFIG.port));
  const [riskOpen, setRiskOpen] = useState(false);
  const [openingDir, setOpeningDir] = useState(false);

  const loadAll = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [configRaw, statusRaw, modelsRaw, logsRaw] = await Promise.all([
        api.getTraeGatewayConfig(),
        api.getTraeGatewayStatus(variant),
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
  }, [variant]);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  async function persist(next: Partial<TraeGatewayConfig>) {
    const merged = { ...config, ...next };
    setSaving(true);
    try {
      await api.saveTraeGatewayConfig(toTraeGatewayConfigRaw(merged));
      setConfig(merged);
      // 保存会启动/重启监听，状态与日志都要重读（仍按当前产品线）。
      const statusRaw = await api.getTraeGatewayStatus(variant);
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

  /**
   * 打开 Trae 数据目录。
   *
   * 非 Windows 上后端返回**结构化 `Unsupported`**（不是假成功）：此时如实把
   * 「当前平台不支持」提示给用户，而不是弹一句「已打开」却什么都没发生。
   */
  async function onOpenDataDir() {
    setOpeningDir(true);
    try {
      const result = (await api.openTraeDataDir(variant)) as {
        ok?: boolean;
        path?: string;
        capability?: string;
        reason?: string;
      };
      if (result?.capability) {
        toast.message("当前平台不支持", { description: result.reason ?? "该能力仅在 Windows 提供。" });
        return;
      }
      toast.success("已打开 Trae 数据目录", { description: result?.path });
    } catch (e) {
      toast.error("打开数据目录失败", { description: api.asError(e) });
    } finally {
      setOpeningDir(false);
    }
  }

  const running = status?.running ?? false;
  const baseUrl = useMemo(() => {
    const host = config.bindAddr === LAN ? LOOPBACK : config.bindAddr;
    return `http://${host}:${config.port}/v1`;
  }, [config.bindAddr, config.port]);

  const keyPrefix = status?.apiKeyPrefix || "";

  if (loading && !status) {
    return (
      <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
        <Skeleton className="h-9 w-48" />
        <Skeleton className="mt-4 h-32 w-full" />
        <Skeleton className="mt-6 h-64 w-full" />
      </div>
    );
  }

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
          <DemoAction>
            <Button
              variant="outline"
              size="sm"
              className="ml-auto"
              disabled={openingDir}
              onClick={() => void onOpenDataDir()}
            >
              {openingDir ? <Loader2 className="animate-spin" /> : <FolderOpen />}
              打开数据目录
            </Button>
          </DemoAction>
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
            <span>最近 Key 前缀</span>
            <code className="font-mono">{keyPrefix ? `${keyPrefix}…` : "尚无可用 Key"}</code>
          </div>
        </div>
      </Card>

      {/* ---- API Key（多 Key + 归属产品线） ---- */}
      {/* 替代 WorkBuddy 的单 Key 卡：这里支持多把 Key，每把绑定一条产品线。 */}
      <TraeApiKeyTable
        className="mb-6"
        defaultVariant={variant}
        onChanged={() => void loadAll()}
      />

      {/* ---- 账号池 ---- */}
      <TraeAccountPoolCard status={status} className="mb-6" />

      {/* ---- 模型清单（无刷新按钮：模型名是客户端常量） ---- */}
      <TraeModelList models={models} defaultModel={config.defaultModel} className="mb-6" />

      {/* ---- 接入指引（单条 Base URL，无 region Tabs） ---- */}
      <TraeIntegrationGuide
        baseUrl={baseUrl}
        model={config.defaultModel}
        keyPrefix={keyPrefix}
        className="mb-6"
      />

      {/* ---- 请求日志 ---- */}
      <TraeRequestLog logs={logs} clearing={clearing} onClear={() => void onClearLogs()} />

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
    </div>
  );
}

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
