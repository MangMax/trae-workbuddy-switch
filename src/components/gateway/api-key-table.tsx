import { useState } from "react";
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
import { REGIONS, regionDescriptor } from "@/lib/region";
import { cn } from "@/lib/utils";
import type { ApiKeyRecord, Region } from "@/lib/types";
import { useGatewayStore } from "@/stores/gateway";

function formatDate(ts: number): string {
  const date = new Date(ts);
  if (Number.isNaN(date.getTime())) return "—";
  return `${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")} ${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

/** API Key 列表 + 创建对话框 + 一次性明文展示 + 吊销 / 删除（P0-3）。 */
export function ApiKeyTable({ className }: { className?: string }) {
  const keys = useGatewayStore((s) => s.keys);
  const createKey = useGatewayStore((s) => s.createKey);
  const revokeKey = useGatewayStore((s) => s.revokeKey);
  const deleteKey = useGatewayStore((s) => s.deleteKey);

  const [createOpen, setCreateOpen] = useState(false);
  const [name, setName] = useState("");
  const [region, setRegion] = useState<Region>("cn");
  const [creating, setCreating] = useState(false);
  const [plaintext, setPlaintext] = useState<{ value: string; name: string } | null>(null);
  const [revokeTarget, setRevokeTarget] = useState<ApiKeyRecord | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ApiKeyRecord | null>(null);
  const [busy, setBusy] = useState(false);

  function openCreate() {
    setName("");
    setRegion("cn");
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
      const result = await createKey(trimmed, region);
      const value = result.key;
      if (!value) {
        toast.error("创建成功但未返回明文，请重试");
        return;
      }
      setCreateOpen(false);
      setPlaintext({ value, name: trimmed });
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
      await revokeKey(revokeTarget.id);
      toast.success("已吊销", { description: revokeTarget.name });
      setRevokeTarget(null);
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
      await deleteKey(deleteTarget.id);
      toast.success("已删除", { description: deleteTarget.name });
      setDeleteTarget(null);
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
        {keys.length === 0 ? (
          <p className="py-4 text-center text-sm text-muted-foreground">尚未创建 API Key。</p>
        ) : (
          <div className="min-w-0 overflow-x-auto">
            <table className="w-full min-w-[560px] text-left text-sm">
              <thead>
                <tr className="text-xs text-muted-foreground">
                  <th className="pb-2 pr-4 font-medium">名称</th>
                  <th className="pb-2 pr-4 font-medium">版本</th>
                  <th className="pb-2 pr-4 font-medium">前缀</th>
                  <th className="pb-2 pr-4 font-medium">创建时间</th>
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
                          {regionDescriptor(key.region).versionLabel}
                        </Badge>
                      </td>
                      <td className="py-2 pr-4 font-mono text-xs text-muted-foreground">{key.prefix}…</td>
                      <td className="py-2 pr-4 text-xs text-muted-foreground">{formatDate(key.createdAt)}</td>
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
                          <Button variant="ghost" size="sm" className="text-destructive hover:text-destructive" onClick={() => setDeleteTarget(key)}>
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
            <DialogDescription>每个 Key 只能访问其归属版本的模型与账号。</DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="key-name">名称</Label>
              <Input
                id="key-name"
                value={name}
                onChange={(event) => setName(event.target.value)}
                placeholder="例如 Cursor"
                spellCheck={false}
                autoComplete="off"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="key-region">归属版本</Label>
              <Select value={region} onValueChange={(value) => setRegion(value as Region)}>
                <SelectTrigger id="key-region" className="w-full" aria-label="归属版本">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {REGIONS.map((r) => (
                    <SelectItem key={r} value={r}>
                      {regionDescriptor(r).gatewayLabel}
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
              <Button variant="outline" size="sm" onClick={() => void copyText(plaintext.value, "API Key 已复制")}>
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
