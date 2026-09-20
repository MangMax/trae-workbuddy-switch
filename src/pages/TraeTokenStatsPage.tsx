import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  ArrowDownToLine,
  ArrowUpFromLine,
  CircleAlert,
  Coins,
  Gauge,
  Info,
  Loader2,
  RefreshCw,
  Server,
  Timer,
  Users,
} from "lucide-react";
import { Bar, BarChart, CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";

import { DemoAction } from "@/components/demo-action";
import { TraeVariantSwitch } from "@/components/trae-variant-switch";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import * as api from "@/lib/api";
import type { TraeTokenStatistics } from "@/lib/trae-types";
import { cn } from "@/lib/utils";

/** 统计窗口选项。`0` 表示全部历史（后端把 `<= 0` 视为不限）。 */
const RANGES = [
  { value: "7", label: "近 7 天" },
  { value: "30", label: "近 30 天" },
  { value: "90", label: "近 90 天" },
  { value: "0", label: "全部" },
] as const;

const TREND_SERIES = [
  { key: "total", label: "总 Token", color: "var(--data-series-indigo)" },
  { key: "input", label: "输入", color: "var(--data-series-sky)" },
  { key: "output", label: "输出", color: "var(--data-series-emerald)" },
] as const;

const TREND_CONFIG: ChartConfig = Object.fromEntries(
  TREND_SERIES.map((series) => [series.key, { label: series.label, color: series.color }]),
);

const MODEL_CONFIG: ChartConfig = {
  total: { label: "总 Token", color: "var(--data-series-violet)" },
};

function formatTokens(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return "—";
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(2)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return String(value);
}

function formatExact(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return "—";
  return new Intl.NumberFormat("zh-CN").format(value);
}

function StatMetric({
  icon: Icon,
  label,
  value,
  hint,
  divided = false,
}: {
  icon: typeof Coins;
  label: string;
  value: string;
  hint?: string;
  divided?: boolean;
}) {
  return (
    <div
      className={cn(
        "flex min-w-0 flex-col items-center justify-center px-4 py-5 text-center sm:py-3",
        divided && "sm:border-l sm:border-border/60",
      )}
    >
      <div className="flex max-w-full items-center justify-center gap-2 text-[13px] font-medium leading-5 text-muted-foreground">
        <Icon className="size-4 shrink-0 stroke-[1.75]" aria-hidden="true" />
        <span className="truncate">{label}</span>
      </div>
      <div
        className="mt-3 max-w-full truncate text-[26px] font-semibold leading-8 tracking-[-0.025em] tabular-nums"
        style={{ fontFamily: '"Bricolage Grotesque Variable", "SF Pro Display", ui-sans-serif, sans-serif' }}
      >
        {value}
      </div>
      {hint && <div className="mt-1.5 max-w-full truncate text-xs text-muted-foreground">{hint}</div>}
    </div>
  );
}

/**
 * 「Token 统计」页（Trae 分区）。
 *
 * 与 WorkBuddy 的 Token 统计**数据源不同**：那边扫客户端落的会话文件，
 * 这边只有本机网关的请求日志。页面上必须把这个边界讲清楚，
 * 否则用户会以为「数字小 = 统计坏了」。
 */
export default function TraeTokenStatsPage() {
  const [stats, setStats] = useState<TraeTokenStatistics | null>(null);
  const [days, setDays] = useState<string>("30");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (range: string) => {
    setLoading(true);
    setError(null);
    try {
      const data = await api.getTraeTokenStatistics(Number.parseInt(range, 10));
      setStats(data);
    } catch (e) {
      setError(api.asError(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(days);
  }, [load, days]);

  const summary = stats?.summary;
  const daily = useMemo(
    () => (stats?.daily ?? []).filter((point) => (point.records ?? 0) > 0),
    [stats],
  );
  const models = stats?.models ?? [];
  const accounts = stats?.accounts ?? [];
  const topModels = models.slice(0, 8);

  if (loading && !stats) {
    return (
      <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
        <Skeleton className="h-9 w-48" />
        <Skeleton className="mt-4 h-32 w-full" />
        <Skeleton className="mt-6 h-64 w-full" />
      </div>
    );
  }

  if (error && !stats) {
    return (
      <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
        <header className="mb-6">
          <h1 className="text-[28px] font-semibold tracking-tight">Token 统计</h1>
          <p className="mt-2 text-sm leading-6 text-muted-foreground">
            汇总经过本机 Trae 网关的调用用量。
          </p>
        </header>
        <Alert variant="destructive">
          <AlertTriangle />
          <AlertTitle>无法读取 Token 统计</AlertTitle>
          <AlertDescription className="flex flex-col gap-3">
            <span>{error}</span>
            <div>
              <Button variant="outline" size="sm" onClick={() => void load(days)}>
                <RefreshCw />
                重试
              </Button>
            </div>
          </AlertDescription>
        </Alert>
      </div>
    );
  }

  const empty = (summary?.records ?? 0) === 0;

  return (
    <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
      <header className="mb-6 flex min-w-0 flex-wrap items-end justify-between gap-3">
        <div className="min-w-0">
          <h1 className="text-[28px] font-semibold tracking-tight">Token 统计</h1>
          <p className="mt-2 text-sm leading-6 text-muted-foreground">
            汇总经过本机 Trae 网关的调用用量。
          </p>
        </div>
        <div className="flex max-w-full flex-wrap items-center justify-end gap-2">
          {/* 产品线切换器：Trae 分区的每个页面都可切，位置固定在页头右侧动作区。 */}
          <TraeVariantSwitch />
          <DemoAction>
            <Button variant="outline" size="sm" disabled={loading} onClick={() => void load(days)}>
              {loading ? <Loader2 className="animate-spin" /> : <RefreshCw />}
              刷新
            </Button>
          </DemoAction>
        </div>
      </header>

      {/* 数据源边界：不写清楚，用户会把「数字小」当成 bug。 */}
      <Alert className="mb-6">
        <Info />
        <AlertTitle>数据来源</AlertTitle>
        <AlertDescription>
          <span className="break-all">
            {stats?.note ?? "只统计经过本机 Trae 网关的调用。"}
          </span>
          {stats?.logFile && (
            <span className="mt-1 block break-all font-mono text-xs text-muted-foreground">
              {stats.logFile}
            </span>
          )}
        </AlertDescription>
      </Alert>

      {empty ? (
        <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
          <div className="font-medium text-foreground">当前窗口内没有任何网关调用</div>
          <p className="mt-2 text-xs">
            到「API 服务」页启用网关，再把客户端的 Base URL 指过来，调用就会记在这里。
          </p>
        </div>
      ) : (
        <>
          {/* ---- 汇总 ---- */}
          <Card
            className="mb-6 min-w-0 gap-0 overflow-hidden rounded-2xl bg-card/70 py-0 shadow-none"
            aria-label="用量汇总"
          >
            <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
              <span className="text-[13px] font-medium">用量汇总</span>
              <Tabs
                className="min-w-0 shrink-0 gap-0"
                value={days}
                onValueChange={(value) => setDays(value as (typeof RANGES)[number]["value"])}
              >
                <TabsList
                  className="grid h-auto w-full grid-cols-2 sm:inline-flex sm:w-fit sm:flex-wrap"
                  aria-label="统计范围"
                >
                  {RANGES.map((range) => (
                    <TabsTrigger key={range.value} value={range.value} className="px-2">
                      {range.label}
                    </TabsTrigger>
                  ))}
                </TabsList>
              </Tabs>
            </div>
            <div className="grid min-w-0 grid-cols-1 divide-y divide-border/60 p-0 sm:grid-cols-4 sm:divide-y-0 sm:py-5">
              <StatMetric
                icon={Coins}
                label="总 Token"
                value={formatTokens(summary?.total)}
                hint={`输入 ${formatTokens(summary?.input)} · 输出 ${formatTokens(summary?.output)}`}
              />
              <StatMetric
                icon={ArrowDownToLine}
                label="输入"
                value={formatTokens(summary?.input)}
                divided
              />
              <StatMetric
                icon={ArrowUpFromLine}
                label="输出"
                value={formatTokens(summary?.output)}
                divided
              />
              <StatMetric
                icon={Server}
                label="请求数"
                value={formatExact(summary?.records)}
                hint={`其中失败 ${formatExact(summary?.errors)}`}
                divided
              />
            </div>
            <div className="grid min-w-0 grid-cols-1 divide-y divide-border/60 border-t border-border/60 p-0 sm:grid-cols-4 sm:divide-y-0 sm:py-5">
              <StatMetric
                icon={Timer}
                label="平均耗时"
                value={`${formatExact(summary?.avgLatencyMs)}ms`}
              />
              <StatMetric
                icon={Gauge}
                label="P95 耗时"
                value={`${formatExact(summary?.p95LatencyMs)}ms`}
                divided
              />
              <StatMetric
                icon={ArrowUpFromLine}
                label="流式请求"
                value={formatExact(summary?.streamRequests)}
                divided
              />
              <StatMetric
                icon={Users}
                label="活跃账号"
                value={formatExact(accounts.length)}
                divided
              />
            </div>
          </Card>

          {/* ---- 每日趋势 ---- */}
          {daily.length > 1 && (
            <Card className="mb-6 gap-0 py-0">
              <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
                <span className="text-[13px] font-medium">每日用量趋势</span>
              </div>
              <div className="px-3 py-4">
                <ChartContainer config={TREND_CONFIG} className="h-[240px] w-full">
                  <LineChart data={daily} margin={{ top: 8, right: 16, bottom: 8, left: 8 }}>
                    <CartesianGrid vertical={false} strokeDasharray="3 3" />
                    <XAxis
                      dataKey="key"
                      tickLine={false}
                      axisLine={false}
                      tickMargin={8}
                      tickFormatter={(value: string) => value.slice(5)}
                    />
                    <YAxis tickLine={false} axisLine={false} width={48} tickFormatter={formatTokens} />
                    <ChartTooltip content={<ChartTooltipContent indicator="line" />} />
                    {TREND_SERIES.map((series) => (
                      <Line
                        key={series.key}
                        type="monotone"
                        dataKey={series.key}
                        stroke={`var(--color-${series.key})`}
                        strokeWidth={2}
                        dot={false}
                      />
                    ))}
                  </LineChart>
                </ChartContainer>
              </div>
            </Card>
          )}

          {/* ---- 模型分布 ---- */}
          <Card className="mb-6 gap-0 py-0">
            <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
              <span className="text-[13px] font-medium">按模型</span>
            </div>
            {topModels.length === 0 ? (
              <p className="px-5 py-6 text-center text-sm text-muted-foreground">暂无模型数据</p>
            ) : (
              <>
                <div className="px-3 py-4">
                  <ChartContainer config={MODEL_CONFIG} className="h-[200px] w-full">
                    <BarChart data={topModels} margin={{ top: 8, right: 16, bottom: 8, left: 8 }}>
                      <CartesianGrid vertical={false} strokeDasharray="3 3" />
                      <XAxis
                        dataKey="key"
                        tickLine={false}
                        axisLine={false}
                        tickMargin={8}
                        interval={0}
                        angle={-18}
                        textAnchor="end"
                        height={56}
                      />
                      <YAxis tickLine={false} axisLine={false} width={48} tickFormatter={formatTokens} />
                      <ChartTooltip content={<ChartTooltipContent indicator="dot" />} />
                      <Bar dataKey="total" fill="var(--color-total)" radius={[4, 4, 0, 0]} />
                    </BarChart>
                  </ChartContainer>
                </div>
                <div className="divide-y divide-border/60 border-t border-border/60">
                  {models.map((model) => (
                    <div key={model.key} className="flex flex-wrap items-center gap-3 px-5 py-2.5">
                      <code className="min-w-0 flex-1 truncate font-mono text-xs">{model.key}</code>
                      <span className="text-xs text-muted-foreground tabular-nums">
                        {formatExact(model.records)} 次
                      </span>
                      {model.errors > 0 && (
                        <Badge variant="destructive" className="shrink-0">
                          失败 {model.errors}
                        </Badge>
                      )}
                      <span className="w-20 text-right text-xs font-medium tabular-nums">
                        {formatTokens(model.total)}
                      </span>
                    </div>
                  ))}
                </div>
              </>
            )}
          </Card>

          {/* ---- 按账号 ---- */}
          <Card className="mb-6 gap-0 py-0">
            <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
              <span className="text-[13px] font-medium">按账号</span>
            </div>
            {accounts.length === 0 ? (
              <p className="px-5 py-6 text-center text-sm text-muted-foreground">暂无账号数据</p>
            ) : (
              <div className="divide-y divide-border/60">
                {accounts.map((account) => (
                  <div key={account.key} className="flex flex-wrap items-center gap-3 px-5 py-3">
                    <span className="min-w-0 flex-1 truncate text-sm font-medium">{account.name}</span>
                    <span className="font-mono text-xs text-muted-foreground">{account.shortId}</span>
                    <span className="text-xs text-muted-foreground tabular-nums">
                      {formatExact(account.records)} 次
                    </span>
                    {account.errors > 0 && (
                      <Badge variant="destructive" className="shrink-0">
                        失败 {account.errors}
                      </Badge>
                    )}
                    <span className="w-20 text-right text-xs font-medium tabular-nums">
                      {formatTokens(account.total)}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </Card>

          {/* ---- 状态码分布 ---- */}
          {(stats?.statuses.length ?? 0) > 0 && (
            <Card className="mb-6 gap-0 py-0">
              <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
                <span className="text-[13px] font-medium">响应状态分布</span>
              </div>
              <div className="flex flex-wrap gap-2 px-5 py-4">
                {stats?.statuses.map((item) => {
                  const code = Number.parseInt(item.key, 10);
                  const ok = code >= 200 && code < 300;
                  return (
                    <span
                      key={item.key}
                      className={cn(
                        "rounded-md border px-2 py-1 font-mono text-xs tabular-nums",
                        ok
                          ? "border-border text-muted-foreground"
                          : "border-destructive/40 text-destructive",
                      )}
                    >
                      {item.key} × {item.records}
                    </span>
                  );
                })}
              </div>
            </Card>
          )}
        </>
      )}

      {stats && stats.parseErrors > 0 && (
        <p className="flex items-center gap-1.5 px-1 text-xs text-amber-600">
          <CircleAlert className="size-3.5" aria-hidden="true" />
          日志中有 {stats.parseErrors} 条记录无法解析（缺少时间戳），已跳过。
        </p>
      )}
    </div>
  );
}
