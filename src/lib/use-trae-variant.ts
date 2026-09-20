import { useCallback, useMemo } from "react";
import { useSearchParams } from "react-router-dom";

import type { TraeVariantId } from "@/lib/trae-types";

/** 承载产品线变体的 URL 查询参数名。 */
const VARIANT_PARAM = "line";

/**
 * 当前正在管理的 Trae 产品线（由 URL 承载）。
 *
 * ## 为什么放在 URL 而不是 React state / 全局 store
 *
 * 侧栏的「Trae Work」与「Trae CN」是两个平级分区，但**共用同一组路由**
 * （`/trae/accounts` 等）。若把变体放在全局 state 里，会出现三个问题：
 *
 * 1. **刷新即丢失**：用户切到 Trae CN 后刷新页面，会莫名其妙回到 Trae Work；
 * 2. **不可分享**：无法把「Trae CN 的账号页」链接发给别人（桌面端用得少，
 *    webui 场景下这条很实际）；
 * 3. **前进/后退对不上**：浏览器后退会改 URL 但不改变量，两者产生分歧。
 *
 * 放进查询串后，变体成为**路由状态的一部分**，上述三条自然消解。
 *
 * ## 取值别名
 *
 * 也接受 `trae-cn` / `trae-work` / `cn` / `work` 等常见写法，避免用户手改 URL
 * 或外部链接写成连字符形式时静默落回默认值。**未知值一律回落默认变体**
 * （与 Rust 侧 `parse_variant_param` 的宽容规则一致）。
 */
export function useTraeVariant(): [TraeVariantId, (next: TraeVariantId) => void] {
  const [params, setParams] = useSearchParams();

  const variant = useMemo<TraeVariantId>(() => {
    const raw = (params.get(VARIANT_PARAM) ?? "").trim().toLowerCase().replace(/-/g, "_");
    if (raw === "trae_cn" || raw === "cn") return "trae_cn";
    // `trae_work` / `work` / 空 / 未知 —— 全部回落默认变体。
    return "trae_work";
  }, [params]);

  const setVariant = useCallback(
    (next: TraeVariantId) => {
      setParams(
        (prev) => {
          const draft = new URLSearchParams(prev);
          if (next === "trae_work") {
            // 默认变体**不写进 URL**：让「默认」与「显式指定」产生同一个 URL，
            // 避免两条看起来不同的链接其实等价、也避免默认值污染可读性。
            draft.delete(VARIANT_PARAM);
          } else {
            draft.set(VARIANT_PARAM, next);
          }
          return draft;
        },
        { replace: true },
      );
    },
    [setParams],
  );

  return [variant, setVariant];
}
