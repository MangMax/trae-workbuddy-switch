//! 产品线变体（variant）：把 Trae 各条产品线的全部差异收敛到一张表。
//!
//! ## 为什么需要这一层
//!
//! Trae 有**多条可以同机并存的产品线**（实测本机同时装着 `TRAE SOLO CN` 与
//! `Trae CN`，各有独立的安装目录、userData、进程名）。在引入本模块之前，
//! 这些差异散落在 [`super::platform`]（候选名列表）与 [`super`]（写死的 CN 端点常量）里，
//! 且**只有一个「全候选混在一起」的视角** —— 于是：
//!
//! - 界面上分不清「管理的是哪一条产品线」（自动探测按最近活跃挑，用户只能猜）；
//! - 想按产品线派生端点时没有可用的维度（端点只能写死成 CN 那套）。
//!
//! 本模块给出与 [`crate::modules::region`] 同形的表驱动设计：`TraeVariant` 枚举 +
//! `variant_spec()` 查表。**所有按产品线分家的取值都必须从这里派生**，
//! 不要再在别处散落 `if solo { … } else { … }`。
//!
//! ## 与 `Region` 的关系：正交，不要合并
//!
//! [`crate::modules::region::Region`] 描述 **WorkBuddy 这一产品的两个发行版本**
//! （`workbuddy-desktop.info` / `workbuddy-desktop-ai.info`），它们的认证文件格式、
//! 账号库结构、上游协议形状**完全一致**，差异只是域名/版本号。而 Trae 的产品线差异是
//! **产品级**的：凭据形态、账号库文件名、登录态载体、上游主机族都不同。
//! 把 Trae 塞进 `Region` 会让 `region_spec(All)`、`accounts_file_for(All)` 之类的既有约定
//! 失去定义，且所有 `match region { Cn | Global }` 的调用点都要补分支。
//! 因此两者**并列存在、互不转换**。
//!
//! ## 端点取值的权威来源（重要）
//!
//! 下表里的主机值**不是猜的**，来自客户端自带且随安装包一起分发的
//! `<安装根>\<产品名>\resources\app\product.json`：
//!
//! ```text
//! product.json → bootConfig → <能力>.trae.<regionKey>
//! ```
//!
//! 其中 `normal` 是 CN 构建的默认 region 键（同一份文件里另有 `SG` / `US` / `CN` / `USTTP`）。
//! 实测两条 CN 产品线（`TRAE SOLO CN` / `Trae CN`）的 `*.trae.normal` **逐字相同**，
//! 即：**产品线不改变端点，region 才改变端点**。这一点决定了下面这张表的形状 ——
//! 每个变体记录自己那套 region 键的取值。
//!
//! 同样来自 `product.json` 的身份事实（实测）：
//!
//! | 键 | `TRAE SOLO CN` | `Trae CN` |
//! |:---|:---|:---|
//! | `nameAlias` | **`TraeWork CN`** | `TraeCode CN` |
//! | `packageType` | `SOLO_CN` | `TRAE_CN` |
//! | `runMode` | `solo-lite` | （null） |
//! | `applicationName` | `trae-solo-cn` | `trae-cn` |
//! | `darwinBundleIdentifier` | `cn.trae.solo.app` | `cn.trae.app` |
//!
//! **⇒ `TRAE SOLO CN` 的官方别名就是 `TraeWork CN`**，即它是 Trae Work 产品线的 CN 版；
//! `Trae CN` 才是 IDE 那条线。这就是本模块把两个变体命名为 `TraeWork` / `TraeCn` 的依据，
//! 也是界面上「Trae Work」这一分区名的出处 —— **不是我们起的名字**。
//!
//! ## 未验证项（勿凭推理"补全"）
//!
//! [`EndpointSet::oauth_base`] 与 [`EndpointSet::ws_base`] 有变体差异，但**没有**抓包证据
//! 表明它们在国际化场景下的正确取值；同理 [`TraeVariant::TraeCn`] 的国际化端点虽从
//! `product.json` 读到，却**从未对真实上游跑通过**（本机没装国际版客户端、没有可用凭据）。
//! 这些字段一律带「未验证」标注并保留 `Option`，让调用方显式处理缺失，
//! 而不是静默用 CN 值冒充国际化值。

use std::path::PathBuf;

/// Trae 产品线变体。
///
/// 命名依据是客户端 `product.json` 的 `nameAlias`：`TRAE SOLO CN` 自称 `TraeWork CN`，
/// `Trae CN` 自称 `TraeCode CN`。对外展示用 [`TraeVariant::display_name`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraeVariant {
    /// Trae Work 产品线（对应客户端的 `TRAE SOLO` / `TRAE SOLO CN`）。
    TraeWork,
    /// Trae IDE 产品线（对应客户端的 `Trae` / `Trae CN`）。
    TraeCn,
}

impl TraeVariant {
    /// 稳定标识（用于文件名、序列化与前端传参）。
    ///
    /// `trae_work` 用下划线而非连字符：它会出现在落盘文件名里，
    /// 与仓库既有 `profiles_trae` 之类的命名习惯一致。
    pub fn as_str(self) -> &'static str {
        match self {
            TraeVariant::TraeWork => "trae_work",
            TraeVariant::TraeCn => "trae_cn",
        }
    }

    /// 对外展示名（界面分区标题、日志、错误文案）。
    ///
    /// `TraeWork` 用官方 `nameAlias` 的写法 `Trae Work`（带空格）；
    /// `TraeCn` 用 `Trae CN`（与客户端 `nameShort` 一致）。
    pub fn display_name(self) -> &'static str {
        match self {
            TraeVariant::TraeWork => "Trae Work",
            TraeVariant::TraeCn => "Trae CN",
        }
    }

    /// 从字符串解析变体。大小写不敏感，接受若干常见别名。
    ///
    /// 接受 `TRAE SOLO CN` / `TRAE SOLO` 这类**目录名**，因为用户可能直接从
    /// 探测结果（`TraeEnvStatus.dataDir` 的末段）把产品名传回来。
    pub fn parse(s: &str) -> Option<TraeVariant> {
        match s.trim().to_ascii_lowercase().as_str() {
            "trae_work" | "traework" | "trae work" | "work" | "solo" | "trae solo" | "trae solo cn" => {
                Some(TraeVariant::TraeWork)
            }
            "trae_cn" | "traecn" | "trae cn" | "cn" | "ide" | "trae" => Some(TraeVariant::TraeCn),
            _ => None,
        }
    }

    /// 全部已知变体，供遍历用。
    pub fn all() -> [TraeVariant; 2] {
        [TraeVariant::TraeWork, TraeVariant::TraeCn]
    }
}

impl Default for TraeVariant {
    /// 默认 `TraeWork`。
    ///
    /// 选它而非 `TraeCn` 的理由与参考实现一致：`TraeWorkAssistant` 的
    /// `TargetApp::parse` 对未知值**回退 `TraeWork`**，且 `TRAE SOLO CN` 是本机
    /// 最近活跃的产品线。**注意这只是一处"缺省"**，不是"另一条线不支持"。
    fn default() -> Self {
        TraeVariant::TraeWork
    }
}

/// 一套端点取值（对应 `product.json` 里某个 region 键下的全部主机）。
///
/// 字段名对齐 [`super`] 里既有的常量语义，便于逐字对拍。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointSet {
    /// 账号中心 / 签到 / 积分基址（`bootConfig.account.trae.*` / `ug.trae.*`）。
    pub account_base: &'static str,
    /// iCube / 市场 / ASR 基址（`bootConfig.iCube.*` / `market.trae.*` / `asr.domain.*`）。
    pub icube_base: &'static str,
    /// 对话（agent）网关基址（`bootConfig.agent.trae.*`，与 `icube_base` **不同主机**）。
    pub agent_host: &'static str,
    /// WebSocket 基址（`bootConfig.ws.trae.*`）。
    ///
    /// **未验证**：CN 值是实测读到的，国际化值同样读到了但**从未连通过**。
    pub ws_base: Option<&'static str>,
}

/// 一个变体的全部差异。
#[derive(Debug, Clone, Copy)]
pub struct VariantSpec {
    /// 所属变体。
    pub variant: TraeVariant,
    /// 稳定的展示名（界面分区标题）。
    pub display_name: &'static str,
    /// 客户端 `product.json` 的 `nameAlias`（官方自称，用于诊断文案与核对）。
    pub name_alias: &'static str,
    /// 客户端 `product.json` 的 `packageType`。
    pub package_type: &'static str,
    /// userData 目录名候选（`%APPDATA%\<名字>`，按优先级）。
    ///
    /// 同一变体可能有多个名字：客户端 `dataFolderName` 都是 `.trae-cn`，
    /// 但 userData 用的是 `nameShort`。
    pub data_dir_names: &'static [&'static str],
    /// 可执行文件名候选（按优先级，与 [`VariantSpec::data_dir_names`] 一一对应）。
    pub exe_names: &'static [&'static str],
    /// 进程名（Windows 精简名，不带 `.exe`）候选。
    pub proc_names: &'static [&'static str],
    /// CN 端点集（有实测依据）。
    pub cn_endpoints: EndpointSet,
    /// 国际版端点集。
    ///
    /// **从未对真实上游跑通过**（本机未装国际版客户端、无可用凭据），
    /// 值取自 CN 客户端 `product.json` 里的 `SG`/`US` 键 —— 那是**客户端自己声明的**，
    /// 比我们的任何推断都可信，但仍不等于「服务端接受」。
    pub global_endpoints: Option<EndpointSet>,
}

impl VariantSpec {
    /// 取该变体的端点集。
    ///
    /// `international = false` 一律返回 CN 端点（有实测依据）；
    /// `true` 返回国际版端点 —— **可能为 `None`**，调用方必须显式处理，
    /// 不要用 CN 值兜底冒充国际化值（那会把请求打到错的域，且错误很难定位）。
    pub fn endpoints(&self, international: bool) -> Option<&EndpointSet> {
        if international {
            self.global_endpoints.as_ref()
        } else {
            Some(&self.cn_endpoints)
        }
    }

    /// 该变体在 userData 根目录下的候选路径（未做存在性过滤）。
    pub fn data_dir_candidates(&self, base: &std::path::Path) -> Vec<PathBuf> {
        self.data_dir_names
            .iter()
            .map(|name| base.join(name))
            .collect()
    }
}

/// Trae Work 变体（`TRAE SOLO` / `TRAE SOLO CN`）。
///
/// **CN 端点取值逐字等于改造前的 `modules::trae` 常量**，保证既有行为零变化：
/// 改造前 `TRAE_API_BASE = "https://api.trae.cn"`、`TRAE_OAUTH_BASE = "https://api.trae.com.cn"`、
/// `TRAE_AGENT_HOST = "https://trae-api-cn.mchost.guru"`。
const TRAE_WORK_SPEC: VariantSpec = VariantSpec {
    variant: TraeVariant::TraeWork,
    display_name: "Trae Work",
    name_alias: "TraeWork CN",
    package_type: "SOLO_CN",
    data_dir_names: &["TRAE SOLO CN", "TRAE SOLO"],
    exe_names: &["TRAE SOLO CN.exe", "TRAE SOLO.exe"],
    proc_names: &["TRAE SOLO CN", "TRAE SOLO"],
    cn_endpoints: EndpointSet {
        account_base: "https://api.trae.cn",
        icube_base: "https://api.trae.com.cn",
        agent_host: "https://trae-api-cn.mchost.guru",
        ws_base: Some("wss://trae-ws-cn.mchost.guru/custom_model"),
    },
    // 国际化取值来自 CN 客户端 product.json 的 SG/US 键。**未验证可否实际连通**。
    global_endpoints: Some(EndpointSet {
        account_base: "https://api.trae.ai",
        icube_base: "https://api.trae.ai",
        agent_host: "https://grow-normal.trae.ai",
        ws_base: None,
    }),
};

/// Trae CN 变体（`Trae` / `Trae CN`）。
const TRAE_CN_SPEC: VariantSpec = VariantSpec {
    variant: TraeVariant::TraeCn,
    display_name: "Trae CN",
    name_alias: "TraeCode CN",
    package_type: "TRAE_CN",
    data_dir_names: &["Trae CN", "Trae"],
    exe_names: &["Trae CN.exe", "Trae.exe"],
    proc_names: &["Trae CN", "Trae"],
    cn_endpoints: EndpointSet {
        // 实测：与 Trae Work 变体**逐字相同** —— 产品线不改变端点，region 才改变。
        account_base: "https://api.trae.cn",
        icube_base: "https://api.trae.com.cn",
        agent_host: "https://trae-api-cn.mchost.guru",
        ws_base: Some("wss://trae-ws-cn.mchost.guru/custom_model"),
    },
    global_endpoints: Some(EndpointSet {
        account_base: "https://api.trae.ai",
        icube_base: "https://api.trae.ai",
        agent_host: "https://grow-normal.trae.ai",
        ws_base: None,
    }),
};

/// 取变体的描述符。
pub fn variant_spec(variant: TraeVariant) -> &'static VariantSpec {
    match variant {
        TraeVariant::TraeWork => &TRAE_WORK_SPEC,
        TraeVariant::TraeCn => &TRAE_CN_SPEC,
    }
}

/// 全部变体的描述符（遍历用）。
pub fn all_specs() -> [&'static VariantSpec; 2] {
    [&TRAE_WORK_SPEC, &TRAE_CN_SPEC]
}

/// 从 userData 目录名 / 安装目录名 / exe 名反查变体。
///
/// 这是「探测到的是哪条产品线」的唯一判定入口：先用目录名精确匹配，
/// 再退化到「包含关系」（容忍 `TRAE SOLO CN` 之类的完整名与 `solo` 之类的片段）。
///
/// 返回 `None` 表示**不认识**这个名字 —— 调用方应保持"跨变体"的宽容行为
/// （例如仍然把它当候选目录），而不是硬判成某一个变体。
pub fn variant_of_name(name: &str) -> Option<TraeVariant> {
    let lowered = name.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return None;
    }
    // 去 `.exe` 后缀：调用方可能传 exe 文件名。
    let lowered = lowered.strip_suffix(".exe").unwrap_or(&lowered).to_string();

    // 先精确匹配全部候选名（含大小写差异）。
    for spec in all_specs() {
        for candidate in spec.data_dir_names.iter().chain(spec.exe_names.iter()) {
            if candidate.eq_ignore_ascii_case(&lowered) {
                return Some(spec.variant);
            }
        }
    }

    // 再退化到包含关系。`solo` 是 Trae Work 的独有词根（`TRAE SOLO`），
    // 放在 `trae` 之前判定，否则 `TRAE SOLO CN` 会被 `trae` 抢先命中。
    if lowered.contains("solo") {
        return Some(TraeVariant::TraeWork);
    }
    if lowered.contains("trae") {
        return Some(TraeVariant::TraeCn);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 变体标识与展示名稳定() {
        assert_eq!(TraeVariant::TraeWork.as_str(), "trae_work");
        assert_eq!(TraeVariant::TraeCn.as_str(), "trae_cn");
        assert_eq!(TraeVariant::TraeWork.display_name(), "Trae Work");
        assert_eq!(TraeVariant::TraeCn.display_name(), "Trae CN");
        assert_eq!(TraeVariant::default(), TraeVariant::TraeWork);
    }

    #[test]
    fn 解析接受目录名与常见别名() {
        for s in ["trae_work", "TraeWork", "Trae Work", "solo", "TRAE SOLO CN"] {
            assert_eq!(TraeVariant::parse(s), Some(TraeVariant::TraeWork), "解析失败: {s}");
        }
        for s in ["trae_cn", "TraeCn", "Trae CN", "ide", "trae"] {
            assert_eq!(TraeVariant::parse(s), Some(TraeVariant::TraeCn), "解析失败: {s}");
        }
        assert_eq!(TraeVariant::parse("doubao"), None);
        assert_eq!(TraeVariant::parse(""), None);
    }

    /// 反查的核心判别式：`solo` 必须先于 `trae` 判定。
    /// 若顺序写反，`TRAE SOLO CN` 会命中 `trae` 分支被误判成 Trae CN。
    #[test]
    fn 反查时solo优先于trae() {
        assert_eq!(variant_of_name("TRAE SOLO CN"), Some(TraeVariant::TraeWork));
        assert_eq!(variant_of_name("TRAE SOLO"), Some(TraeVariant::TraeWork));
        assert_eq!(variant_of_name("TRAE SOLO CN.exe"), Some(TraeVariant::TraeWork));
        assert_eq!(variant_of_name("Trae CN"), Some(TraeVariant::TraeCn));
        assert_eq!(variant_of_name("Trae CN.exe"), Some(TraeVariant::TraeCn));
        // 目录名不区分大小写。
        assert_eq!(variant_of_name("trae solo cn"), Some(TraeVariant::TraeWork));
        // 不认识的平台（豆包）不能被硬判成某个 Trae 变体。
        assert_eq!(variant_of_name("Doubao"), None);
    }

    /// 变体之间的候选名**必须不重叠**：重叠会导致探测时互相抢，
    /// 出现「选中 Trae Work 的安装、读 Trae CN 的 userData」。
    #[test]
    fn 变体候选名互不重叠() {
        let work = variant_spec(TraeVariant::TraeWork);
        let cn = variant_spec(TraeVariant::TraeCn);
        for a in work.data_dir_names {
            assert!(
                !cn.data_dir_names.contains(a),
                "userData 目录名重叠: {a}"
            );
        }
        for a in work.exe_names {
            assert!(!cn.exe_names.contains(a), "exe 名重叠: {a}");
        }
    }

    /// 两条产品线的 CN 端点**逐字相同**（实测结论）。
    /// 这个断言是"产品线不改变端点、region 才改变端点"的机器可读证据；
    /// 若有人给某条产品线单独改了端点，这里会红。
    #[test]
    fn 两条产品线的cn端点逐字相同() {
        let work = variant_spec(TraeVariant::TraeWork).cn_endpoints;
        let cn = variant_spec(TraeVariant::TraeCn).cn_endpoints;
        assert_eq!(work, cn);
        assert_eq!(work.account_base, "https://api.trae.cn");
        assert_eq!(work.icube_base, "https://api.trae.com.cn");
        assert_eq!(work.agent_host, "https://trae-api-cn.mchost.guru");
    }

    /// CN 端点必须等于改造前的硬编码常量原值 —— 保证既有行为零变化。
    #[test]
    fn cn端点等于改造前的常量原值() {
        let spec = variant_spec(TraeVariant::TraeWork);
        assert_eq!(spec.cn_endpoints.account_base, super::super::TRAE_API_BASE_CN);
        assert_eq!(spec.cn_endpoints.icube_base, super::super::TRAE_OAUTH_BASE_CN);
    }

    /// `international = true` 而国际版端点缺失时必须返回 `None`，
    /// **绝不能**回落到 CN 值 —— 那会把国际化请求打到国内域。
    #[test]
    fn 国际版端点缺失时不回落cn() {
        let spec = variant_spec(TraeVariant::TraeWork);
        assert!(spec.endpoints(false).is_some());
        let global = spec.endpoints(true).expect("国际版端点应已登记");
        assert_ne!(global.account_base, spec.cn_endpoints.account_base);
        // 用一个刻意缺国际版端点的 spec 验证 None 语义。
        let mut stripped = *spec;
        stripped.global_endpoints = None;
        assert!(stripped.endpoints(true).is_none());
    }
}
