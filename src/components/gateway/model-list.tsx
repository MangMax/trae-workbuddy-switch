import { useState } from "react";
import { Loader2, RefreshCw } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { DemoAction } from "@/components/demo-action";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import * as api from "@/lib/api";
import { REGIONS, regionDescriptor } from "@/lib/region";
import { cn } from "@/lib/utils";
import type { CatalogSource, Region } from "@/lib/types";
import { useGatewayStore } from "@/stores/gateway";

const SOURCE_LABEL: Record<CatalogSource, { text: string; variant: "success" | "secondary" | "warning" }> = {
  live: { text: "实时", variant: "success" },
  cached: { text: "已保存", variant: "secondary" },
  builtin: { text: "内置", variant: "warning" },
};

function formatTime(ts: number | null): string {
  if (!ts) return "—";
  const date = new Date(ts);
  if (Number.isNaN(date.getTime())) return "—";
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

/** 模型列表：按 region 切换，展示来源徽标（实时 / 已保存 / 内置）与刷新按钮（P0-4 / P0-12 / P1-7）。 */
export function ModelList({ className }: { className?: string }) {
  const [region, setRegion] = useState<Region>("cn");
  const [refreshing, setRefreshing] = useState(false);
  const snapshot = useGatewayStore((s) => s.models[region]);
  const refreshModels = useGatewayStore((s) => s.refreshModels);

  async function onRefresh() {
    setRefreshing(true);
    try {
      await refreshModels(region);
      toast.success("模型列表已刷新");
    } catch (e) {
      toast.error("刷新失败", { description: api.asError(e) });
    } finally {
      setRefreshing(false);
    }
  }

  const source = snapshot ? SOURCE_LABEL[snapshot.source] : null;

  return (
    <Card className={cn("gap-0 py-0", className)}>
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
        <div className="flex flex-wrap items-center gap-3">
          <span className="text-sm font-semibold">模型列表</span>
          <Tabs value={region} onValueChange={(value) => setRegion(value as Region)}>
            <TabsList>
              {REGIONS.map((r) => (
                <TabsTrigger key={r} value={r}>
                  {regionDescriptor(r).versionLabel}
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
        </div>
        <DemoAction>
          <Button variant="ghost" size="sm" onClick={() => void onRefresh()} disabled={refreshing}>
            {refreshing ? <Loader2 className="animate-spin" /> : <RefreshCw />}
            刷新
          </Button>
        </DemoAction>
      </div>

      <div className="px-5 py-4">
        {source && (
          <div className="mb-3 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
            <span className="flex items-center gap-1.5">
              来源
              <Badge variant={source.variant} className="rounded-md">
                {source.text}
              </Badge>
            </span>
            <span>更新于 {formatTime(snapshot?.fetched_at ?? null)}</span>
            <span>共 {snapshot?.models.length ?? 0} 个模型</span>
          </div>
        )}
        {snapshot?.note && <p className="mb-3 text-xs text-amber-600">{snapshot.note}</p>}
        {!snapshot || snapshot.models.length === 0 ? (
          <p className="py-4 text-sm text-muted-foreground">暂无模型数据。</p>
        ) : (
          <div className="flex flex-wrap gap-2">
            {snapshot.models.map((model) => (
              <span
                key={model.id}
                className="inline-flex items-center gap-1.5 rounded-lg border border-border bg-muted/40 px-2.5 py-1 text-xs"
                title={`${model.name} · 上下文 ${model.context_window} · 最大输出 ${model.max_tokens}${model.credits ? ` · ${model.credits}` : ""}`}
              >
                <span className="font-medium">{model.name}</span>
                {model.free && (
                  <Badge variant="success" className="rounded-md px-1.5 py-0 text-[10px]">
                    免费
                  </Badge>
                )}
                {model.badges.map((badge) => (
                  <Badge key={badge} variant="warning" className="rounded-md px-1.5 py-0 text-[10px]">
                    {badge}
                  </Badge>
                ))}
              </span>
            ))}
          </div>
        )}
      </div>
    </Card>
  );
}
