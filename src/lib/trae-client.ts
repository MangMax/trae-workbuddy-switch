/**
 * Trae 客户端展示辅助。
 *
 * ## 为什么需要它
 *
 * Trae 有**多个产品线变体**（见 Rust 侧 `modules::trae::variant`）：
 * `TRAE SOLO CN` / `TRAE SOLO`（= Trae Work）与 `Trae CN` / `Trae`（= Trae CN）。
 * 每条各有独立的安装目录、userData 目录与进程名。自动探测会按 userData 的
 * **最近活跃度**挑一条（见 Rust 的 `platform::detect_data_dir`），
 * 因此界面上必须把「挑中的是哪一条」显示出来 —— 否则同时装了两个 Trae 的用户
 * 根本无从判断切换器管的是哪一个，只能靠猜。
 *
 * ## 为什么产品名优先取后端的 `variantLabel`
 *
 * 变体判定（含「solo 词根优先」这种容易写错的判别顺序）只有 Rust 侧一份实现，
 * 前端再写一遍必然漂移。所以这里**首选**后端已经判好的 `variantLabel`，
 * 仅在后端没给时才退回从路径取目录名。
 *
 * 反过来说：**这里只做展示，不参与任何判定**。探测逻辑的唯一真相在 Rust 侧，
 * 前端拿到的 `path` / `dataDir` / `variantLabel` 已经是探测结果。
 */

/** 路径末段（文件名，如 `TRAE SOLO CN.exe`）；取不到返回 `null`。 */
export function pathFileName(path: string | null | undefined): string | null {
  const segments = splitPath(path);
  return segments.length > 0 ? segments[segments.length - 1] : null;
}

/**
 * 路径的**父目录名**（如 `D:\Programs\TRAE SOLO CN\TRAE SOLO CN.exe` → `TRAE SOLO CN`）。
 *
 * 对安装路径而言，父目录名恰好就是产品名（Rust 侧按 `<root>\<产品名>\<exe>` 拼）。
 * 路径层级不足（如只给了 `Trae.exe`）时返回 `null`，让调用方退回不显示。
 */
export function pathParentName(path: string | null | undefined): string | null {
  const segments = splitPath(path);
  return segments.length >= 2 ? segments[segments.length - 2] : null;
}

/**
 * 去掉 `.exe` 后的可执行文件名（如 `Trae CN.exe` → `Trae CN`）。
 *
 * 仅在父目录名不可用时作为兜底：有些用户会把 exe 直接放在根目录或自定义目录，
 * 此时父目录名（`tools` 之类）不代表产品，反而是 exe 名更接近产品名。
 */
export function pathStem(path: string | null | undefined): string | null {
  const name = pathFileName(path);
  if (!name) return null;
  return name.replace(/\.exe$/i, "");
}

/** 按 `\` 与 `/` 切分并丢弃空段；非字符串或空串返回 `[]`。 */
function splitPath(path: string | null | undefined): string[] {
  if (typeof path !== "string") return [];
  return path
    .trim()
    .replace(/[\\/]+$/, "")
    .split(/[\\/]+/)
    .filter((segment) => segment.length > 0);
}

/**
 * 推断「当前管理的是哪条产品线」。
 *
 * **优先用后端的 `variantLabel`**：Rust 侧 `variant::variant_of_name` 是按
 * `%APPDATA%\<产品名>` 的目录名精确匹配变体表得到的，并且带一层「solo 词根优先」
 * 的判别（避免 `TRAE SOLO CN` 被 `trae` 抢先命中）。这是唯一有权威依据的来源。
 *
 * 后端没给（老版本后端 / 探测失败）时才退回**从路径取目录名**：userData 目录名
 * 由 Rust 按 `<产品名>` 逐字拼出，所以 `dataDir` 的末段仍然等于产品名；
 * `path` 只作最后兜底 —— 用户手工配置的 exe 路径可能落在任意目录里，父目录名未必是产品名。
 *
 * 都推断不出来时返回 `null`，调用方应省略该标签而不是显示一个猜测值。
 */
export function traeProductLabel(env: {
  path?: string | null;
  dataDir?: string | null;
  variantLabel?: string | null;
} | null | undefined): string | null {
  if (!env) return null;
  // 后端已判定出变体：直接用它，不要在前端再推一遍（避免两份逻辑漂移）。
  const fromVariant = env.variantLabel?.trim();
  if (fromVariant) return fromVariant;
  // dataDir 形如 `C:\Users\x\AppData\Roaming\TRAE SOLO CN`，末段即产品名。
  const fromDataDir = pathFileName(env.dataDir);
  if (fromDataDir) return fromDataDir;
  return pathParentName(env.path) ?? pathStem(env.path);
}

/**
 * 是否为**自动探测**结果（用户没有手工指定 exe 路径）。
 *
 * 手工指定时，界面上应突出「你指定了哪个」，而不是把自动探测结果当既成事实展示。
 */
export function isAutoDetected(env: {
  configuredPath?: string | null;
} | null | undefined): boolean {
  if (!env) return false;
  return !env.configuredPath?.trim();
}
