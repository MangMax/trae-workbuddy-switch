//! 确定性伪随机源（PCG32），用于账号池的加权抽签与等权重洗牌。
//!
//! 为什么不用 `rand`：选号结果必须**可复现**才能写可证伪的单测（给定种子 → 给定账号，
//! 断言必须逐次一致）。参考实现同样使用自有 PCG 实例（`rand::PCG(seed, len)`）而非
//! 全局随机源，此处对齐该语义：种子由调用方注入，测试注入固定种子，生产注入
//! `now_nanos ^ pick_seq`，因此同名一次选号在并发下也不会退化为同一序列。

/// PCG-XSH-RR 32 位发生器（LCG 状态 + 输出置换）。
const PCG_MULTIPLIER: u64 = 6364136223846793005;

/// 确定性随机源。
#[derive(Debug, Clone)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    /// 以 `seed` 与 `stream` 构造。`stream` 取奇数（内部强制），不同 `stream`
    /// 得到互不重叠的状态序列——洗牌用独立 `stream`，避免消耗抽签序列。
    pub fn new(seed: u64, stream: u64) -> Self {
        let inc = (stream << 1) | 1;
        let mut rng = Self { state: 0, inc };
        rng.state = rng.state.wrapping_mul(PCG_MULTIPLIER).wrapping_add(inc);
        rng.state = rng.state.wrapping_add(seed);
        rng.state = rng.state.wrapping_mul(PCG_MULTIPLIER).wrapping_add(inc);
        rng
    }

    /// 下一个 32 位输出。
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(PCG_MULTIPLIER).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// 下一个 64 位输出（两次 32 位拼接）。
    pub fn next_u64(&mut self) -> u64 {
        ((self.next_u32() as u64) << 32) | self.next_u32() as u64
    }

    /// `[0, bound)` 上的**无偏**取值（拒绝采样消除取模偏置）。
    ///
    /// `bound == 0` 返回 0（调用方在空集合上不应调用；此处不 panic 以保持
    /// 选号路径在空池时不致崩）。
    pub fn below(&mut self, bound: u64) -> u64 {
        if bound <= 1 {
            return 0;
        }
        // 拒绝阈值：2^64 mod bound，等价于 bound.wrapping_neg() % bound。
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let value = self.next_u64();
            if value >= threshold {
                return value % bound;
            }
        }
    }

    /// Fisher-Yates 原地洗牌。
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        if items.len() < 2 {
            return;
        }
        for index in (1..items.len()).rev() {
            let swap_with = self.below((index + 1) as u64) as usize;
            items.swap(index, swap_with);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_and_stream_reproduce_identical_sequence() {
        let mut a = Pcg32::new(42, 7);
        let mut b = Pcg32::new(42, 7);
        let left: Vec<u32> = (0..16).map(|_| a.next_u32()).collect();
        let right: Vec<u32> = (0..16).map(|_| b.next_u32()).collect();
        assert_eq!(left, right, "同种子必须逐项复现");
    }

    #[test]
    fn different_seed_diverges() {
        let mut a = Pcg32::new(1, 1);
        let mut b = Pcg32::new(2, 1);
        let left: Vec<u32> = (0..8).map(|_| a.next_u32()).collect();
        let right: Vec<u32> = (0..8).map(|_| b.next_u32()).collect();
        assert_ne!(left, right);
    }

    #[test]
    fn different_stream_diverges() {
        let mut a = Pcg32::new(9, 1);
        let mut b = Pcg32::new(9, 2);
        let left: Vec<u32> = (0..8).map(|_| a.next_u32()).collect();
        let right: Vec<u32> = (0..8).map(|_| b.next_u32()).collect();
        assert_ne!(left, right, "不同 stream 不得退化为同一序列");
    }

    #[test]
    fn below_respects_bound_and_edge_cases() {
        let mut rng = Pcg32::new(1234, 3);
        assert_eq!(rng.below(0), 0);
        assert_eq!(rng.below(1), 0);
        for _ in 0..500 {
            let value = rng.below(7);
            assert!(value < 7, "取值必须落在 [0,7)：{value}");
        }
    }

    #[test]
    fn below_covers_full_range_without_bias_hang() {
        // 覆盖性：bound=5 时 0..4 都必须出现过（证明不是常量输出或死循环）。
        let mut rng = Pcg32::new(2024, 11);
        let mut seen = [false; 5];
        for _ in 0..200 {
            seen[rng.below(5) as usize] = true;
        }
        assert!(seen.iter().all(|hit| *hit), "五个桶都应被覆盖: {seen:?}");
    }

    #[test]
    fn shuffle_is_a_permutation_and_seed_deterministic() {
        let mut rng = Pcg32::new(77, 5);
        let mut items: Vec<u32> = (0..10).collect();
        rng.shuffle(&mut items);
        let mut sorted = items.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..10).collect::<Vec<u32>>(), "洗牌必须是置换");

        let mut a = vec![0u32, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let mut b = vec![0u32, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        Pcg32::new(5, 9).shuffle(&mut a);
        Pcg32::new(5, 9).shuffle(&mut b);
        assert_eq!(a, b, "同种子洗牌结果必须一致");
    }

    #[test]
    fn shuffle_handles_degenerate_lengths() {
        let mut rng = Pcg32::new(1, 1);
        let mut empty: Vec<u32> = Vec::new();
        rng.shuffle(&mut empty);
        assert!(empty.is_empty());

        let mut single = vec![42u32];
        rng.shuffle(&mut single);
        assert_eq!(single, vec![42]);
    }
}
