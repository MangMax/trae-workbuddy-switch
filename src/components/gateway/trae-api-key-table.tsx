import { useCallback, useEffect, useState } from "react";
import { KeyRound, Loader2, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { DemoAction } from "@/components/demo-action";
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
import * as api from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import { traeRegionLabelOf } from "@/lib/trae-types";
import type { TraeApiKeyRecord, TraeVariantId } from "@/lib/trae-types";
import { cn } from "@/lib/utils";

/**
 * 归属取值集合 = **两个区域**（国内版 / 国际版）。
 *
 * ⚠️ 2026-09-21 由「产品线」改为「区域」，这不是文案调整：账号库按区域分家之后，
 * 网关的账号池本来就是一个区域一个（`pool.rs::sync_for` → `entries_for_region`），
 * 而国内两个程序位读到的是**同一本**库。继续把两个国内程序位列为两个选项，会得到
 * 「两把 Key 走同一个池、却记着不同归属」，且**根本建不出国际版 Key** ——
 * Token 统计的「国际版」档因此恒为空。选项集合必须与后端的分区维度一致。
 */
const VARIANTS: TraeVariantId[] = ["cn", "global"];

function formatDate(ts: number): string {
  if (!ts) return "—";
  const date = new Date(ts);
  if (Number.isNaN(date.getTime())) return "—";
  return `${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")} ${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

/**
 * Trae 多 Key 列表 + 创建 / 吊销 / 删除（对齐 WorkBuddy `gateway/api-key-table.tsx` 骨架）。
 *
 * ## 与 WorkBuddy 版的两处刻意差异
 *
 * 1. **「归属版本」列的取值是区域**（国内版 / 国际版），与 WorkBuddy 的 region 同义：
 *    Key 绑定 `variant`，决定它走**哪个区域的账号池**。展示走 `traeVariantLabel()`
 *    （与 Rust `TraeRegion::display_name()` 同源）。传 `"cn"` 与传 `"trae_work"`
 *    都会落进国内库（后端把区域标识解析到该区域主程序），但**下拉只列区域**：
 *    程序位是「写进哪个客户端」的执行轴，不决定 Key 能用哪些账号。
 * 2. **不搬 `Region` / `useGatewayStore`**：那是 WorkBuddy 网关的 store 耦合。
 *    本组件自持数据（无 store），列表直接调 `list_trae_api_keys`。
 *
 * 明文只在创建时一次性返回（`create_trae_api_key`），列表接口永远只给脱敏前缀。
 */
export function TraeApiKeyTable({
  className,
  /** 创建默认归属的**区域**（由页面传入当前 `?line=`，缺省 `cn` = 国内版）。 */
  defaultVariant = "cn",
  /** 数据变化（创建 / 吊销 / 删除）后的回调，供页面刷新网关状态里的 Key 前缀。 */
  onChanged,
}: {
  className?: string;
  defaultVariant?: TraeVariantId;
  onChanged?: () => void;
}) {
  const [keys, setKeys] = useState<TraeApiKeyRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [createOpen, setCreateOpen] = useState(false);
  const [name, setName] = useState("");
  const [variant, setVariant] = useState<TraeVariantId>(defaultVariant);
  const [creating, setCreating] = useState(false);
  const [plaintext, setPlaintext] = useState<{ value: string; name: string } | null>(null);
  const [revokeTarget, setRevokeTarget] = useState<TraeApiKeyRecord | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<TraeApiKeyRecord | null>(null);
  const [busy, setBusy] = useState(false);

  /** 拉取列表。创建 / 吊销 / 删除后都重新拉，保证列表与后端一致。 */
  const load = useCallback(async () => {
    setLoading(true);
    try {
      const result = await api.listTraeApiKeys();
      setKeys(result.keys ?? []);
    } catch (e) {
      // 演示模式 / 后端不可用：保持空列表，不清空已有数据。
      toast.error("读取 API Key 列表失败", { description: api.asError(e) });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  function openCreate() {
    setName("");
    setVariant(defaultVariant);
    setCreateOpen(true);
  }

  async function onCreate() {
    const trimmed = name.trim();
    if (!trimmed) {
      toast.error("请填写名称");
      return;
    }
    setCreating(true);
    try {
      const result = await api.createTraeApiKey(trimmed, variant);
      const value = result.key;
      if (!value) {
        // 创建成功但没回明文 = 契约异常，必须显式提示而不是静默（明文不可复原）。
        toast.error("创建成功但未返回明文，请重试");
        return;
      }
      setCreateOpen(false);
      setPlaintext({ value, name: trimmed });
      await load();
      onChanged?.();
    } catch (e) {
      toast.error("创建失败", { description: api.asError(e) });
    } finally {
      setCreating(false);
    }
  }

  async function confirmRevoke() {
    if (!revokeTarget) return;
    setBusy(true);
    try {
      await api.revokeTraeApiKey(revokeTarget.id);
      toast.success("已吊销", { description: revokeTarget.name });
      setRevokeTarget(null);
      await load();
      onChanged?.();
    } catch (e) {
      toast.error("吊销失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    setBusy(true);
    try {
      await api.deleteTraeApiKey(deleteTarget.id);
      toast.success("已删除", { description: deleteTarget.name });
      setDeleteTarget(null);
      await load();
      onChanged?.();
    } catch (e) {
      toast.error("删除失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card className={cn("gap-0 py-0", className)}>
      <div className="flex items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
        <span className="text-sm font-semibold">API Key</span>
        <DemoAction>
          <Button size="sm" onClick={openCreate}>
            <KeyRound />
            创建 API Key
          </Button>
        </DemoAction>
      </div>

      <div className="px-5 py-3">
        {loading && keys.length === 0 ? (
          <p className="py-4 text-center text-sm text-muted-foreground">
            <Loader2 className="mr-1.5 inline size-3.5 animate-spin" />
            读取中…
          </p>
        ) : keys.length === 0 ? (
          <p className="py-4 text-center text-sm text-muted-foreground">尚未创建 API Key。</p>
        ) : (
          <div className="min-w-0 overflow-x-auto">
            <table className="w-full min-w-[640px] text-left text-sm">
              <thead>
                <tr className="text-xs text-muted-foreground">
                  <th className="pb-2 pr-4 font-medium">名称</th>
                  <th className="pb-2 pr-4 font-medium">归属版本</th>
                  <th className="pb-2 pr-4 font-medium">前缀</th>
                  <th className="pb-2 pr-4 font-medium">创建时间</th>
                  <th className="pb-2 pr-4 font-medium">最近使用</th>
                  <th className="pb-2 pr-4 font-medium">状态</th>
                  <th className="pb-2 font-medium">操作</th>
                </tr>
              </thead>
              <tbody>
                {keys.map((key) => {
                  const revoked = key.revoked;
                  return (
                    <tr key={key.id} className="border-t border-border/60">
                      <td className="py-2 pr-4 font-medium">{key.name}</td>
                      <td className="py-2 pr-4">
                        <Badge variant="secondary" className="rounded-md">
                          {traeRegionLabelOf(key.variant)}
                        </Badge>
                      </td>
                      <td className="py-2 pr-4 font-mono text-xs text-muted-foreground">{key.prefix}…</td>
                      <td className="py-2 pr-4 text-xs text-muted-foreground">{formatDate(key.createdAt)}</td>
                      <td className="py-2 pr-4 text-xs text-muted-foreground">
                        {key.lastUsedAt ? formatDate(key.lastUsedAt) : "从未使用"}
                      </td>
                      <td className="py-2 pr-4">
                        {revoked ? (
                          <Badge variant="secondary" className="rounded-md text-muted-foreground">
                            已吊销
                          </Badge>
                        ) : (
                          <Badge variant="success" className="rounded-md">
                            启用
                          </Badge>
                        )}
                      </td>
                      <td className="py-2">
                        {revoked ? (
                          <Button
                            variant="ghost"
                            size="sm"
                            className="text-destructive hover:text-destructive"
                            onClick={() => setDeleteTarget(key)}
                          >
                            <Trash2 />
                            删除
                          </Button>
                        ) : (
                          <Button variant="ghost" size="sm" onClick={() => setRevokeTarget(key)}>
                            吊销
                          </Button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* 创建对话框 */}
      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>创建 API Key</DialogTitle>
            <DialogDescription>每个 Key 只能访问其归属版本的模型与账号池。</DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="trae-key-name">名称</Label>
              <Input
                id="trae-key-name"
                value={name}
                onChange={(event) => setName(event.target.value)}
                placeholder="例如 Cursor"
                spellCheck={false}
                autoComplete="off"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="trae-key-variant">归属版本</Label>
              <Select value={variant} onValueChange={(value) => setVariant(value as TraeVariantId)}>
                <SelectTrigger id="trae-key-variant" className="w-full" aria-label="归属版本">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {VARIANTS.map((item) => (
                    <SelectItem key={item} value={item}>
                      {traeRegionLabelOf(item)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setCreateOpen(false)} disabled={creating}>
              取消
            </Button>
            <Button onClick={() => void onCreate()} disabled={creating}>
              {creating && <Loader2 className="animate-spin" />}
              创建
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 一次性明文展示 */}
      <Dialog open={plaintext !== null} onOpenChange={(open) => !open && setPlaintext(null)}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>API Key 已创建</DialogTitle>
            <DialogDescription>完整 Key 只显示这一次，请立即复制保存。</DialogDescription>
          </DialogHeader>
          {plaintext && (
            <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2.5">
              <code className="min-w-0 flex-1 break-all font-mono text-xs">{plaintext.value}</code>
              <Button
                variant="outline"
                size="sm"
                onClick={() => void copyText(plaintext.value, "API Key 已复制")}
              >
                复制
              </Button>
            </div>
          )}
          <DialogFooter>
            <Button onClick={() => setPlaintext(null)}>我已保存，关闭</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 吊销确认 */}
      <Dialog open={revokeTarget !== null} onOpenChange={(open) => !open && setRevokeTarget(null)}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>吊销 API Key</DialogTitle>
            <DialogDescription>
              吊销后「{revokeTarget?.name}」立即失效（401），列表中保留为「已吊销」状态。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setRevokeTarget(null)} disabled={busy}>
              取消
            </Button>
            <Button variant="destructive" onClick={() => void confirmRevoke()} disabled={busy}>
              吊销
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 删除确认 */}
      <Dialog open={deleteTarget !== null} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>删除 API Key</DialogTitle>
            <DialogDescription>确定删除已吊销的「{deleteTarget?.name}」？此操作不可撤销。</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDeleteTarget(null)} disabled={busy}>
              取消
            </Button>
            <Button variant="destructive" onClick={() => void confirmDelete()} disabled={busy}>
              删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </Card>
  );
}
