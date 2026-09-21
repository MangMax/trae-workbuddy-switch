import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
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
import { Bar, CartesianGrid, ComposedChart, Line, LineChart, XAxis, YAxis } from "recharts";

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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import * as api from "@/lib/api";
import type {
  TraeModelDailyPoint,
  TraeTokenScope,
  TraeTokenStatistics,
  TraeUnsupported,
} from "@/lib/trae-types";
import { cn } from "@/lib/utils";

/** 统计窗口选项。`0` 表示全部历史（后端把 `<= 0` 视为不限）。 */
const RANGES = [
  { value: "7", label: "近 7 天" },
  { value: "30", label: "近 30 天" },
  { value: "90", label: "近 90 天" },
  { value: "0", label: "全部" },
] as const;

/**
 * 变体范围条四档（**筛选维度**，独立于时间窗口）。
 *
 * `unlabeled` 是升级前的旧日志（没有 `variant` 键），不是第三种产品线；
 * 切勿把它并进 `TraeVariantId`。
 */
const SCOPE_OPTIONS: { value: TraeTokenScope; label: string; hint: string }[] = [
  { value: "unlabeled", label: "本机未标注", hint: "升级前未带产品线归属的旧日志" },
  { value: "cn", label: "国内版", hint: "归属国内区域的调用（两条程序位合计）" },
  { value: "global", label: "国际版", hint: "归属国际版区域的调用" },
  { value: "all", label: "全部", hint: "所有产品线的调用" },
];

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

/** 堆叠柱各模型配色的固定循环（用既有 CSS 变量，不新增裸色值）。 */
const MODEL_COLORS = [
  "var(--data-series-indigo)",
  "var(--data-series-sky)",
  "var(--data-series-emerald)",
  "var(--data-series-violet)",
  "var(--data-series-amber)",
] as const;

/** 堆叠柱最多展示几个模型（其余并入「其他」颜色不做，避免图例过长）。 */
const TOP_MODELS = 6;

/** 热力网格五档底纹。 */
const HEATMAP_LEVEL_CLASS = [
  "bg-muted/70",
  "bg-primary/20",
  "bg-primary/40",
  "bg-primary/65",
  "bg-primary",
] as const;

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

/** `YYYY-MM-DD` 本地日期键（与后端 `daily.key` 口径一致）。 */
function dateKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function formatHeatmapDate(date: Date): string {
  return date.toLocaleDateString("zh-CN", { month: "long", day: "numeric" });
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
 *
 * 本轮新增：变体范围条（`TraeTokenScope`，非第三种产品线）、整年热力网格（源 `daily`）、
 * 按模型 × 按天的堆叠柱 + 调用次数折线（源 `modelDaily`）、`unsupported` 置灰卡。
 */
export default function TraeTokenStatsPage() {
  const [stats, setStats] = useState<TraeTokenStatistics | null>(null);
  const [days, setDays] = useState<string>("30");
  const [scope, setScope] = useState<TraeTokenScope>("all");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (range: string, scopeValue: TraeTokenScope) => {
    setLoading(true);
    setError(null);
    try {
      const data = await api.getTraeTokenStatistics(Number.parseInt(range, 10), scopeValue);
      setStats(data);
    } catch (e) {
      setError(api.asError(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(days, scope);
  }, [load, days, scope]);

  const summary = stats?.summary;
  const daily = useMemo(
    () => (stats?.daily ?? []).filter((point) => (point.records ?? 0) > 0),
    [stats],
  );
  const models = stats?.models ?? [];
  const accounts = stats?.accounts ?? [];
  const topModels = models.slice(0, 8);
  const modelDaily = stats?.modelDaily ?? [];
  const unsupported = stats?.unsupported ?? [];
  const counts = stats?.variantCounts;

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
              <Button variant="outline" size="sm" onClick={() => void load(days, scope)}>
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
            <Button variant="outline" size="sm" disabled={loading} onClick={() => void load(days, scope)}>
              {loading ? <Loader2 className="animate-spin" /> : <RefreshCw />}
              刷新
            </Button>
          </DemoAction>
        </div>
      </header>

      {/* 数据源边界：不写清楚，用户会把「数字小」当成 bug。**必须保留。** */}
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

      {/* ---- 版本范围条：四档筛选（国内版 / 国际版 / 未标注 / 全部），
              计数来自 variantCounts（只受时间窗口影响） ---- */}
      <Card className="mb-6 gap-0 py-0">
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
          <span className="text-[13px] font-medium">版本范围</span>
          <span className="text-xs text-muted-foreground">仅筛选下方统计口径，不改变时间窗口</span>
        </div>
        <div className="px-5 py-3">
          <Tabs
            className="min-w-0"
            value={scope}
            onValueChange={(value) => setScope(value as TraeTokenScope)}
          >
            <TabsList
              className="grid h-auto w-full grid-cols-2 sm:inline-flex sm:w-fit sm:flex-wrap"
              aria-label="版本范围"
            >
              {SCOPE_OPTIONS.map((option) => (
                <TabsTrigger key={option.value} value={option.value} className="gap-1.5 px-3">
                  {option.label}
                  <span className="rounded-full bg-muted px-1.5 text-[11px] tabular-nums text-muted-foreground">
                    {counts ? counts[option.value] : "—"}
                  </span>
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
        </div>
      </Card>

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

          {/* ---- 整年热力网格（源 `daily`，空日由前端补齐） ---- */}
          <TokenHeatGrid daily={daily} />

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

          {/* ---- 按模型 × 按天：堆叠柱（Token）+ 折线（调用次数） ---- */}
          <ModelDailyChart points={modelDaily} />

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
                    <ComposedChart data={topModels} margin={{ top: 8, right: 16, bottom: 8, left: 8 }}>
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
                    </ComposedChart>
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

      {/* ---- 平台做不到的维度（置灰卡；形状来自 handlers::unsupported_note） ---- */}
      {unsupported.length > 0 && <UnsupportedSection items={unsupported} />}

      {stats && stats.parseErrors > 0 && (
        <p className="flex items-center gap-1.5 px-1 text-xs text-amber-600">
          <CircleAlert className="size-3.5" aria-hidden="true" />
          日志中有 {stats.parseErrors} 条记录无法解析（缺少时间戳），已跳过。
        </p>
      )}
    </div>
  );
}

/**
 * 整年热力网格（53 周 × 7 天）。
 *
 * 骨架参 `TokenStatsPage.tsx:813-879`；数据源＝本页的 `daily`（**同源**），
 * 空日由前端补 0，因此「整年」是画布语义，只有窗口内的日期有值。
 */
function TokenHeatGrid({ daily }: { daily: TraeTokenStatistics["daily"] }) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const valueByDate = useMemo(() => new Map(daily.map((point) => [point.key ?? "", point.total])), [daily]);
  const recordByDate = useMemo(
    () => new Map(daily.map((point) => [point.key ?? "", point.records])),
    [daily],
  );

  const today = new Date();
  today.setHours(12, 0, 0, 0);
  const todayKey = dateKey(today);
  const start = new Date(today);
  start.setDate(start.getDate() - start.getDay() - 52 * 7);

  const weeks = Array.from({ length: 53 }, (_, weekIndex) =>
    Array.from({ length: 7 }, (_, dayIndex) => {
      const date = new Date(start);
      date.setDate(start.getDate() + weekIndex * 7 + dayIndex);
      const key = dateKey(date);
      return {
        date,
        key,
        value: valueByDate.get(key) ?? 0,
        records: recordByDate.get(key) ?? 0,
        future: key > todayKey,
      };
    }),
  );
  const max = Math.max(1, ...weeks.flatMap((week) => week.filter((day) => !day.future).map((day) => day.value)));
  const monthLabels = weeks.map((week, weekIndex) => {
    const firstOfMonth = week.find((day) => day.date.getDate() === 1);
    let labelDate: Date | null = null;
    if (firstOfMonth && firstOfMonth.key <= todayKey) {
      labelDate = firstOfMonth.date;
    } else if (weekIndex === 0) {
      labelDate = week[0].date;
    }
    if (!labelDate || dateKey(labelDate) > todayKey) return null;
    return labelDate.toLocaleDateString("zh-CN", { month: "short" });
  });
  const activeDays = weeks.flat().filter((day) => !day.future && day.value > 0).length;

  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller) return;
    scroller.scrollLeft = scroller.scrollWidth - scroller.clientWidth;
  }, [daily]);

  return (
    <Card className="mb-6 min-w-0 gap-0 rounded-xl py-0 shadow-none">
      <div className="flex items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
        <span className="text-[13px] font-medium">Token 活动（最近一年）</span>
        <span className="shrink-0 text-xs text-muted-foreground">{activeDays} 个活跃日</span>
      </div>
      <div className="min-w-0 px-4 pt-4 pb-5 sm:px-5">
        <div ref={scrollerRef} className="overflow-x-auto pb-1">
          <div
            className="min-w-[760px]"
            role="img"
            aria-label={`最近一年每日 Token 活动热力图，共 ${activeDays} 个活跃日`}
          >
            <div
              className="grid gap-1"
              style={{ gridTemplateColumns: "repeat(53, minmax(10px, 1fr))" }}
              aria-hidden="true"
            >
              {weeks.flatMap((week, weekIndex) =>
                week.map((day, dayIndex) => {
                  const level = day.value ? Math.max(1, Math.ceil(Math.sqrt(day.value / max) * 4)) : 0;
                  const cell = (
                    <span
                      key={day.key}
                      className={`aspect-square min-w-0 rounded-[3px] ${
                        day.future ? "opacity-0" : HEATMAP_LEVEL_CLASS[level]
                      }`}
                      style={{ gridColumn: weekIndex + 1, gridRow: dayIndex + 1 }}
                      aria-label={`${formatHeatmapDate(day.date)}使用了 ${formatExact(day.value)} 个 Token`}
                    />
                  );

                  if (day.future) return cell;

                  return (
                    <Tooltip key={day.key} disableHoverableContent>
                      <TooltipTrigger asChild>{cell}</TooltipTrigger>
                      <TooltipContent
                        side="top"
                        sideOffset={7}
                        className="pointer-events-none rounded-lg bg-foreground px-2.5 py-1.5 text-xs leading-4 text-background shadow-md"
                      >
                        {formatHeatmapDate(day.date)} 使用了 {formatTokens(day.value)} 个 Token
                        {day.records > 0 ? ` · ${formatExact(day.records)} 次调用` : ""}
                      </TooltipContent>
                    </Tooltip>
                  );
                }),
              )}
            </div>
            <div
              className="mt-3 grid gap-1 text-[11px] text-muted-foreground"
              style={{ gridTemplateColumns: "repeat(53, minmax(10px, 1fr))" }}
              aria-hidden="true"
            >
              {monthLabels.map((label, index) => (
                <span key={`${index}-${label ?? "empty"}`} className="whitespace-nowrap">
                  {label}
                </span>
              ))}
            </div>
          </div>
        </div>
      </div>
    </Card>
  );
}

/** 按天 × 按模型的堆叠柱（Token）+ 调用次数折线（同一图，双 Y 轴）。 */
function ModelDailyChart({ points }: { points: TraeModelDailyPoint[] }) {
  const { rows, series } = useMemo(() => {
    const totals = new Map<string, number>();
    for (const point of points) {
      totals.set(point.model, (totals.get(point.model) ?? 0) + point.total);
    }
    const top = [...totals.entries()]
      .sort((left, right) => right[1] - left[1])
      .slice(0, TOP_MODELS)
      .map(([model]) => model);
    const topSet = new Set(top);

    const byDate = new Map<string, { tokens: Map<string, number>; calls: number }>();
    for (const point of points) {
      if (!topSet.has(point.model)) continue;
      const entry = byDate.get(point.date) ?? { tokens: new Map<string, number>(), calls: 0 };
      entry.tokens.set(point.model, (entry.tokens.get(point.model) ?? 0) + point.total);
      entry.calls += point.records;
      byDate.set(point.date, entry);
    }

    const series = top.map((model, index) => ({
      key: `s${index}`,
      model,
      color: MODEL_COLORS[index % MODEL_COLORS.length],
    }));

    const rows = [...byDate.entries()]
      .sort((left, right) => (left[0] < right[0] ? -1 : left[0] > right[0] ? 1 : 0))
      .map(([date, entry]) => {
        const row: Record<string, number | string> = { date, calls: entry.calls };
        series.forEach((item) => {
          row[item.key] = entry.tokens.get(item.model) ?? 0;
        });
        return row;
      });

    return { rows, series };
  }, [points]);

  const config: ChartConfig = useMemo(() => {
    const base: ChartConfig = { calls: { label: "调用次数", color: "var(--data-series-amber)" } };
    series.forEach((item) => {
      base[item.key] = { label: item.model, color: item.color };
    });
    return base;
  }, [series]);

  return (
    <Card className="mb-6 gap-0 py-0">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
        <span className="text-[13px] font-medium">按模型 × 按天</span>
        <span className="text-xs text-muted-foreground">堆叠柱＝每日 Token（按模型拆分）；折线＝当日调用次数</span>
      </div>
      {rows.length === 0 ? (
        <p className="px-5 py-6 text-center text-sm text-muted-foreground">暂无可展示的按模型按天数据</p>
      ) : (
        <>
          <div className="px-3 py-4">
            <ChartContainer config={config} className="h-[260px] w-full">
              <ComposedChart data={rows} margin={{ top: 8, right: 16, bottom: 8, left: 8 }}>
                <CartesianGrid vertical={false} strokeDasharray="3 3" />
                <XAxis
                  dataKey="date"
                  tickLine={false}
                  axisLine={false}
                  tickMargin={8}
                  tickFormatter={(value: string) => String(value).slice(5)}
                />
                <YAxis yAxisId="left" tickLine={false} axisLine={false} width={48} tickFormatter={formatTokens} />
                <YAxis
                  yAxisId="right"
                  orientation="right"
                  tickLine={false}
                  axisLine={false}
                  width={40}
                  tickFormatter={formatExact}
                />
                <ChartTooltip content={<ChartTooltipContent indicator="dot" />} />
                {series.map((item, index) => (
                  <Bar
                    key={item.key}
                    yAxisId="left"
                    dataKey={item.key}
                    stackId="tokens"
                    fill={`var(--color-${item.key})`}
                    radius={index === series.length - 1 ? [4, 4, 0, 0] : 0}
                  />
                ))}
                <Line
                  yAxisId="right"
                  type="monotone"
                  dataKey="calls"
                  stroke="var(--data-series-amber)"
                  strokeWidth={2}
                  dot={false}
                />
              </ComposedChart>
            </ChartContainer>
          </div>
          <div className="flex flex-wrap gap-x-4 gap-y-1.5 border-t border-border/60 px-5 py-3">
            {series.map((item) => (
              <span key={item.key} className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
                <span className="size-2 shrink-0 rounded-full" style={{ backgroundColor: item.color }} aria-hidden="true" />
                {item.model}
              </span>
            ))}
            <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
              <span className="size-2 shrink-0 rounded-full" style={{ backgroundColor: "var(--data-series-amber)" }} aria-hidden="true" />
              调用次数（右轴）
            </span>
          </div>
        </>
      )}
    </Card>
  );
}

/** 平台做不到的维度：置灰卡，逐条带 `supportedOn` / `reason`。 */
function UnsupportedSection({ items }: { items: TraeUnsupported[] }) {
  return (
    <Card className="mb-6 gap-0 border-dashed py-0">
      <div className="border-b border-border/60 px-5 py-3">
        <span className="text-[13px] font-medium">平台不支持的维度</span>
      </div>
      <div className="divide-y divide-border/60">
        {items.map((item) => (
          <div key={item.capability} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-5 py-3 opacity-70">
            <span className="text-sm font-medium text-muted-foreground">{item.label}</span>
            <span className="text-xs text-muted-foreground">（仅 {item.supportedOn}）</span>
            <span className="w-full text-xs leading-5 text-muted-foreground">{item.reason}</span>
          </div>
        ))}
      </div>
    </Card>
  );
}
