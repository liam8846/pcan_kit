use core::hash::{BuildHasher, Hasher};
use core::time::Duration;
use std::collections::hash_map::RandomState;

/// 重連退避策略。
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct BackoffPolicy {
    /// 首次退避延遲。
    pub initial: Duration,
    /// 退避延遲上限。
    pub max: Duration,
    /// 每次失敗的延遲倍率。
    pub multiplier: f64,
    /// 抖動比例；例如 `0.25` 表示基礎值的正負百分之二十五。
    pub jitter_ratio: f64,
    /// 最大重試次數；`None` 表示無限重試。
    pub max_attempts: Option<u32>,
    /// 連線穩定多久後將重試計數歸零。
    ///
    /// 若每次短暫連上就立即歸零，持續抖動的裝置會高頻重試；若永不歸零，
    /// 間歇故障又會永遠停在最大延遲。因此以穩定時間作為復原門檻。
    pub reset_after_stable: Duration,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(100),
            max: Duration::from_secs(30),
            multiplier: 2.0,
            jitter_ratio: 0.25,
            max_attempts: None,
            reset_after_stable: Duration::from_secs(60),
        }
    }
}

impl BackoffPolicy {
    /// 計算第 `attempt` 次（一為起點）的基礎延遲，不含抖動。
    #[must_use]
    pub fn base_delay(&self, attempt: u32) -> Duration {
        if attempt == 0 || self.initial >= self.max {
            return self.initial.min(self.max);
        }
        let multiplier = if self.multiplier.is_finite() && self.multiplier > 1.0 {
            self.multiplier
        } else {
            1.0
        };
        let mut seconds = self.initial.as_secs_f64();
        let maximum = self.max.as_secs_f64();
        for _ in 1..attempt {
            if seconds >= maximum / multiplier {
                return self.max;
            }
            seconds *= multiplier;
        }
        if !seconds.is_finite() || seconds >= maximum {
            self.max
        } else {
            Duration::try_from_secs_f64(seconds).unwrap_or(self.max)
        }
    }
}

/// 退避延遲的抖動注入器。
pub trait Jitter: Send + 'static {
    /// 對基礎延遲加上指定比例的抖動。
    fn perturb(&mut self, base: Duration, ratio: f64) -> Duration;
}

/// 不加入抖動的決定性實作。
#[derive(Debug, Default, Clone, Copy)]
pub struct NoJitter;

impl Jitter for NoJitter {
    fn perturb(&mut self, base: Duration, _ratio: f64) -> Duration {
        base
    }
}

/// 使用 `SplitMix64` 的可重現抖動產生器。
#[derive(Debug, Clone)]
pub struct SplitMixJitter {
    state: u64,
}

impl SplitMixJitter {
    /// 以固定種子建立產生器。
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// 使用標準函式庫程序種子建立產生器，不依賴外部亂數 crate。
    #[must_use]
    pub fn from_entropy() -> Self {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| {
                    duration.as_secs() ^ u64::from(duration.subsec_nanos())
                }),
        );
        Self::new(hasher.finish())
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}

impl Default for SplitMixJitter {
    fn default() -> Self {
        Self::from_entropy()
    }
}

impl Jitter for SplitMixJitter {
    fn perturb(&mut self, base: Duration, ratio: f64) -> Duration {
        if ratio <= 0.0 || !ratio.is_finite() {
            return base;
        }
        let upper = u32::try_from(self.next() >> 32).unwrap_or_default();
        let unit = f64::from(upper) / f64::from(u32::MAX);
        let factor = 1.0 + (unit.mul_add(2.0, -1.0) * ratio.clamp(0.0, 1.0));
        Duration::try_from_secs_f64(base.as_secs_f64() * factor).unwrap_or(base)
    }
}
