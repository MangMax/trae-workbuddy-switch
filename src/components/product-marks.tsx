import { cn } from "@/lib/utils";
import workbuddyIcon from "@/assets/workbuddy-official-icon.png";
import codebuddyCnIdeIcon from "@/assets/codebuddy-cn-ide-icon.png";
import traeLogo from "@/assets/trae-logo.webp";

const appIconUrl = `${import.meta.env.BASE_URL}icon-transparent.png`;

interface MarkProps {
  size?: number;
  className?: string;
}

/**
 * WorkBuddy 官方应用图标（从 WorkBuddy.app 的 icon.icns 提取）。
 * 与 CodeBuddy IDE 图标同为标准 macOS app icon 风格（约 10% 透明边距），
 * 放大 118% 居中裁掉透明圈后与 CodeBuddy 系列图标视觉一致。
 */
export function WorkBuddyMark({ size = 32, className }: MarkProps) {
  return (
    <span
      aria-hidden
      className={cn("relative inline-flex shrink-0 overflow-hidden rounded-[22%]", className)}
      style={{ width: size, height: size }}
    >
      <img
        src={workbuddyIcon}
        alt=""
        className="absolute left-1/2 top-1/2 size-[118%] max-w-none -translate-x-1/2 -translate-y-1/2 object-cover"
      />
    </span>
  );
}

/** 应用自身的透明角色图标；桌面安装图标仍使用 public/icon.png。 */
export function AppIconMark({ size = 32, className }: MarkProps) {
  return (
    <span
      aria-hidden
      className={cn("inline-flex shrink-0", className)}
      style={{ width: size, height: size }}
    >
      <img src={appIconUrl} alt="" className="size-full object-contain" />
    </span>
  );
}

export function CodeBuddyMark({ size = 32, className }: MarkProps) {
  const icon = Math.max(10, Math.round(size));
  return (
    <span
      aria-hidden
      className={cn(
        "inline-flex shrink-0 items-center justify-center rounded-[22%] border border-white/10 bg-zinc-950 text-zinc-50 shadow-sm",
        className,
      )}
      style={{ width: size, height: size, fontSize: icon }}
    >
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2.2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="size-[1em]"
      >
        <path d="M4.4 7.4 10.4 12 4.4 16.6" />
        <path d="M13 16.6h7" />
      </svg>
    </span>
  );
}

/**
 * CodeBuddy IDE（桌面客户端）官方应用图标。
 * 源图四周自带约 9% 透明边距：正方形图 + object-cover 不会触发任何缩放，
 * 必须先把图放大到 122% 再居中裁剪，才能把透明圈裁掉并与 WorkBuddy 的
 * 全幅 logo 达到同样的视觉大小（裁剪仅落在透明边距上，几乎不伤画面）。
 */
export function CodeBuddyCnIdeMark({ size = 32, className }: MarkProps) {
  return (
    <span
      aria-hidden
      className={cn("relative inline-flex shrink-0 overflow-hidden", className)}
      style={{ width: size, height: size }}
    >
      <img
        src={codebuddyCnIdeIcon}
        alt=""
        className="absolute left-1/2 top-1/2 size-[122%] max-w-none -translate-x-1/2 -translate-y-1/2 object-cover"
      />
    </span>
  );
}

/**
 * Trae 客户端图标。
 *
 * 源图是**白色单色**标志（透明底），直接贴在浅色侧栏上会看不见，
 * 因此与 `CodeBuddyMark` 采用同一手法：套一个深色圆角方块作为承托底色，
 * 两种主题下都有足够对比度，也和同排其他产品图标保持一致的视觉体量。
 */
export function TraeMark({ size = 32, className }: MarkProps) {
  return (
    <span
      aria-hidden
      className={cn(
        "inline-flex shrink-0 items-center justify-center rounded-[22%] border border-white/10 bg-zinc-950 shadow-sm",
        className,
      )}
      style={{ width: size, height: size }}
    >
      <img src={traeLogo} alt="" className="size-[78%] object-contain" />
    </span>
  );
}

export function StatusDot({ on, className }: { on: boolean; className?: string }) {
  return (
    <span
      aria-hidden
      className={cn("size-1.5 shrink-0 rounded-full", on ? "bg-primary" : "bg-muted-foreground/35", className)}
    />
  );
}

/**
 * 产品线变体标识（与 Rust 侧 `TraeVariant::as_str()` 逐字一致）。
 *
 * 刻意**不复用** `TraeVariantId` 的导入：本文件是纯展示组件，
 * 不依赖任何业务类型模块，避免「改一个类型定义要连带动画图标组件」。
 */
type TraeVariantKey = "trae_work" | "trae_cn";

/**
 * Trae 产品线图标（按变体区分）。
 *
 * ## 为什么两条产品线共用一个 logo 图案
 *
 * 墙上没有第二张 Trae 素材：`nameAlias` 显示 `TRAE SOLO CN` 自称 `TraeWork CN`、
 * `Trae CN` 自称 `TraeCode CN`，**两者是同一个品牌的两条产品线**，官方给的就是
 * 同一个 Trae 标志。因此区分靠**承托底色 + 角标文字**，而不是伪造第二个 logo：
 *
 * - `trae_work`：深色承托块（与侧栏 Tab 里的 `TraeMark` 完全一致）
 * - `trae_cn`  ：同图案 + 承托块右下角一枚小字角标，让两者并排时一眼可辨
 *
 * 若将来拿到官方区分的素材，只需替换这里的 `src`，调用方不必改。
 */
export function TraeVariantMark({
  variant,
  size = 32,
  className,
}: MarkProps & { variant: TraeVariantKey }) {
  return (
    <span
      aria-hidden
      className={cn(
        "relative inline-flex shrink-0 items-center justify-center rounded-[22%] border border-white/10 bg-zinc-950 shadow-sm",
        className,
      )}
      style={{ width: size, height: size }}
    >
      <img src={traeLogo} alt="" className="size-[78%] object-contain" />
      {variant === "trae_cn" && (
        // 角标是**装饰性的**（`aria-hidden` 由外层承担），
        // 真正可读的信息在调用方的 tooltip 文案里。
        <span
          className="absolute -bottom-px -right-px rounded-[3px] bg-primary px-[2px] font-semibold leading-[1.4] text-primary-foreground"
          style={{ fontSize: Math.max(7, Math.round(size * 0.3)) }}
        >
          CN
        </span>
      )}
    </span>
  );
}
