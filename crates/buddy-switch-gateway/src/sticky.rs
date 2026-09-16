//! 会话粘性：把同一轮对话尽量固定到同一账号。
//!
//! 移植自参考实现 `internal/session` 的粘性绑定（配置 `session_sticky`）。
//!
//! 为什么需要：多账号池下若每轮随机选号，同一会话的上下文会落到不同账号上，
//! 上游侧表现为「对话突然失忆」；且跨账号的缓存前缀命中率归零。
//! 绑定的取舍是：**同一会话优先复用上次成功的账号**，但该账号不可用时立刻解绑
//! 换号——粘性是优化而非约束，绝不能因为粘性而拒绝服务。

use std::collections::HashMap;

/// 一条粘性绑定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickyBinding {
    /// 绑定的账号 uid。
    pub uid: String,
    /// 绑定时刻（毫秒）。
    pub bound_at_ms: i64,
}

/// 粘性绑定表（带 TTL 的 LRU 语义简化版：只按时间淘汰）。
#[derive(Debug, Clone, Default)]
pub struct StickyTable {
    bindings: HashMap<String, StickyBinding>,
    ttl_ms: i64,
}

impl StickyTable {
    /// 新建；`ttl_ms <= 0` 时回落 30 分钟（避免配置漏填导致绑定永不过期）。
    pub fn new(ttl_ms: i64) -> Self {
        Self {
            bindings: HashMap::new(),
            ttl_ms: if ttl_ms > 0 { ttl_ms } else { 30 * 60 * 1000 },
        }
    }

    /// 当前 TTL。
    pub fn ttl_ms(&self) -> i64 {
        self.ttl_ms
    }

    /// 生效中的绑定数（不含已过期条目）。
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// 查询绑定；已过期视为不存在。
    pub fn get(&self, key: &str, now_ms: i64) -> Option<&str> {
        self.bindings
            .get(key)
            .filter(|binding| now_ms.saturating_sub(binding.bound_at_ms) < self.ttl_ms)
            .map(|binding| binding.uid.as_str())
    }

    /// 写入/刷新绑定。
    pub fn bind(&mut self, key: &str, uid: &str, now_ms: i64) {
        if key.is_empty() || uid.is_empty() {
            return;
        }
        self.bindings.insert(
            key.to_string(),
            StickyBinding {
                uid: uid.to_string(),
                bound_at_ms: now_ms,
            },
        );
    }

    /// 解绑（换号时调用；下次请求重新绑定）。
    pub fn unbind(&mut self, key: &str) {
        self.bindings.remove(key);
    }

    /// 清理过期条目，返回清理数量。
    pub fn gc(&mut self, now_ms: i64) -> usize {
        let ttl = self.ttl_ms;
        let before = self.bindings.len();
        self.bindings
            .retain(|_, binding| now_ms.saturating_sub(binding.bound_at_ms) < ttl);
        before - self.bindings.len()
    }
}

/// 派生粘性键：`region|model|轮主键`。
///
/// 把模型纳入键是刻意的：同一会话切到不同模型时，应当允许落到各自最合适的账号
/// （否则「模型级限流只封锁该模型」的优势会被粘性抵消）。
pub fn sticky_key(region: &str, model: &str, conversation_request_id: &str) -> String {
    format!("{region}|{model}|{conversation_request_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000_000_000;

    #[test]
    fn bind_then_get_returns_uid() {
        let mut table = StickyTable::new(1000);
        table.bind("k", "u1", NOW);
        assert_eq!(table.get("k", NOW), Some("u1"));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn expired_binding_is_invisible() {
        let mut table = StickyTable::new(1000);
        table.bind("k", "u1", NOW);
        assert_eq!(table.get("k", NOW + 999), Some("u1"), "TTL 内有效");
        assert_eq!(table.get("k", NOW + 1000), None, "边界即失效");
    }

    #[test]
    fn bind_refreshes_timestamp() {
        let mut table = StickyTable::new(1000);
        table.bind("k", "u1", NOW);
        table.bind("k", "u1", NOW + 900);
        assert_eq!(table.get("k", NOW + 1500), Some("u1"), "重新绑定应刷新计时");
    }

    #[test]
    fn unbind_removes_binding() {
        let mut table = StickyTable::new(1000);
        table.bind("k", "u1", NOW);
        table.unbind("k");
        assert_eq!(table.get("k", NOW), None);
        assert!(table.is_empty());
    }

    #[test]
    fn gc_drops_only_expired_entries() {
        let mut table = StickyTable::new(1000);
        table.bind("old", "u1", NOW);
        table.bind("new", "u2", NOW + 900);
        assert_eq!(table.gc(NOW + 1500), 1, "只应清理一条");
        assert_eq!(table.get("new", NOW + 1500), Some("u2"));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn empty_key_or_uid_is_ignored() {
        let mut table = StickyTable::new(1000);
        table.bind("", "u1", NOW);
        table.bind("k", "", NOW);
        assert!(table.is_empty());
    }

    #[test]
    fn non_positive_ttl_falls_back_to_default() {
        let table = StickyTable::new(0);
        assert_eq!(table.ttl_ms(), 30 * 60 * 1000);
        let table = StickyTable::new(-5);
        assert_eq!(table.ttl_ms(), 30 * 60 * 1000);
    }

    #[test]
    fn sticky_key_separates_region_and_model() {
        assert_eq!(sticky_key("cn", "glm-5.2", "req"), "cn|glm-5.2|req");
        assert_ne!(
            sticky_key("cn", "glm-5.2", "req"),
            sticky_key("cn", "deepseek-v4-flash", "req"),
            "不同模型不得共享绑定"
        );
        assert_ne!(
            sticky_key("cn", "glm-5.2", "req"),
            sticky_key("global", "glm-5.2", "req"),
            "不同域不得共享绑定"
        );
    }
}
