import * as api from "@/lib/api";
import type { TraeProgramStatus, TraeRegionId, TraeVariantId, TraeVariantStatus } from "@/lib/trae-types";

/** 造一个「状态未知」的程序位占位。 */
function programStub(
  program: TraeProgramStatus["program"],
  label: string,
  nameAlias: string,
  variant: TraeVariantId | null,
): TraeProgramStatus {
  return {
    program,
    label,
    nameAlias,
    variant,
    installed: false,
    running: false,
    version: null,
    path: null,
    dataDir: null,
    dataDirExists: false,
  };
}

/**
 * 探测不到任何区域时仍列出的**两个区域**（状态未知的占位）。
 *
 * ⚠️ **只此一份**：账号页的区域切换条与账号卡片都要遍历「有哪些区域/程序位」，
 * 各自维护一份占位表迟早漂移（曾经由 `TraeVariantBar` 私有持有）。
 *
 * 程序位的 `variant` 是**切换时要回传给后端的标识**：
 * 国内两个程序位有各自的历史标识（`trae_work` / `trae_cn`，后端据此选客户端），
 * 国际版 TraeCode 尚未建模 ⇒ `null`（按钮必须禁用，不能拿同区域另一个客户端顶替）。
 */
export const TRAE_VARIANT_FALLBACK: TraeVariantStatus[] = [
  {
    variant: "cn",
    variantLabel: "国内版",
    installed: false,
    running: false,
    version: null,
    path: null,
    dataDir: null,
    dataDirExists: false,
    programs: [
      programStub("trae_work", "TraeWork", "TraeWork CN", "trae_work"),
      programStub("trae_code", "TraeCode", "TraeCode CN", "trae_cn"),
    ],
  },
  {
    variant: "global",
    variantLabel: "国际版",
    installed: false,
    running: false,
    version: null,
    path: null,
    dataDir: null,
    dataDirExists: false,
    programs: [
      programStub("trae_work", "TraeWork AI", "TraeWork", "global"),
      programStub("trae_code", "Trae AI", "TraeCode（待实测）", null),
    ],
  },
];

/** 程序位标识 → 该程序当前登录账号（无 / 读不到时为 `null`）。 */
export type TraeVariantLogins = Partial<Record<TraeVariantId, string | null>>;

/**
 * 读取**全部区域**的环境状态（并排视角）。
 *
 * 返回**永不为空**：探测命令不可用（演示模式、后端未起来）或返回空列表时，
 * 回落到 {@link TRAE_VARIANT_FALLBACK} 的两个区域占位 —— 调用方据此渲染
 * 「有哪些区域」的控件，选项必须在后端不可用时依然存在，
 * 否则用户连切回另一条线的入口都没有。
 */
export async function loadTraeVariantStatuses(): Promise<TraeVariantStatus[]> {
  try {
    const result = await api.getTraeVariants();
    return result.variants?.length ? result.variants : TRAE_VARIANT_FALLBACK;
  } catch {
    return TRAE_VARIANT_FALLBACK;
  }
}

/**
 * 读取**每个程序位各自的**当前登录账号（`profiles.currentAccount`）。
 *
 * ## 为什么遍历的是**程序位**而不是区域
 *
 * 登录态快照是**客户端级**的（它只能恢复到采集它的那个客户端里去），因此
 * 「当前账号」天然是**每个程序位一条**。区域级那个值只是界面汇总用的展示，
 * 不能拿来判断某个账号是否"当前账号"。
 *
 * `statuses` 由调用方传入而不是本函数自己探测：调用方（账号页）本来就要用
 * 同一份 `statuses` 去渲染控件，再探一次会得到第二个可能不一致的快照。
 *
 * **单个程序位失败不影响其余**：读不到只记 `null`（视为「没有当前账号」），
 * 不抛错 —— 卡片上少一枚「当前账号」角标，远好过整页报错。
 */
export async function loadTraeVariantLogins(
  statuses: TraeVariantStatus[],
): Promise<TraeVariantLogins> {
  const targets = statuses.flatMap((item) =>
    item.programs
      .map((program) => program.variant)
      .filter((variant): variant is TraeVariantId => variant !== null),
  );
  const pairs = await Promise.all(
    targets.map(async (variant): Promise<[TraeVariantId, string | null]> => {
      try {
        const profiles = await api.getTraeProfiles(variant);
        return [variant, profiles.currentAccount ?? null];
      } catch {
        return [variant, null];
      }
    }),
  );
  return Object.fromEntries(pairs) as TraeVariantLogins;
}

/** 取某个区域的条目（找不到时返回 `undefined`，调用方自行回落占位）。 */
export function findRegionStatus(
  statuses: TraeVariantStatus[],
  region: TraeRegionId,
): TraeVariantStatus | undefined {
  return statuses.find((item) => item.variant === region);
}
