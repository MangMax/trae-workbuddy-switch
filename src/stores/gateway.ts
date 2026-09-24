import { create } from "zustand";

import * as api from "@/lib/api";
import {
  DEFAULT_GATEWAY_CONFIG,
  normalizeGatewayConfig,
  normalizeGatewayStatus,
} from "@/lib/gateway";
import type {
  AccountStrategy,
  AccountStrategyView,
  ApiKeyRecord,
  CatalogSnapshot,
  CreateApiKeyResult,
  GatewayConfig,
  GatewayLogEntry,
  GatewayStatus,
  Region,
} from "@/lib/types";

/** 网关配置缺省值（与后端 GatewayConfig::default 对齐）。 */
export { DEFAULT_GATEWAY_CONFIG };

function defaultStrategyView(region?: Region): AccountStrategyView {
  return {
    region,
    strategy: { kind: "current" },
    selected: null,
  };
}

function defaultStrategies(): Record<Region, AccountStrategyView> {
  return { cn: defaultStrategyView("cn"), global: defaultStrategyView("global") };
}

/** 兼容后端可能缺字段：始终补齐两个 region。 */
function normalizeStrategies(input: Partial<Record<Region, AccountStrategyView>> | null | undefined): Record<Region, AccountStrategyView> {
  return {
    cn: input?.cn ? { ...input.cn, region: input.cn.region ?? "cn" } : defaultStrategyView("cn"),
    global: input?.global ? { ...input.global, region: input.global.region ?? "global" } : defaultStrategyView("global"),
  };
}

interface GatewayState {
  config: GatewayConfig;
  status: GatewayStatus | null;
  keys: ApiKeyRecord[];
  models: Record<Region, CatalogSnapshot | null>;
  strategies: Record<Region, AccountStrategyView>;
  logs: GatewayLogEntry[];
  loading: boolean;
  saving: boolean;
  error: string | null;
  lastLoadedAt: number;

  loadConfig: () => Promise<void>;
  /** 返回监听启动失败原因；null 表示配置保存且监听正常（配置保存失败时直接 throw）。 */
  saveConfig: (config: GatewayConfig) => Promise<string | null>;
  refreshStatus: () => Promise<void>;
  loadKeys: () => Promise<void>;
  createKey: (name: string, region: Region) => Promise<CreateApiKeyResult>;
  revokeKey: (id: string) => Promise<void>;
  deleteKey: (id: string) => Promise<void>;
  loadModels: (region: Region) => Promise<void>;
  refreshModels: (region: Region) => Promise<void>;
  loadStrategies: () => Promise<void>;
  saveStrategy: (region: Region, strategy: AccountStrategy) => Promise<void>;
  loadLogs: () => Promise<void>;
  clearLogs: () => Promise<void>;
  /** 首次进入页面：并发拉取配置 / 状态 / Keys / 策略 / 日志 / 两版模型。 */
  loadAll: () => Promise<void>;
}

export const useGatewayStore = create<GatewayState>((set, get) => ({
  config: DEFAULT_GATEWAY_CONFIG,
  status: null,
  keys: [],
  models: { cn: null, global: null },
  strategies: defaultStrategies(),
  logs: [],
  loading: false,
  saving: false,
  error: null,
  lastLoadedAt: 0,

  async loadConfig() {
    try {
      const config = await api.getGatewayConfig();
      set({ config: normalizeGatewayConfig(config) });
    } catch (e) {
      set({ error: api.asError(e) });
    }
  },

  async saveConfig(config) {
    set({ saving: true });
    try {
      const result = await api.saveGatewayConfig(config);
      set({ config: normalizeGatewayConfig(result.config), error: null });
      await get().refreshStatus();
      // 配置已保存但监听重启失败：由调用方（设置页）以 warning 呈现原因。
      return result.listen_error;
    } finally {
      set({ saving: false });
    }
  },

  async refreshStatus() {
    try {
      set({ status: normalizeGatewayStatus(await api.gatewayStatus()) });
    } catch (e) {
      set({ status: null, error: api.asError(e) });
    }
  },

  async loadKeys() {
    try {
      const { keys } = await api.listApiKeys();
      set({ keys: keys ?? [] });
    } catch (e) {
      set({ error: api.asError(e) });
    }
  },

  async createKey(name, region) {
    const result = await api.createApiKey(name, region);
    if (result.record) {
      set((s) => ({ keys: [result.record!, ...s.keys.filter((key) => key.id !== result.record!.id)] }));
    } else {
      await get().loadKeys();
    }
    return result;
  },

  async revokeKey(id) {
    await api.revokeApiKey(id);
    await get().loadKeys();
  },

  async deleteKey(id) {
    await api.deleteApiKey(id);
    await get().loadKeys();
  },

  async loadModels(region) {
    try {
      const snapshot = await api.getGatewayModels(region);
      set((s) => ({ models: { ...s.models, [region]: snapshot } }));
    } catch (e) {
      set({ error: api.asError(e) });
    }
  },

  async refreshModels(region) {
    const snapshot = await api.refreshGatewayModels(region);
    set((s) => ({ models: { ...s.models, [region]: snapshot } }));
  },

  async loadStrategies() {
    try {
      const map = await api.getAccountStrategy();
      set({ strategies: normalizeStrategies(map) });
    } catch (e) {
      set({ error: api.asError(e) });
    }
  },

  async saveStrategy(region, strategy) {
    await api.saveAccountStrategy(region, strategy);
    await get().loadStrategies();
  },

  async loadLogs() {
    try {
      const { logs } = await api.getGatewayLogs();
      set({ logs: [...(logs ?? [])].sort((a, b) => b.ts - a.ts) });
    } catch (e) {
      set({ error: api.asError(e) });
    }
  },

  async clearLogs() {
    await api.clearGatewayLogs();
    set({ logs: [] });
  },

  async loadAll() {
    set({ loading: true, error: null });
    try {
      await Promise.all([
        get().loadConfig(),
        get().refreshStatus(),
        get().loadKeys(),
        get().loadStrategies(),
        get().loadLogs(),
        get().loadModels("cn"),
        get().loadModels("global"),
      ]);
      set({ lastLoadedAt: Date.now() });
    } finally {
      set({ loading: false });
    }
  },
}));
