import { useCallback, useState } from "react";

/**
 * 账号列表的「紧凑模式」偏好。
 *
 * WorkBuddy 与 Trae 两个产品分区共用同一个键：它是**用户看表的习惯**，不是产品属性，
 * 在两个 Tab 之间来回切换时不应该来回跳。读取失败（隐私模式等）时退回默认的紧凑模式。
 */
const STORAGE_KEY = "buddy-switch.compact";

export function useCompactMode(): [boolean, () => void] {
  const [compact, setCompact] = useState<boolean>(() => {
    try {
      return localStorage.getItem(STORAGE_KEY) !== "0";
    } catch {
      return true;
    }
  });

  const toggle = useCallback(() => {
    setCompact((value) => {
      const next = !value;
      try {
        localStorage.setItem(STORAGE_KEY, next ? "1" : "0");
      } catch {
        /* 存储不可用时静默 */
      }
      return next;
    });
  }, []);

  return [compact, toggle];
}
