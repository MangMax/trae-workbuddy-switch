import { Cpu } from "lucide-react";

import { Card } from "@/components/ui/card";
import type { TraeGatewayModel } from "@/lib/trae-types";
import { cn } from "@/lib/utils";

/**
 * Trae 模型清单（对齐 WorkBuddy `gateway/model-list.tsx` 骨架）。
 *
 * ## 为什么**没有**「刷新」按钮（用户已裁定）
 *
 * WorkBuddy 的模型清单来自上游探测（`refreshModels` 会真的打一次上游接口），
 * 所以有刷新按钮。Trae 侧**不发上游探测**——模型名是客户端侧常量
 * （见 `crates/buddy-switch-gateway/src/trae/routes.rs`：`/v1/models` 直接回静态清单），
 * 因此「刷新」永远不可能改变结果。一个点了不会改变任何可观察结果的按钮是假控件，
 * 不加；改为一行诚实说明，讲清「为什么这里没有刷新」。
 */
export function TraeModelList({
  models,
  defaultModel,
  className,
}: {
  models: TraeGatewayModel[];
  defaultModel: string;
  className?: string;
}) {
  return (
    <Card className={cn("gap-0 py-0", className)}>
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
        <div className="flex items-center gap-2 text-sm font-semibold">
          <Cpu className="size-4 stroke-[1.75]" />
          模型清单
        </div>
        <span className="text-xs text-muted-foreground">
          共 {models.length} 个 · 默认 {defaultModel}
        </span>
      </div>
      <div className="px-5 py-4">
        <p className="mb-3 text-xs text-muted-foreground">
          Trae 模型为客户端常量，不随上游刷新；下方清单即对外暴露的全部模型。
        </p>
        {models.length === 0 ? (
          <p className="py-4 text-sm text-muted-foreground">暂无模型数据。</p>
        ) : (
          <div className="flex flex-wrap gap-2">
            {models.map((model) => (
              <code
                key={model.id}
                className={cn(
                  "rounded-md border px-2 py-1 font-mono text-xs",
                  model.id === defaultModel
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
  );
}
