import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  CircleSlash,
  Columns3,
  Download,
  FileDown,
  FileUp,
  Loader2,
  QrCode,
  RefreshCw,
  Rows3,
  UserPlus,
  XCircle,
} from "lucide-react";
import { toast } from "sonner";

import { DemoAction } from "@/components/demo-action";
import { TraeVariantSwitch } from "@/components/trae-variant-switch";
import { TraeAccountCard } from "@/components/trae-account-card";
import { TraeExportAccountsDialog } from "@/components/trae-export-accounts-dialog";
import { TraeImportAccountsDialog } from "@/components/trae-import-accounts-dialog";
import { TraeOAuthLoginDialog } from "@/components/trae-oauth-login-dialog";
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
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import * as api from "@/lib/api";
import { isAutoDetected, traeProductLabel } from "@/lib/trae-client";
import { traeVariantLabel } from "@/lib/trae-types";
import type {
  TraeAccount,
  TraeAccountsOverview,
  TraeCheckinEvent,
  TraeCheckinReport,
  TraeCheckinStatus,
  TraeCreditsOverview,
  TraeEnvStatus,
  TraeProfilesOverview,
  TraeSettings,
  TraeVariantId,
} from "@/lib/trae-types";
import { cn } from "@/lib/utils";
import { useCompactMode } from "@/lib/use-compact-mode";
import { useTraeVariant } from "@/lib/use-trae-variant";

/** 未分组在过滤器里的哨兵值（`Select` 不接受空字符串作为 value）。 */
const UNGROUPED = "__ungrouped__";

/**
 * 「账号管理」页（Trae 分区）。
 *
 * ## 与 WorkBuddy 页面的关系
 *
 * **骨架逐段照抄** WorkBuddy 的 `AccountsPage`：产品状态标记行 → 「添加与迁移账号」
 * 横幅 → 环境说明行 → 空态卡 → 账号工具栏（开关 / 分组筛选 / 紧凑 / 刷新）→
 * 卡片栅格 → 各确认 Dialog。用户在两个分区之间切换时不需要重新学习界面。
 *
 * **文案与数据源按 Trae 实情落地**，不搬 WorkBuddy 的：
 * - 没有 region 双版本 Tabs（Trae 只有一套账号库，没有国内版/国际版之分）；
 * - 没有 OAuth 扫码（Trae 只支持粘贴 `Cloud-IDE-JWT`）；
 * - 没有自动签到定时器（Trae 侧无调度器），对应位置是「跳过今日已签」这一
 *   真实生效的批量签到策略开关；
 * - 没有自动旅行 / CodeBuddy CLI / CodeBuddy IDE（Trae 侧不存在这些客户端）。
 */
export default function TraeAccountsPage() {
  const [overview, setOverview] = useState<TraeAccountsOverview | null>(null);
  const [env, setEnv] = useState<TraeEnvStatus | null>(null);
  /**
   * 当前正在管理的**产品线变体**，由侧栏分区决定（URL `?line=`）。
   *
   * 为什么**不再**用「自动探测挑中的那一条」：探测是无入参、横跨全部变体的
   * 全局行为，它回答的是「本机哪条线最近活跃」，而不是「用户此刻想管哪条线」。
   * 用户点了侧栏的「Trae CN」，页面就必须是 Trae CN —— 哪怕 Trae Work 更活跃。
   *
   * 探测结果仍然有用：它作为**兜底**（URL 没有显式指定时）以及用于展示
   * 「该线的安装/运行状态」。两者分工不同，不要混为一谈。
   */
  const [variant] = useTraeVariant();
  const [credits, setCredits] = useState<TraeCreditsOverview | null>(null);
  const [checkin, setCheckin] = useState<TraeCheckinStatus | null>(null);
  const [profiles, setProfiles] = useState<TraeProfilesOverview | null>(null);
  const [settings, setSettings] = useState<TraeSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [autoCheckinSaving, setAutoCheckinSaving] = useState(false);
  const [oauthOpen, setOauthOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [importing, setImporting] = useState(false);
  const [groupFilter, setGroupFilter] = useState<string>("all");
  const [progress, setProgress] = useState<TraeCheckinEvent | null>(null);
  const [report, setReport] = useState<TraeCheckinReport | null>(null);
  const [compact, toggleCompact] = useCompactMode();
  /** 待确认删除的账号。用 shadcn Dialog 承载确认，而非 `window.confirm`——
   *  Tauri WebView 不支持原生 confirm，且原生弹窗无法保持主题与无障碍契约。 */
  const [pendingDelete, setPendingDelete] = useState<TraeAccount | null>(null);

  const loadAll = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // 四份分家数据全部按**当前选中的产品线**读取，与侧栏分区同源。
      // `env` 仍然单独取：它是「单一视角」的环境快照，用于展示该线的安装/运行状态。
      const [accountData, envData, creditData, checkinData, profileData, settingsData] =
        await Promise.all([
          api.getTraeAccounts(variant),
          api.getTraeEnv(),
          api.getTraeCredits(variant),
          api.getTraeCheckinStatus(variant),
          api.getTraeProfiles(variant),
          api.getTraeSettings(),
        ]);
      setOverview(accountData);
      setEnv(envData);
      setCredits(creditData);
      setCheckin(checkinData);
      setProfiles(profileData);
      setSettings(settingsData);
    } catch (e) {
      setError(api.asError(e));
    } finally {
      setLoading(false);
    }
  }, [variant]);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  // 签到进度：仅桌面端有事件通道；webui 只在结束时拿到完整报告。
  useEffect(() => {
    if (!api.isDesktop()) return;
    let dispose: (() => void) | undefined;
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        dispose = await listen<TraeCheckinEvent>("trae-checkin-progress", (event) => {
          setProgress(event.payload);
        });
      } catch {
        // 事件通道不可用不影响主流程（仍可用返回的完整报告渲染结果）。
      }
    })();
    return () => dispose?.();
  }, []);

  /** 统一的动作执行：加忙标记、成功提示、失败提示、随后刷新聚合数据。 */
  async function run<T>(key: string, label: string, action: () => Promise<T>, after?: (result: T) => void) {
    setBusy(key);
    try {
      const result = await action();
      after?.(result);
      toast.success(`${label}完成`);
      await loadAll();
    } catch (e) {
      toast.error(`${label}失败`, { description: api.asError(e) });
    } finally {
      setBusy(null);
    }
  }

  /**
   * 「跳过今日已签到」开关。
   *
   * 与 WorkBuddy 工具栏上的「自动签到」开关**同位**，但语义按 Trae 实情落地：
   * WorkBuddy 的自动签到是定时任务（到点由调度器触发），而 Trae 侧的签到
   * 只在用户点「全部签到」时执行，因此这里暴露的是「批量签到的跳过策略」
   * ——一个真实生效、且与用户决策直接相关的开关，而不是一个没有后台支撑的假自动。
   */
  async function onSkipCheckedChange(enabled: boolean) {
    if (!settings || autoCheckinSaving) return;
    const previous = settings;
    setSettings({ ...settings, checkinSkipChecked: enabled });
    setAutoCheckinSaving(true);
    try {
      setSettings(await api.saveTraeSettings({ checkinSkipChecked: enabled }));
    } catch (e) {
      setSettings(previous);
      toast.error("设置保存失败", { description: api.asError(e) });
    } finally {
      setAutoCheckinSaving(false);
    }
  }

  /**
   * 工具栏唯一图标按钮：**签到并刷新全部账号积分**（与 WorkBuddy 同名同形）。
   *
   * Trae 侧这是两个接口（签到、积分快照），WorkBuddy 是一个后端动作，
   * 但对用户的语义完全一致——「点一下把我所有账号都签一遍并把积分刷新出来」。
   * 因此这里必须**两步都做完**再收工：只签到会让卡片上的积分停留在旧值，
   * 用户点完看到的数字没动，会以为签到没生效。
   *
   * 跳过策略由 `settings.checkinSkipChecked` 决定（后端读，不在这里传）。
   */
  async function checkinAll() {
    await run(
      "checkin",
      "签到并刷新积分",
      async () => {
        // 变体决定「签哪条产品线的账号」以及「冷却/摘要落哪份文件」，
        // 不传会让后端回落默认变体 —— 在 Trae CN 页面上点签到却签了 Trae Work。
        const result = await api.traeCheckin({ scope: "all", variant });
        // 积分刷新失败不应让整次操作报错：签到本身可能已经成功，
        // 把「积分没刷出来」当成签到失败会让用户误以为一分没拿到。
        try {
          await api.traeRefreshCredits(undefined, variant);
        } catch (e) {
          toast.warning("积分刷新失败", { description: api.asError(e) });
        }
        return result;
      },
      (result) => setReport(result),
    );
  }

  /** 导入本机账号：读客户端登录态里的 `Cloud-IDE-JWT`，已存在的账号就地覆盖刷新。 */
  async function importLocal() {
    if (importing) return;
    setImporting(true);
    try {
      // 把当前产品线传下去：Trae 多条产品线可同机并存，不传会让后端按「最近活跃」
      // 全局挑一条 —— 用户在 Trae Work 分区导入却可能读到 Trae CN 的目录，
      // 连报错文案都会说错产品线。
      //
      // 用已解析好的 `variant` 状态而不是 `env?.variant`：后者在 env 还没加载完时
      // 是 `null`，会让「用户抢在加载完成前点导入」落到默认变体上；
      // `variant` 初值就是默认变体、加载完成后被修正，语义更稳。
      const result = await api.traeImportLocalAccount(variant);
      toast.success("已导入本机账号", {
        description: `${result.name || result.userId}${result.userId ? ` · UID ${result.userId.slice(-6)}` : ""}`,
      });
      await loadAll();
    } catch (e) {
      toast.error("导入本机账号失败", { description: api.asError(e) });
    } finally {
      setImporting(false);
    }
  }

  const accounts = overview?.accounts ?? [];
  const groups = overview?.groups ?? [];

  const visible = useMemo(() => {
    if (groupFilter === "all") return accounts;
    if (groupFilter === UNGROUPED) return accounts.filter((account) => !account.groupId);
    return accounts.filter((account) => account.groupId === groupFilter);
  }, [accounts, groupFilter]);

  const coolingCount = overview?.cooling ?? 0;
  const expiringSoon = accounts.filter((account) => account.jwtStatus === "warn").length;
  /** 当前登录账号：由「登录态快照」记录的槽位判定，用于卡片上的当前账号角标。 */
  const currentUserId = profiles?.currentAccount ?? null;
  const switchBusy = busy?.startsWith("switch-") ?? false;
  // 探测到的是哪条产品线。同机装多个 Trae 时，用户靠这个确认切换器管的是哪一个。
  const productLabel = traeProductLabel(env);
  const autoDetected = isAutoDetected(env);
  /** 当前产品线的展示名；拿不到探测结果时回落 `variant` 状态的展示名。 */
  const variantLabel = productLabel ?? traeVariantLabel(variant);

  const checkedToday = accounts.filter((account) => account.checkedToday).length;
  const totalCredits = accounts.reduce(
    (sum, account) => sum + (account.remainingCredits ?? account.credits ?? 0),
    0,
  );

  function groupNameOf(account: TraeAccount): string | null {
    if (!account.groupId) return null;
    return groups.find((group) => group.id === account.groupId)?.name ?? "分组";
  }

  return (
    <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
      {/* 页头与 WorkBuddy 逐字同构：**只有标题与说明，不放任何动作按钮**。
          动作一律下沉到「添加与迁移账号」横幅与账号工具栏，
          这样两个分区的动作位置、视觉重量完全一致。
          曾经这里堆了 4 个按钮（刷新 / 刷新积分 / 全部签到 / 添加账号），
          与 WorkBuddy 的骨架冲突，也让同一功能出现两个入口。 */}
      <header className="mb-6">
        <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-3">
          <div className="min-w-0">
            <h1 className="text-[28px] font-semibold tracking-tight">账号管理</h1>
            <p className="mt-2 text-sm leading-6 text-muted-foreground">
              管理 {variantLabel} 账号的登录凭据、签到与登录态切换。与 WorkBuddy 的账号库彼此独立。
            </p>
          </div>
          {/* 产品线切换器放在页头右侧（详情页动作区域），宽屏下与标题同排、
              窄屏自动换行到下一行。它从侧栏下沉到这里的原因见组件自身注释。 */}
          <TraeVariantSwitch className="shrink-0" />
        </div>
      </header>

      {error && (
        <Alert variant="destructive" className="mb-4">
          <AlertTriangle />
          <AlertTitle>无法读取 Trae 数据</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      {loading && !overview ? (
        <Skeleton className="mb-6 h-24 w-full" />
      ) : accounts.length === 0 ? (
        /* 空态：与 WorkBuddy 的 `EmptyRegionCard` 同构（可能原因 + 期望产物 + 两个动作）。 */
        <EmptyTraeCard
          installed={env?.installed ?? false}
          dataDir={env?.dataDir ?? null}
          onRecheck={() => void loadAll()}
          onImport={() => void importLocal()}
          onOAuth={() => setOauthOpen(true)}
          importing={importing}
        />
      ) : (
        <>
          {/* 「添加与迁移账号」横幅：位置、结构与 WorkBuddy 完全一致。 */}
          <div className="relative mb-6 overflow-visible rounded-2xl border border-border bg-muted/30 px-5 py-5 shadow-[0_6px_20px_rgba(15,23,42,.025)]">
            <div className="pointer-events-none absolute inset-0 overflow-hidden rounded-2xl">
              <div className="absolute -right-12 -top-20 size-44 rounded-full border-[28px] border-slate-400/[0.035]" />
            </div>
            <div className="relative flex flex-wrap items-center gap-x-5 gap-y-4">
              <div className="min-w-[190px] flex-1">
                <h2 className="text-sm font-semibold text-foreground">添加与迁移账号</h2>
                <p className="mt-1 text-xs leading-5 text-muted-foreground">快速接入新账号，或从已有环境恢复</p>
              </div>
              <div className="flex flex-wrap items-center gap-2.5">
                {/* 主入口与 WorkBuddy 同形同位：OAuth。Trae 的授权页是普通网页
                    （不是二维码），因此文案是「网页登录」而不是「扫码添加」。 */}
                <DemoAction>
                  <Button
                    className="h-10 bg-primary px-4 text-primary-foreground shadow-sm hover:bg-primary/90"
                    onClick={() => setOauthOpen(true)}
                  >
                    <QrCode />OAuth 网页登录
                  </Button>
                </DemoAction>
                <DemoAction>
                  <Button className="h-10 px-4" onClick={() => void importLocal()} disabled={importing} variant="outline">
                    {importing ? <Loader2 className="animate-spin" /> : <Download />}导入本机账号
                  </Button>
                </DemoAction>
              </div>
              <div className="flex items-center gap-1">
                {/* 粘贴 JWT 是 Trae 独有且**已降级**的兜底路径（OAuth 才是主路径），
                    按「Trae 独有项下沉」放进 ghost 组，与备份导入导出同级。 */}
                <DemoAction>
                  <Button variant="ghost" size="sm" className="h-9 px-2.5" onClick={() => setAddOpen(true)} title="手动粘贴 Cloud-IDE-JWT（兜底方式，优先用上方网页登录）">
                    <UserPlus />粘贴 JWT
                  </Button>
                </DemoAction>
                <DemoAction>
                  <Button variant="ghost" size="sm" className="h-9 px-2.5" onClick={() => setImportOpen(true)} title="从备份文件导入账号">
                    <FileUp />导入备份
                  </Button>
                </DemoAction>
                <DemoAction>
                  <Button variant="ghost" size="sm" className="h-9 px-2.5" onClick={() => setExportOpen(true)} disabled={accounts.length === 0} title="导出账号备份">
                    <FileDown />导出
                  </Button>
                </DemoAction>
              </div>
            </div>
          </div>

          {/* 客户端环境说明行：Trae 独有（WorkBuddy 的环境信息在卡片里），下沉为一条细说明。 */}
          <Card className="mb-6 gap-0 py-0">
            <div className="flex flex-wrap items-center gap-x-8 gap-y-3 px-5 py-3.5 text-sm">
              <span className="flex items-center gap-2">
                客户端
                <span className={cn("font-medium", env?.installed ? "text-emerald-600" : "text-muted-foreground")}>
                  {env?.installed ? `已安装${env.version ? ` v${env.version}` : ""}` : "未检测到"}
                </span>
                {env?.installed && productLabel && (
                  <TooltipProvider delayDuration={400}>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Badge variant="secondary" className="cursor-default">
                          {productLabel}
                          {!autoDetected && <span className="ml-1 text-muted-foreground">· 手动指定</span>}
                        </Badge>
                      </TooltipTrigger>
                      <TooltipContent className="max-w-md">
                        <div className="space-y-1 font-mono text-xs break-all">
                          <div>{env.path ?? "—"}</div>
                          {env.dataDir && <div className="text-muted-foreground">{env.dataDir}</div>}
                        </div>
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                )}
              </span>
              <span className="flex items-center gap-2">
                运行状态
                <span className={cn("inline-flex items-center gap-1.5 font-medium", env?.running ? "text-emerald-600" : "text-muted-foreground")}>
                  <span className={cn("size-2 rounded-full", env?.running ? "bg-emerald-500" : "bg-muted-foreground/50")} />
                  {env?.running ? "运行中" : "未运行"}
                </span>
              </span>
              <span className="text-muted-foreground">
                今日已签 <span className="font-medium text-foreground">{checkedToday}</span>
              </span>
              <span className="text-muted-foreground">
                可用积分 <span className="font-medium text-foreground">{totalCredits.toLocaleString("zh-CN")}</span>
              </span>
              {credits && (
                <span className="text-muted-foreground">
                  今日新增 <span className="font-medium text-foreground">{credits.todayEarned}</span>
                </span>
              )}
              {expiringSoon > 0 && (
                <span className="text-amber-600 dark:text-amber-400">
                  JWT 临期 <span className="font-medium">{expiringSoon}</span>
                </span>
              )}
              {coolingCount > 0 && (
                <span className="flex items-center gap-2 text-muted-foreground">
                  <CircleSlash className="size-3.5" />
                  冷却中 <span className="font-medium text-foreground">{coolingCount}</span>
                  <DemoAction>
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={busy === "clear-cooldown"}
                      onClick={() => void run("clear-cooldown", "清除冷却", () => api.traeClearCooldown(undefined, variant))}
                    >
                      全部清除
                    </Button>
                  </DemoAction>
                </span>
              )}
            </div>
            {env?.dataDir && (
              <div className="border-t border-border/60 px-5 py-2.5 text-xs text-muted-foreground">
                客户端数据目录：<code className="font-mono">{env.dataDir}</code>
                {!env.dataDirExists && <span className="ml-2 text-amber-600 dark:text-amber-400">（尚未生成，请先启动一次 Trae 并登录）</span>}
              </div>
            )}
          </Card>

          <section className="mt-7 min-w-0" aria-labelledby="trae-accounts-list-title">
            <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
              <div className="flex items-center gap-2">
                <h2 id="trae-accounts-list-title" className="text-base font-semibold tracking-tight">
                  账号
                </h2>
                <Badge
                  variant="secondary"
                  className="h-6 min-w-6 rounded-full border-0 px-1.5 text-[11px] tabular-nums text-muted-foreground shadow-none"
                  aria-label={`${accounts.length} 个账号`}
                >
                  {accounts.length}
                </Badge>
                {/* 分组筛选是 Trae 独有项（WorkBuddy 没有分组维度），
                    按「独有项下沉」放在标题侧，不占用右侧工具栏的固定槽位——
                    右侧必须与 WorkBuddy 保持「开关 → 分隔符 → 两个图标」的一致节奏。 */}
                <Select value={groupFilter} onValueChange={setGroupFilter}>
                  <SelectTrigger size="sm" className="ml-1 w-40" aria-label="按分组筛选">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">全部账号（{accounts.length}）</SelectItem>
                    <SelectItem value={UNGROUPED}>未分组（{overview?.ungrouped ?? 0}）</SelectItem>
                    {groups.map((group) => (
                      <SelectItem key={group.id} value={group.id}>
                        {group.name}（{group.count}）
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <TooltipProvider delayDuration={400}>
                <div className="ml-auto flex items-center gap-1">
                  {/* 签到策略：与 WorkBuddy 工具栏的「自动签到」开关**同位**。
                      Trae 侧没有调度器，开关的实际语义是「批量签到是否跳过今日已签账号」。
                      WorkBuddy 该位置的第二个开关是「自动旅行」——Trae 没有这个功能，
                      按用户决定「Trae 无自动旅行则隐藏」处理，不塞一个假开关占位。 */}
                  <div className="mr-1 flex items-center gap-2.5">
                    <label
                      htmlFor="trae-skip-checked"
                      className="cursor-pointer text-xs font-medium text-muted-foreground"
                    >
                      跳过已签到
                    </label>
                    <DemoAction>
                      <Switch
                        id="trae-skip-checked"
                        checked={settings?.checkinSkipChecked ?? true}
                        disabled={!settings || autoCheckinSaving}
                        onCheckedChange={(enabled) => void onSkipCheckedChange(enabled)}
                        aria-label="跳过今日已签到账号"
                      />
                    </DemoAction>
                    {autoCheckinSaving && (
                      <Loader2
                        className="size-3.5 animate-spin text-muted-foreground"
                        aria-label="正在保存签到设置"
                      />
                    )}
                  </div>
                  <Separator orientation="vertical" className="mx-2 h-5" />
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        className={cn("size-9 rounded-lg", compact && "bg-accent text-accent-foreground")}
                        onClick={toggleCompact}
                        aria-label={compact ? "切换为宽松模式" : "切换为紧凑模式"}
                      >
                        {compact ? <Rows3 /> : <Columns3 />}
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent side="top">{compact ? "切换为宽松模式" : "切换为紧凑模式"}</TooltipContent>
                  </Tooltip>
                  {/* 与 WorkBuddy 同名同形：一个图标同时承担「批量签到」与「刷新积分」。
                      Trae 侧这两步是两个接口，`checkinAll` 里按顺序调完再统一刷新。 */}
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <span>
                        <DemoAction>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="size-9 rounded-lg"
                            disabled={busy === "checkin" || busy === "refresh-credits" || accounts.length === 0}
                            onClick={() => void checkinAll()}
                            aria-label="签到并刷新全部账号积分"
                          >
                            <RefreshCw className={busy === "checkin" || busy === "refresh-credits" ? "animate-spin" : undefined} />
                          </Button>
                        </DemoAction>
                      </span>
                    </TooltipTrigger>
                    <TooltipContent side="top">
                      {api.isDemoMode() ? "演示模式下不可操作" : "签到并刷新全部账号积分"}
                    </TooltipContent>
                  </Tooltip>
                </div>
              </TooltipProvider>
            </div>

            {visible.length === 0 ? (
              <Card className="px-5 py-10 text-center text-sm text-muted-foreground">
                该分组下没有账号，换个分组看看。
              </Card>
            ) : (
              <div className={cn("grid min-w-0 items-start gap-5", compact ? "grid-cols-[repeat(auto-fit,minmax(min(100%,300px),1fr))]" : "grid-cols-[repeat(auto-fit,minmax(min(100%,340px),1fr))]")}>
                {visible.map((account) => (
                  <TraeAccountCard
                    key={account.userId}
                    account={account}
                    groupName={groupNameOf(account)}
                    current={account.userId === currentUserId}
                    compact={compact}
                    busy={busy}
                    switchBusy={switchBusy}
                    featuresDisabled={api.isDemoMode()}
                    onSwitch={(target) =>
                      void run(
                        `switch-${target.userId}`,
                        "切换账号",
                        () => api.traeSwitchAccount({ userId: target.userId, launch: true, variant }),
                        (outcome) => {
                          if (!outcome.success) {
                            const last = outcome.steps[outcome.steps.length - 1];
                            toast.error("切换未完成", { description: outcome.error ?? last?.message });
                          }
                        },
                      )
                    }
                    onSaveLogin={(target) =>
                      void run(`save-${target.userId}`, "保存登录态", () => api.traeSaveLogin(target.userId, variant))
                    }
                    onRefreshJwt={(target) =>
                      void run(`jwt-${target.userId}`, "刷新 JWT", () => api.traeRefreshJwt(target.userId, variant))
                    }
                    onClearCooldown={(target) =>
                      void run(`thaw-${target.userId}`, "解除冷却", () => api.traeClearCooldown(target.userId, variant))
                    }
                    onDelete={(target) => setPendingDelete(target)}
                  />
                ))}
              </div>
            )}
          </section>
        </>
      )}

      {/* 签到进度 / 结果：与「全部签到」同源，就地展开不做成独立页。 */}
      {(progress || report) && (
        <Card className="mt-6 gap-0 py-0">
          <div className="flex items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
            <span className="text-sm font-semibold">签到结果</span>
            {progress?.type === "account" && (
              <span className="text-xs text-muted-foreground">
                正在处理 {progress.index} / {report?.total ?? "…"}
              </span>
            )}
          </div>
          <div className="divide-y divide-border/60">
            {(report?.results ?? []).map((item) => (
              <div key={item.userId + item.name} className="flex items-center justify-between gap-3 px-5 py-2.5 text-sm">
                <span className="min-w-0 truncate">{item.name}</span>
                <span className="flex shrink-0 items-center gap-3">
                  {item.delta > 0 && <span className="text-emerald-600 dark:text-emerald-400">+{item.delta}</span>}
                  <span className="text-xs text-muted-foreground">{item.message || item.action}</span>
                  <Badge variant={item.ok ? "secondary" : "destructive"}>
                    {item.action === "skip_already" ? "已签到" : item.ok ? "成功" : "失败"}
                  </Badge>
                </span>
              </div>
            ))}
            {!report && progress?.type === "account" && (
              <div className="px-5 py-2.5 text-sm text-muted-foreground">
                {progress.name}：{progress.status === "success" ? "成功" : progress.status === "already" ? "已签到" : "失败"}
              </div>
            )}
          </div>
          {report && report.warnings.length > 0 && (
            <div className="border-t border-border/60 px-5 py-3 text-xs text-amber-600 dark:text-amber-400">
              {report.warnings.map((warning) => (
                <div key={warning}>{warning}</div>
              ))}
            </div>
          )}
        </Card>
      )}

      {/* 上次签到明细（含冷却）：与 WorkBuddy 一样就地展开，不占导航。 */}
      {(checkin?.summary.results.length ?? 0) > 0 && (
        <Card className="mt-6 gap-0 py-0">
          <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
            <span className="text-sm font-semibold">上次签到</span>
            <span className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
              <span>{checkin?.summary.time ?? "时间未知"}</span>
              {checkin && !checkin.summaryIsToday && (
                <span className="text-amber-600 dark:text-amber-400">（非今日）</span>
              )}
              <span className="flex items-center gap-1.5">
                <CheckCircle2 className="size-3.5 text-emerald-600 dark:text-emerald-400" />
                成功 <span className="font-medium text-foreground">{checkin?.summary.totalOk ?? 0}</span>
                <XCircle className="ml-1.5 size-3.5 text-destructive" />
                失败 <span className="font-medium text-foreground">{checkin?.summary.failed ?? 0}</span>
              </span>
            </span>
          </div>
          <div className="divide-y divide-border/60">
            {checkin?.summary.results.slice(0, 10).map((item, index) => (
              <div
                key={`${item.userId}-${index}`}
                className="flex items-center justify-between gap-3 px-5 py-2.5 text-sm"
              >
                <span className="min-w-0 truncate">{item.name || item.userId}</span>
                <span className="flex shrink-0 items-center gap-3">
                  {item.delta > 0 && (
                    <span className="text-emerald-600 dark:text-emerald-400">+{item.delta}</span>
                  )}
                  <span className="max-w-[280px] truncate text-xs text-muted-foreground">
                    {item.message || item.action}
                  </span>
                </span>
              </div>
            ))}
          </div>
          {(checkin?.summary.results.length ?? 0) > 10 && (
            <div className="border-t border-border/60 px-5 py-2.5 text-xs text-muted-foreground">
              仅展示前 10 条，共 {checkin?.summary.results.length} 条。
            </div>
          )}
          {checkin && checkin.cooldownCount > 0 && (
            <div className="border-t border-border/60 px-5 py-2.5 text-xs text-muted-foreground">
              冷却明细：{checkin.cooldowns.map((item) => `${item.userId}（${item.type}${item.permanent ? "·永久" : ""}）`).join("、")}
            </div>
          )}
        </Card>
      )}

      <TraeOAuthLoginDialog
        open={oauthOpen}
        onOpenChange={setOauthOpen}
        onSuccess={(account) => {
          toast.success("已添加账号", {
            description: `${account.name || account.userId}${account.hasRefreshToken ? " · 支持自动续期" : ""}`,
          });
          void loadAll();
        }}
      />

      <AddAccountDialog
        open={addOpen}
        onOpenChange={setAddOpen}
        groups={groups}
        variant={variant}
        onAdded={() => void loadAll()}
      />

      <TraeExportAccountsDialog
        open={exportOpen}
        onOpenChange={setExportOpen}
        accounts={accounts}
        variant={variant}
        onExported={(count) => toast.success(`已导出 ${count} 个账号`)}
      />

      <TraeImportAccountsDialog
        open={importOpen}
        onOpenChange={setImportOpen}
        variant={variant}
        onImported={(result) => {
          toast.success("导入完成", {
            description: `新增 ${result.imported} · 覆盖 ${result.overwritten} · 跳过 ${result.skipped}`,
          });
          void loadAll();
        }}
      />

      {/* 删除确认 */}
      <Dialog open={pendingDelete !== null} onOpenChange={(open) => !open && setPendingDelete(null)}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>删除账号</DialogTitle>
            <DialogDescription>
              确定删除账号「{pendingDelete?.name || pendingDelete?.userId}」？登录态快照会保留，
              可在「设置 → 登录态快照」中单独清理。此操作不可撤销。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setPendingDelete(null)}>
              取消
            </Button>
            <Button
              variant="destructive"
              disabled={busy?.startsWith("delete-")}
              onClick={() => {
                const target = pendingDelete;
                if (!target) return;
                setPendingDelete(null);
                void run(`delete-${target.userId}`, "删除账号", () =>
                  api.traeDeleteAccount(target.userId, false, variant),
                );
              }}
            >
              删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/**
 * 空态卡：与 WorkBuddy 的 `EmptyRegionCard` 同构（可能原因 + 期望产物 + 动作）。
 *
 * ## 文案必须说实话：旧版说「登录一次即可自动识别」，那是错的
 *
 * 这张卡原先写着「Cloud-IDE-JWT 存放在该目录下，登录一次 Trae 后即可自动识别」，
 * 并把「从本机导入」作为**主**按钮。但实测 Trae 1.107.1 已改为**加密存储**凭据
 * （`storage.json` / `state.vscdb` / `Local Storage\leveldb` 里都搜不到明文
 * `Cloud-IDE-JWT`），本机导入在这条产品线上**注定失败**。
 *
 * 于是用户看到的正是那句文案：明明已经登录，却怎么都识别不到——这不是探测 bug，
 * 是我们给了一条走不通的路还写了包票。现在把 OAuth 登录提为主入口，
 * 并把本机导入的真实适用范围讲清楚。
 */
function EmptyTraeCard({
  installed,
  dataDir,
  onRecheck,
  onImport,
  onOAuth,
  importing,
}: {
  installed: boolean;
  dataDir: string | null;
  onRecheck: () => void;
  onImport: () => void;
  onOAuth: () => void;
  importing: boolean;
}) {
  return (
    <Card className="gap-0 py-0">
      <div className="flex items-start gap-3 px-5 py-5">
        <AlertTriangle className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-medium">还没有 Trae 账号</h2>

          <div className="mt-3 text-sm text-muted-foreground">
            <p className="font-medium text-foreground/80">可能原因：</p>
            <ul className="mt-1 list-disc space-y-1 pl-5">
              <li>未安装 Trae 桌面客户端</li>
              {installed && <li>客户端已安装，但当前无可用登录态</li>}
              <li>
                <span className="text-foreground/80">客户端已登录，但凭据是加密存储的</span>
                ——Trae 1.107.x 起不再把明文 JWT 写入
                <code className="mx-1 rounded bg-muted/60 px-1 py-0.5 font-mono text-[11px]">storage.json</code>
                ，因此本机导入读不到它。这是**当前版本最常见**的原因。
              </li>
              <li>账号库为空，且尚未通过网页登录或备份文件添加过账号</li>
            </ul>
          </div>

          <div className="mt-4 rounded-lg border border-border bg-muted/30 px-3.5 py-3">
            <p className="text-sm text-foreground/80">
              <span className="font-medium">推荐做法：</span>
              用「OAuth 网页登录」在浏览器里登录一次并授权。应用会自己接住回调，
              无需查找任何本地文件，也无需粘贴令牌——这是唯一不受加密存储影响的方式。
            </p>
          </div>

          <div className="mt-4 flex flex-wrap gap-2">
            <Button size="sm" onClick={onOAuth}>
              <QrCode />
              OAuth 网页登录
            </Button>
            <Button size="sm" variant="outline" onClick={onRecheck}>
              <RefreshCw />
              重新检测
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={onImport}
              disabled={importing}
              title="仅适用于旧版客户端或凭据仍是明文的安装；1.107.x 起通常读不到"
            >
              {importing ? <Loader2 className="animate-spin" /> : <Download />}
              尝试从本机导入
            </Button>
          </div>

          <div className="mt-4">
            <p className="text-sm font-medium text-foreground/80">已探测的登录态目录：</p>
            <div className="mt-1.5 flex flex-wrap items-center gap-2">
              <code className="min-w-0 break-all rounded-md border border-border bg-muted/40 px-2 py-1 font-mono text-[11px] text-muted-foreground">
                {dataDir ? `${dataDir}\\User\\globalStorage\\storage.json` : "尚未探测到客户端数据目录"}
              </code>
            </div>
            <p className="mt-1.5 text-xs text-muted-foreground">
              该文件在新版客户端里已不再保存明文凭据；仅供确认客户端数据目录位置。
            </p>
          </div>
        </div>
      </div>
    </Card>
  );
}

/** 添加账号对话框：粘贴 JWT + 可选分组。 */
function AddAccountDialog({
  open,
  onOpenChange,
  groups,
  variant,
  onAdded,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  groups: TraeAccountsOverview["groups"];
  /** 写进哪个产品线的账号库。 */
  variant: TraeVariantId;
  onAdded: () => void;
}) {
  const [name, setName] = useState("");
  const [jwt, setJwt] = useState("");
  const [groupId, setGroupId] = useState<string>("");
  const [saving, setSaving] = useState(false);

  async function submit() {
    if (!jwt.trim()) {
      toast.error("请粘贴 JWT");
      return;
    }
    setSaving(true);
    try {
      await api.traeAddAccount(name.trim(), jwt.trim(), groupId || null, variant);
      toast.success("账号已添加");
      setName("");
      setJwt("");
      setGroupId("");
      onOpenChange(false);
      onAdded();
    } catch (e) {
      toast.error("添加失败", { description: api.asError(e) });
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>粘贴 JWT 添加账号</DialogTitle>
          <DialogDescription>
            兜底方式，仅在无法使用网页登录时使用。填 `Cloud-IDE-JWT …` 令牌
            （可从代理日志或旧版客户端的明文存储中取得）。用户名留空时按 UID 尾部自动命名。
            <br />
            <span className="text-foreground/80">
              注意：这种方式拿到的账号没有 refresh_token，JWT 过期后需要重新粘贴；
              网页登录的账号可以自动续期。
            </span>
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="trae-jwt">JWT</Label>
            <Input
              id="trae-jwt"
              value={jwt}
              onChange={(event) => setJwt(event.target.value)}
              placeholder="Cloud-IDE-JWT eyJ…"
              autoComplete="off"
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="trae-name">账号名（可选）</Label>
            <Input id="trae-name" value={name} onChange={(event) => setName(event.target.value)} placeholder="例如 主号" />
          </div>
          <div className="space-y-2">
            <Label htmlFor="trae-group">分组（可选）</Label>
            <Select value={groupId} onValueChange={setGroupId}>
              <SelectTrigger id="trae-group" className="w-full">
                <SelectValue placeholder="未分组" />
              </SelectTrigger>
              <SelectContent>
                {groups.map((group) => (
                  <SelectItem key={group.id} value={group.id}>
                    {group.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={saving}>
            取消
          </Button>
          <Button onClick={() => void submit()} disabled={saving}>
            {saving && <Loader2 className="animate-spin" />}
            添加
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
