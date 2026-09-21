// 网关字段归一化与派生工具（对照架构设计 A-3.4 / A-3.7）。
// 后端字段名如有微调，集中在此兼容，避免兼容逻辑散落各组件。
//
// 注意：同一 gateway 模块内两套类型序列化风格不同 ——
//   · GatewayConfig（用户配置，state.rs 定义）序列化为 snake_case，camelCase 仅作反序列化 alias；
//   · GatewayStatus / gateway_status 响应（给前端读）为 camelCase。
// 因此本文件对 Config 以 snake_case 为准、对 Status 以 camelCase 为准，并各自保留别名兜底。

import type { GatewayConfig, GatewayStatus } from "@/lib/types";

/** 网关配置缺省值（与后端 GatewayConfig::default 对齐；字段名为 snake_case）。 */
export const DEFAULT_GATEWAY_CONFIG: GatewayConfig = {
  enabled: false,
  bind_addr: "127.0.0.1",
  port: 57891,
  allow_non_loopback: false,
  log_keep: 200,
  log_bodies: false,
  per_key_rate_limit: null,
};

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function asNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function asBoolean(value: unknown): boolean | undefined {
  return typeof value === "boolean" ? value : undefined;
}

/**
 * 归一化 get_gateway_config / save_gateway_config 的返回。
 *
 * 后端 GatewayConfig 序列化为 snake_case（camelCase 仅为后端 `#[serde(alias)]`，
 * 反序列化容忍、序列化不产出）。此处 camelCase 兜底属防御性，正常不应被触发。
 */
export function normalizeGatewayConfig(raw: unknown): GatewayConfig {
  const record = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  return {
    enabled: asBoolean(record.enabled) ?? DEFAULT_GATEWAY_CONFIG.enabled,
    bind_addr:
      asString(record.bind_addr) ?? asString(record.bindAddr) ?? DEFAULT_GATEWAY_CONFIG.bind_addr,
    port: asNumber(record.port) ?? DEFAULT_GATEWAY_CONFIG.port,
    allow_non_loopback:
      asBoolean(record.allow_non_loopback) ??
      asBoolean(record.allowNonLoopback) ??
      DEFAULT_GATEWAY_CONFIG.allow_non_loopback,
    log_keep: asNumber(record.log_keep) ?? asNumber(record.logKeep) ?? DEFAULT_GATEWAY_CONFIG.log_keep,
    log_bodies: asBoolean(record.log_bodies) ?? asBoolean(record.logBodies) ?? DEFAULT_GATEWAY_CONFIG.log_bodies,
    per_key_rate_limit:
      asNumber(record.per_key_rate_limit) ?? asNumber(record.perKeyRateLimit) ?? null,
  };
}

/**
 * 将 gateway_status 原始响应归一化为前端规范形状。
 *
 * 后端 gateway_status（GatewayStatusView）现为 snake_case：
 * `base_url` / `bind_addr` / `allow_non_loopback`。此处以 snake_case 为准，
 * 并保留 camelCase 别名兜底（防御性，正常不应触发）。
 * 传入非法值时返回 null，调用方据此回退到配置值。
 */
export function normalizeGatewayStatus(raw: unknown): GatewayStatus | null {
  if (!raw || typeof raw !== "object") return null;
  const record = raw as Record<string, unknown>;
  const addr =
    asString(record.addr) ?? asString(record.bindAddr) ?? asString(record.bind_addr) ?? "127.0.0.1";
  const port = asNumber(record.port) ?? 57891;
  const allowNonLoopback =
    asBoolean(record.allowNonLoopback) ?? asBoolean(record.allow_non_loopback) ?? false;
  return {
    enabled: asBoolean(record.enabled) ?? false,
    running: asBoolean(record.running) ?? false,
    addr,
    port,
    allowNonLoopback,
    baseUrl: asString(record.baseUrl) ?? asString(record.base_url),
    error: typeof record.error === "string" ? record.error : null,
  };
}

/**
 * 计算对外展示的 Base URL。
 *
 * 优先使用后端返回的 `baseUrl`；否则按监听地址与端口拼接（兜底，正常不应触发）。
 * `0.0.0.0` 对外不可直接使用，展示时回退为 `127.0.0.1`。
 */
export function resolveGatewayBaseUrl(
  status: GatewayStatus | null,
  fallbackAddr: string,
  fallbackPort: number,
): string {
  if (status?.baseUrl) return status.baseUrl;
  const rawHost = status?.addr ?? fallbackAddr;
  const host = rawHost === "0.0.0.0" ? "127.0.0.1" : rawHost;
  const port = status?.port ?? fallbackPort;
  return `http://${host}:${port}/v1`;
}

/** 网关进程是否实际在监听（缺省视为已停止）。 */
export function resolveGatewayRunning(status: GatewayStatus | null): boolean {
  return status?.running ?? false;
}
