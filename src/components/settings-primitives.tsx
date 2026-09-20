import type { ReactElement, ReactNode } from "react";

import { DemoAction } from "@/components/demo-action";
import { Card } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";

/**
 * 设置页的三个行原语（分组 / 普通行 / 字段行）。
 *
 * ## 为什么要抽出来
 *
 * 这两个产品的设置页要求「视觉上无法区分」，而本模块原先在
 * `SettingsPage.tsx` 与 `TraeSettingsPage.tsx` 里各有一份**逐字重复**的实现。
 * 重复的代价不是多敲几行，而是**漂移**：Trae 那份当时已经漏掉了 `operational`
 * 参数，于是同一个「需要演示模式遮罩的开关行」在两个页面表现不同。
 * 抽成共享模块后，两边只可能是同一套间距与断点。
 *
 * ## 边界
 *
 * 这里只放**纯版式**原语，不含任何数据获取与业务判断 —— 两个设置页的内容
 * 差异极大（Trae 有客户端探测/设备标识/平台能力，WorkBuddy 有外观/权限/更新），
 * 强行合并内容只会得到一堆 `if (product === …)`。骨架统一、内容各自实现。
 */

interface SettingsGroupProps {
  id: string;
  title: string;
  children: ReactNode;
}

/** 设置分组：标题贴左 + 圆角卡片。 */
export function SettingsGroup({ id, title, children }: SettingsGroupProps) {
  return (
    <section className="min-w-0 space-y-2.5" aria-labelledby={id}>
      <div className="px-1">
        <h2 id={id} className="text-[13px] font-medium leading-5">
          {title}
        </h2>
      </div>
      <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">{children}</Card>
    </section>
  );
}

/** 设置行：左右两栏的通用容器（无标签结构时用）。 */
export function SettingsRow({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div
      className={cn(
        "mx-4 flex min-w-0 items-center justify-between gap-3 border-b border-border/50 px-0 py-2.5 sm:mx-5",
        className,
      )}
    >
      {children}
    </div>
  );
}

interface SettingsFieldRowProps {
  label: ReactNode;
  description?: ReactNode;
  htmlFor?: string;
  children: ReactNode;
  className?: string;
  /**
   * 该行的控件是「有副作用的操作」（需要演示模式遮罩）时置 true。
   *
   * 用参数而不是让调用方自己包 `<DemoAction>`：遮罩必须与行的右栏宽度绑定
   * （`w-full sm:w-auto`），调用方自己包极容易漏掉宽度而让按钮在窄屏溢出。
   */
  operational?: boolean;
}

/** 设置字段行：左标签（可带说明）+ 右控件。 */
export function SettingsFieldRow({
  label,
  description,
  htmlFor,
  children,
  className,
  operational = false,
}: SettingsFieldRowProps) {
  return (
    <SettingsRow className={cn("flex-col items-stretch gap-2 sm:flex-row sm:items-center", className)}>
      <div className="min-w-0 flex-1">
        {htmlFor ? (
          <Label htmlFor={htmlFor} className="text-[13px] leading-4">
            {label}
          </Label>
        ) : (
          <div className="text-[13px] font-medium leading-4">{label}</div>
        )}
        {description && (
          <p className="mt-0.5 text-xs leading-4 text-muted-foreground/75">{description}</p>
        )}
      </div>
      <div className="flex min-w-0 w-full shrink-0 justify-end sm:w-auto">
        {operational ? <DemoAction className="w-full sm:w-auto">{children as ReactElement}</DemoAction> : children}
      </div>
    </SettingsRow>
  );
}
