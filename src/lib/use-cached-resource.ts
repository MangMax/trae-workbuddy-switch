import { useCallback, useEffect, useRef } from "react";

import { asError } from "@/lib/api";
import { useResourceStore, type ResourceEntry, type ResourceLoadOptions } from "@/stores/resources";

/**
 * 把一个「键 → 异步快照」接进组件。
 *
 * 语义（改造前 Trae 页面是「挂载即 `setLoading(true)` + 全套请求」）：
 *
 * | 情况 | `loading` | 界面 |
 * | --- | --- | --- |
 * | 该键**已有**快照 | `false` | 立刻渲染真数据，请求在后台跑 |
 * | 该键**没有**快照 | `true` | 走骨架屏（与改造前一致） |
 * | 取数失败且无快照 | `false` | 渲染错误提示 + 空态（不留用户干等） |
 * | 取数失败但有旧快照 | `false` | 继续显示旧数据 + 错误提示 |
 *
 * 因此「切到 WorkBuddy 再切回 TraeWork」不再闪骨架 —— 这正是本次优化要解决的问题。
 */
export interface CachedResource<T> {
  /** 快照值；`undefined` = 还没成功取到过。 */
  data: T | undefined;
  /** **仅在没有可用值**时为 true。有旧值时后台刷新不会让页面退回骨架屏。 */
  loading: boolean;
  /** 已经格式化过的错误文案（用 `api.asError`，与页面改造前的 `setError` 同源）。 */
  error: string | null;
  /** 强制取一次新值并等待完成。**变更操作之后必须调用它**，否则界面停在变更前。 */
  refresh: () => Promise<void>;
  /**
   * 就地改写快照（乐观更新，如设置项开关先落本地再回读）。
   *
   * `current` **一定**有值：调用点都在「已经渲染出表单」的位置，而表单只在
   * 快照就绪后才渲染。快照尚未就绪时该调用是空操作（没什么可改的，
   * 紧接着完成的那次取数会带来权威值），因此不需要调用方自己判空。
   */
  patch: (updater: (current: T) => T) => void;
}

export function useCachedResource<T>(
  key: string | null,
  loader: () => Promise<T>,
  options?: ResourceLoadOptions,
): CachedResource<T> {
  const entry = useResourceStore((s) => (key === null ? undefined : s.entries[key])) as
    | ResourceEntry<T>
    | undefined;
  const load = useResourceStore((s) => s.load);
  const patchEntry = useResourceStore((s) => s.patch);

  // loader 每次渲染都是新函数（`useCallback` 依赖里带着变体等），所以只把它放进 ref：
  // effect 的依赖必须是**键**。否则「同一把键、loader 换了个身份」会白白多发一轮请求，
  // 而键相同就意味着结果相同 —— 那正是要避免的重复加载。
  const loaderRef = useRef(loader);
  loaderRef.current = loader;
  const freshMs = options?.freshMs ?? 0;

  useEffect(() => {
    if (key === null) return;
    void load(key, () => loaderRef.current(), { freshMs });
  }, [key, load, freshMs]);

  const refresh = useCallback(async () => {
    if (key === null) return;
    await load(key, () => loaderRef.current(), { force: true });
  }, [key, load]);

  const patch = useCallback(
    (updater: (current: T) => T) => {
      if (key === null) return;
      patchEntry(key, (current) => (current === undefined ? undefined : updater(current as T)));
    },
    [key, patchEntry],
  );

  const data = entry?.value;

  return {
    data,
    // 出错且无值时不留在 loading：让页面渲染错误提示 + 空态，而不是永远转圈。
    loading: key !== null && data === undefined && entry?.error == null,
    error: entry?.error == null ? null : asError(entry.error),
    refresh,
    patch,
  };
}
