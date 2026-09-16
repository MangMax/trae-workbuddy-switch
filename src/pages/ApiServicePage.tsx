import { useEffect, useState } from "react";
import { AlertTriangle, Copy, Loader2, Power } from "lucide-react";
import { toast } from "sonner";

import { ApiKeyTable } from "@/components/gateway/api-key-table";
import { AccountStrategyCard } from "@/components/gateway/account-strategy-card";
import { IntegrationGuide } from "@/components/gateway/integration-guide";
import { ModelList } from "@/components/gateway/model-list";
import { RequestLog } from "@/components/gateway/request-log";
import { DemoAction } from "@/components/demo-action";
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
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import { resolveGatewayBaseUrl, resolveGatewayRunning } from "@/lib/gateway";
import { REGIONS, regionDescriptor } from "@/lib/region";
import { cn } from "@/lib/utils";
import type { ApiKeyRecord, GatewayConfig, Region } from "@/lib/types";
import { useGatewayStore } from "@/stores/gateway";

const LOOPBACK = "127.0.0.1";
const LAN = "0.0.0.0";

function representativeKey(keys: ApiKeyRecord[], region: Region): string {
  const active = keys.find((key) => key.region === region && !key.revoked);
  return active ? `${active.prefix}…` : "sk-wb-…（在下方 Key 列表创建）";
}

/** 「API 服务」页：网关开关、监听、Base URL、Key、模型、策略、接入指引、请求日志（P0-11）。 */
export default function ApiServicePage() {
  const config = useGatewayStore((s) => s.config);
  const status = useGatewayStore((s) => s.status);
  const keys = useGatewayStore((s) => s.keys);
  const loading = useGatewayStore((s) => s.loading);
  const error = useGatewayStore((s) => s.error);
  const loadAll = useGatewayStore((s) => s.loadAll);
  const saveConfig = useGatewayStore((s) => s.saveConfig);

  const [portDraft, setPortDraft] = useState(String(config.port));
  const [saving, setSaving] = useState(false);
  const [riskOpen, setRiskOpen] = useState(false);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  useEffect(() => {
    setPortDraft(String(config.port));
  }, [config.port]);

  async function persist(next: Partial<GatewayConfig>) {
    setSaving(true);
    try {
      await saveConfig({ ...config, ...next });
    } catch (e) {
      toast.error("保存失败", { description: api.asError(e) });
    } finally {
      setSaving(false);
    }
  }

  function onBindAddrChange(next: string) {
    if (next === config.bind_addr) return;
    if (next === LOOPBACK) {
      void persist({ bind_addr: LOOPBACK, allow_non_loopback: false });
      return;
    }
    // 非回环监听需要风险确认（Q2 / U5）。
    setRiskOpen(true);
  }

  function confirmLan() {
    setRiskOpen(false);
    void persist({ bind_addr: LAN, allow_non_loopback: true });
  }

  function commitPort() {
    const parsed = Number.parseInt(portDraft, 10);
    if (!Number.isFinite(parsed) || parsed < 1 || parsed > 65535) {
      toast.error("端口需为 1-65535 之间的整数");
      setPortDraft(String(config.port));
      return;
    }
    if (parsed === config.port) return;
    void persist({ port: parsed });
  }

  const baseUrl = resolveGatewayBaseUrl(status, config.bind_addr, config.port);
  const running = resolveGatewayRunning(status);

  return (
    <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
      <header className="mb-6">
        <h1 className="text-[28px] font-semibold tracking-tight">API 服务</h1>
        <p className="mt-2 text-sm leading-6 text-muted-foreground">
          把 WorkBuddy 的模型额度以 OpenAI / Anthropic 兼容接口提供给本机工具。
        </p>
      </header>

      {error && (
        <Alert variant="destructive" className="mb-4">
          <AlertTriangle />
          <AlertTitle>操作失败</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      {/* 网关 */}
      <Card className="mb-6 gap-0 py-0">
        <div className="flex items-center justify-between gap-3 border-b border-border/60 px-5 py-4">
          <div className="min-w-0">
            <div className="flex items-center gap-2 text-sm font-medium">
              <Power className="size-4 text-muted-foreground" />
              启用 API 网关
            </div>
            <p className="mt-1 text-xs text-muted-foreground">开启后本机 AI 工具可通过下方地址调用 WorkBuddy 模型。</p>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {saving && <Loader2 className="size-3.5 animate-spin text-muted-foreground" />}
            <DemoAction>
              <Switch
                checked={config.enabled}
                disabled={saving}
                onCheckedChange={(enabled) => void persist({ enabled })}
                aria-label="启用 API 网关"
              />
            </DemoAction>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-x-6 gap-y-3 px-5 py-4">
          <div className="flex items-center gap-2">
            <span className="text-xs text-muted-foreground">监听地址</span>
            <Select value={config.bind_addr} onValueChange={onBindAddrChange} disabled={saving}>
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
                onChange={(event) => setPortDraft(event.target.value)}
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
            <span className={cn("inline-flex items-center gap-1.5 font-medium", running ? "text-emerald-600" : "text-muted-foreground")}>
              <span className={cn("size-2 rounded-full", running ? "bg-emerald-500" : "bg-muted-foreground/50")} />
              {running ? "运行中" : "已停止"}
            </span>
          </span>
        </div>
      </Card>

      {/* 接入地址（按版本） */}
      <Card className="mb-6 gap-0 py-0">
        <div className="border-b border-border/60 px-5 py-3">
          <span className="text-sm font-semibold">接入地址（按版本）</span>
        </div>
        <div className="divide-y divide-border/60">
          {REGIONS.map((region) => (
            <div key={region} className="px-5 py-4">
              <div className="text-sm font-medium">{regionDescriptor(region).gatewayLabel}</div>
              <div className="mt-2 flex flex-wrap items-center gap-3">
                <span className="text-xs text-muted-foreground">Base URL</span>
                <code className="rounded-md border border-border bg-muted/40 px-2 py-1 font-mono text-xs">{baseUrl}</code>
                <Button variant="ghost" size="sm" onClick={() => void copyText(baseUrl, "Base URL 已复制")}>
                  <Copy />
                  复制
                </Button>
              </div>
              <div className="mt-1.5 flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
                <span>API Key</span>
                <code className="font-mono">{representativeKey(keys, region)}</code>
              </div>
            </div>
          ))}
        </div>
      </Card>

      <ApiKeyTable className="mb-6" />
      <AccountStrategyCard className="mb-6" />
      <ModelList className="mb-6" />
      <IntegrationGuide baseUrl={baseUrl} className="mb-6" />
      <RequestLog />

      {/* 非回环监听风险确认 */}
      <Dialog open={riskOpen} onOpenChange={setRiskOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>允许局域网访问</DialogTitle>
            <DialogDescription>
              局域网内任何设备都可消耗你的额度，请确认可信网络后再开启。网关仍要求携带有效 API Key。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setRiskOpen(false)}>
              取消
            </Button>
            <Button variant="destructive" onClick={confirmLan}>
              确认开启
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {loading && (
        <div className="mt-6 flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="animate-spin" />
          加载网关数据…
        </div>
      )}
    </div>
  );
}
