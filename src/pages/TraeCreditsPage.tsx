import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  CalendarDays,
  Coins,
  Loader2,
  RefreshCw,
  TrendingDown,
  TrendingUp,
  User,
  type LucideIcon,
} from "lucide-react";
import { CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";
import { toast } from "sonner";

import { DemoAction } from "@/components/demo-action";
import { TraeVariantSwitch } from "@/components/trae-variant-switch";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader } from "@/components/ui/card";
import { ChartContainer, ChartTooltip, ChartTooltipContent, type ChartConfig } from "@/components/ui/chart";
import { Skeleton } from "@/components/ui/skeleton";
import * as api from "@/lib/api";
import type { TraeAccountsOverview, TraeCreditsOverview } from "@/lib/trae-types";
import { cn } from "@/lib/utils";
import { useTraeVariant } from "@/lib/use-trae-variant";

/** 趋势图展示的天数（含今日）。 */
const TREND_DAYS = 7;

const TREND_SERIES = [
  { key: "total", label: "积分总数", color: "var(--data-series-indigo)" },
  { key: "earned", label: "获得积分", color: "var(--data-series-emerald)" },
  { key: "consumed", label: "消耗积分", color: "var(--data-series-amber)" },
] as const;

/** 本地日期 `YYYY-MM-DD`（与后端快照的日期口径一致，不用 UTC 以免跨时区错位）。 */
function localDate(date: Date): string {
  const year = date.getFullYear();
  const month = `${date.getMonth() + 1}`.padStart(2, "0");
  const day = `${date.getDate()}`.padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function formatCredits(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return "—";
  return value.toLocaleString("zh-CN", { maximumFractionDigits: 2 });
}

function StatMetric({
  icon: Icon,
  label,
  value,
  hint,
  tone = "default",
  divided = false,
}: {
  icon: LucideIcon;
  label: string;
  value: string;
  hint?: string;
  tone?: "default" | "up" | "down";
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
        className={cn(
          "mt-3 max-w-full truncate text-[26px] font-semibold leading-8 tracking-[-0.025em] tabular-nums",
          tone === "up"
            ? "text-emerald-600 dark:text-emerald-400"
            : tone === "down"
              ? "text-amber-600 dark:text-amber-400"
              : "text-foreground",
        )}
        style={{ fontFamily: '"Bricolage Grotesque Variable", "SF Pro Display", ui-sans-serif, sans-serif' }}
      >
        {value}
      </div>
      {hint && <div className="mt-1.5 max-w-full truncate text-xs text-muted-foreground">{hint}</div>}
    </div>
  );
}

/**
 * 「积分统计」页（Trae 分区）。
 *
 * 数据源是 `get_trae_credits` 的聚合结果（剩余积分缓存 + 签到明细 + 每日快照），
 * 账号名与分组来自 `get_trae_accounts`——积分缓存只存 uid，展示必须回表取名字。
 */
export default function TraeCreditsPage() {
  /** 当前产品线（由侧栏分区 / URL `?line=` 决定），决定读哪份积分与账号数据。 */
  const [variant] = useTraeVariant();
  const [credits, setCredits] = useState<TraeCreditsOverview | null>(null);
  const [overview, setOverview] = useState<TraeAccountsOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadAll = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // 积分与账号两份数据都按**当前选中的产品线**读：它们的分家文件不同，
      // 混读会让「积分趋势」是 Trae CN 的、而「账号列表」是 Trae Work 的。
      const [creditData, accountData] = await Promise.all([
        api.getTraeCredits(variant),
        api.getTraeAccounts(variant),
      ]);
      setCredits(creditData);
      setOverview(accountData);
    } catch (e) {
      setError(api.asError(e));
    } finally {
      setLoading(false);
    }
  }, [variant]);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  const accounts = overview?.accounts ?? [];
  const groups = overview?.groups ?? [];
  const today = localDate(new Date());

  /** 每个账号的可用积分：`remaining` 是权威缓存，缺失时回落账号视图里的同名字段。 */
  const rows = useMemo(
    () =>
      accounts
        .map((account) => ({
          userId: account.userId,
          name: account.name,
          groupId: account.groupId,
          credits:
            credits?.remaining?.[account.userId] ?? account.remainingCredits ?? account.credits ?? null,
        }))
        .sort((left, right) => {
          if (left.credits === null && right.credits === null) return 0;
          if (left.credits === null) return 1;
          if (right.credits === null) return -1;
          return right.credits - left.credits;
        }),
    [accounts, credits],
  );

  const total = useMemo(() => rows.reduce((sum, row) => sum + (row.credits ?? 0), 0), [rows]);
  const average = rows.length === 0 ? 0 : total / rows.length;

  const todaySnapshot = useMemo(() => credits?.daily.find((item) => item.date === today), [credits, today]);

  /** 今日新增：优先用每日快照的 `earned`（含签到 + 购买），回落签到明细的正增量之和。 */
  const todayEarned = useMemo(() => {
    if (todaySnapshot && todaySnapshot.earned > 0) return todaySnapshot.earned;
    const fromRecords = (credits?.records ?? [])
      .filter((record) => record.date === today)
      .reduce((sum, record) => sum + Math.max(0, record.delta), 0);
    return fromRecords > 0 ? fromRecords : (credits?.todayEarned ?? 0);
  }, [todaySnapshot, credits, today]);

  const todayConsumed = todaySnapshot?.consumed ?? 0;

  /** 近 7 日趋势：缺快照的日期补 0，保证 x 轴天数固定。 */
  const trend = useMemo(() => {
    const byDate = new Map((credits?.daily ?? []).map((item) => [item.date, item]));
    const points: { date: string; label: string; total: number; earned: number; consumed: number }[] = [];
    for (let offset = TREND_DAYS - 1; offset >= 0; offset -= 1) {
      const date = new Date(Date.now() - offset * 86_400_000);
      const key = localDate(date);
      const snapshot = byDate.get(key);
      points.push({
        date: key,
        label: `${date.getMonth() + 1}/${date.getDate()}`,
        total: snapshot?.total ?? 0,
        earned: snapshot?.earned ?? 0,
        consumed: snapshot?.consumed ?? 0,
      });
    }
    // 今日尚无快照时，用当前实时总额补上最后一个点，避免曲线末端突然掉到 0。
    const last = points[points.length - 1];
    if (last && last.total === 0 && total > 0) last.total = Math.round(total);
    return points;
  }, [credits, total]);

  const hasTrend = trend.some((point) => point.total > 0 || point.earned > 0 || point.consumed > 0);

  const chartConfig: ChartConfig = useMemo(
    () => Object.fromEntries(TREND_SERIES.map((series) => [series.key, { label: series.label, color: series.color }])),
    [],
  );

  async function refreshCredits() {
    setRefreshing(true);
    try {
      await api.traeRefreshCredits(undefined, variant);
      await loadAll();
      toast.success("积分数据已刷新");
    } catch (e) {
      toast.error("刷新失败", { description: api.asError(e) });
    } finally {
      setRefreshing(false);
    }
  }

  return (
    <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
      <header className="mb-6 flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-[28px] font-semibold tracking-tight">积分统计</h1>
          <p className="mt-2 text-sm leading-6 text-muted-foreground">
            查看每个 Trae 账号的剩余积分、每日变化与历史趋势。
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {/* 产品线切换器：Trae 分区的每个页面都可切，位置固定在页头右侧动作区。
              侧栏已合并为单个 `Trae` 入口，产品线的选择在这里。 */}
          <TraeVariantSwitch />
          <DemoAction>
            <Button variant="outline" size="sm" disabled={refreshing} onClick={() => void refreshCredits()}>
              {refreshing ? <Loader2 className="animate-spin" /> : <RefreshCw />}
              {refreshing ? "同步中" : "同步积分"}
            </Button>
          </DemoAction>
        </div>
      </header>

      {error && (
        <Alert variant="destructive" className="mb-5">
          <AlertTriangle />
          <AlertTitle>无法读取 Trae 积分数据</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      {loading && !credits ? (
        <Skeleton className="mb-6 h-24 w-full" />
      ) : (
        <Card className="mb-6 min-w-0 gap-0 overflow-hidden rounded-2xl bg-card/70 py-0 shadow-none" aria-label="积分总览">
          <CardContent className="grid min-w-0 grid-cols-1 divide-y divide-border/60 p-0 sm:grid-cols-5 sm:divide-y-0 sm:py-5">
            <StatMetric icon={Coins} label="可用积分总额" value={formatCredits(total)} hint="全部账号合计" />
            <StatMetric
              icon={CalendarDays}
              label="平均可用积分"
              value={formatCredits(Math.round(average))}
              hint="总额 ÷ 账号数"
              divided
            />
            <StatMetric
              icon={User}
              label="账号数"
              value={String(rows.length)}
              hint={`已同步 ${rows.filter((row) => row.credits !== null).length}`}
              divided
            />
            <StatMetric
              icon={TrendingUp}
              label="今日新增积分"
              value={formatCredits(todayEarned)}
              hint={today}
              tone={todayEarned > 0 ? "up" : "default"}
              divided
            />
            <StatMetric
              icon={TrendingDown}
              label="今日消耗积分"
              value={formatCredits(todayConsumed)}
              hint={today}
              tone={todayConsumed > 0 ? "down" : "default"}
              divided
            />
          </CardContent>
          {credits?.updatedAt && (
            <div className="border-t border-border/60 px-5 py-2.5 text-xs text-muted-foreground">
              积分缓存更新时间：{credits.updatedAt}
              {credits.historyDays > 0 && <span className="ml-3">历史明细保留 {credits.historyDays} 天</span>}
            </div>
          )}
        </Card>
      )}

      <section className="min-w-0 space-y-2.5" aria-labelledby="trae-credits-trend-title">
        <div className="flex flex-wrap items-center justify-between gap-2 px-1">
          <h2 id="trae-credits-trend-title" className="text-[13px] font-medium leading-5">
            近 {TREND_DAYS} 日积分趋势
          </h2>
          <div className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
            {TREND_SERIES.map((series) => (
              <span key={series.key} className="inline-flex items-center gap-1.5">
                <span className="size-2 shrink-0 rounded-full" style={{ backgroundColor: series.color }} aria-hidden="true" />
                {series.label}
              </span>
            ))}
          </div>
        </div>
        <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
          <CardHeader className="gap-0 px-4 pt-3 pb-0 sm:px-5">
            <CardDescription className="text-xs">
              数据来自每日积分快照；执行签到或同步积分后才会产生当天的点。
            </CardDescription>
          </CardHeader>
          <CardContent className="min-w-0 px-4 pt-3 pb-4 sm:px-5">
            {loading && !credits ? (
              <Skeleton className="h-56 w-full" />
            ) : rows.length === 0 ? (
              <div className="rounded-lg border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
                尚无账号数据。添加账号后这里会展示积分趋势。
              </div>
            ) : !hasTrend ? (
              <div className="rounded-lg border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
                暂无趋势数据。执行一次签到或「同步积分」后即可看到每日变化。
              </div>
            ) : (
              <ChartContainer config={chartConfig} className="h-56 w-full">
                <LineChart data={trend} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                  <CartesianGrid vertical={false} strokeDasharray="3 3" />
                  <XAxis dataKey="label" tickLine={false} axisLine={false} tickMargin={8} />
                  <YAxis
                    tickLine={false}
                    axisLine={false}
                    width={52}
                    tickFormatter={(value) => formatCredits(Number(value))}
                  />
                  <ChartTooltip
                    cursor={{ stroke: "var(--border)", strokeDasharray: "3 3" }}
                    content={<ChartTooltipContent labelFormatter={(_, payload) => {
                      const item = Array.isArray(payload) ? payload[0] : payload;
                      return String(item?.payload?.date ?? "");
                    }} />}
                  />
                  {TREND_SERIES.map((series) => (
                    <Line
                      key={series.key}
                      type="monotone"
                      dataKey={series.key}
                      stroke={series.color}
                      strokeWidth={series.key === "total" ? 2.5 : 2}
                      dot={{ r: 3, fill: series.color, strokeWidth: 0 }}
                      activeDot={{ r: 5 }}
                      isAnimationActive={false}
                    />
                  ))}
                </LineChart>
              </ChartContainer>
            )}
          </CardContent>
        </Card>
      </section>

      {/* ---- 平台做不到的维度（置灰说明；形状来自 handlers::unsupported_note） ---- */}
      {(credits?.unsupported.length ?? 0) > 0 && (
        <section className="mt-6 min-w-0 space-y-2.5" aria-labelledby="trae-credits-unsupported-title">
          <div className="px-1">
            <h2 id="trae-credits-unsupported-title" className="text-[13px] font-medium leading-5">
              平台不支持的维度
            </h2>
          </div>
          <Card className="min-w-0 gap-0 overflow-hidden rounded-xl border-dashed py-0 shadow-none">
            <div className="divide-y divide-border/60">
              {credits?.unsupported.map((item) => (
                <div key={item.capability} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-5 py-3 opacity-70">
                  <span className="text-sm font-medium text-muted-foreground">{item.label}</span>
                  <span className="text-xs text-muted-foreground">（仅 {item.supportedOn}）</span>
                  <span className="w-full text-xs leading-5 text-muted-foreground">{item.reason}</span>
                </div>
              ))}
            </div>
          </Card>
        </section>
      )}

      <section className="mt-6 min-w-0 space-y-2.5" aria-labelledby="trae-credits-detail-title">
        <div className="px-1">
          <h2 id="trae-credits-detail-title" className="text-[13px] font-medium leading-5">
            账号积分明细
          </h2>
          {/* 口径说明：Trae 积分来自签到快照，**没有**「请求用量」这一口径的数据源，
              因此这里是单栏明细 + 说明，而不是 WorkBuddy 那套「明细 / 请求用量」分栏。 */}
          <p className="mt-1 text-xs leading-5 text-muted-foreground">
            数据来源为签到快照与「同步积分」的结果；Trae 侧不存在「产生这些积分的请求用量」口径，故不设分栏。
          </p>
        </div>
        <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
          <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
            <span className="text-[13px] font-medium">按账号</span>
            <span className="text-xs text-muted-foreground">共 {rows.length} 个账号</span>
          </div>
          {loading && !overview ? (
            <div className="p-4">
              <Skeleton className="h-40 w-full" />
            </div>
          ) : rows.length === 0 ? (
            <div className="px-4 py-10 text-center text-sm text-muted-foreground">尚无账号数据。</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full min-w-[700px] text-sm">
                <thead className="bg-muted/50 text-xs text-muted-foreground">
                  <tr>
                    <th className="px-4 py-2.5 text-left font-medium">排名</th>
                    <th className="px-4 py-2.5 text-left font-medium">账号</th>
                    <th className="px-4 py-2.5 text-left font-medium">分组</th>
                    <th className="px-4 py-2.5 text-left font-medium">积分到期</th>
                    <th className="px-4 py-2.5 text-right font-medium">剩余可用积分</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-border/60">
                  {rows.map((row, index) => {
                    const group = groups.find((item) => item.id === row.groupId);
                    const expireAt = credits?.expireTimes?.[row.userId];
                    return (
                      <tr key={row.userId}>
                        <td className="px-4 py-2.5 tabular-nums text-muted-foreground">#{index + 1}</td>
                        <td className="px-4 py-2.5">
                          <div className="font-medium">{row.name}</div>
                          <div className="font-mono text-xs text-muted-foreground">{row.userId}</div>
                        </td>
                        <td className="px-4 py-2.5">
                          {group ? (
                            <Badge variant="secondary" className="gap-1.5">
                              <span
                                className="size-2 rounded-full"
                                style={{ backgroundColor: group.color }}
                                aria-hidden="true"
                              />
                              {group.name}
                            </Badge>
                          ) : (
                            <span className="text-xs text-muted-foreground">未分组</span>
                          )}
                        </td>
                        <td className="px-4 py-2.5 text-xs text-muted-foreground">
                          {expireAt
                            ? new Date(expireAt * 1000).toLocaleDateString("zh-CN")
                            : "—"}
                        </td>
                        <td className="px-4 py-2.5 text-right tabular-nums">
                          {row.credits === null ? (
                            <span className="text-xs text-muted-foreground">未同步</span>
                          ) : (
                            formatCredits(row.credits)
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </Card>
      </section>
    </div>
  );
}
