//! Retry policy.
//!
//! The bundle builder drives a retry loop between submit attempts. Only
//! outcomes for which [`crate::Error::is_retriable`] returns `true` are
//! retried; rejections and config errors propagate on the first attempt.
//!
//! ```
//! use jito_bundler::RetryPolicy;
//! use std::time::Duration;
//!
//! let policy = RetryPolicy::new()
//!     .max_attempts(4)
//!     .initial_backoff(Duration::from_millis(250))
//!     .backoff_multiplier(2.0)
//!     .tip_bump_bps(2500); // +25 % per retry
//! ```

use std::time::Duration;

use crate::error::{invalid_config, Result};

/// Exponential-backoff retry policy with optional tip bumping on drops.
///
/// Defaults are tuned for mainnet bundle submission: 3 attempts, 200 ms
/// initial delay doubling each time, and a 20 % tip bump between attempts.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    max_attempts: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
    multiplier: f64,
    tip_bump_bps: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(4),
            multiplier: 2.0,
            tip_bump_bps: 2000,
        }
    }
}

impl RetryPolicy {
    /// Starts from the default policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Disables retries entirely. The bundle is submitted once and the result
    /// — landed or otherwise — is returned directly.
    pub fn none() -> Self {
        Self {
            max_attempts: 1,
            ..Self::default()
        }
    }

    /// Total number of attempts. `1` disables retries; must be at least `1`.
    pub fn max_attempts(mut self, n: u32) -> Self {
        self.max_attempts = n.max(1);
        self
    }

    /// Delay before the second attempt. Subsequent attempts scale by
    /// [`Self::backoff_multiplier`] and cap at [`Self::max_backoff`].
    pub fn initial_backoff(mut self, d: Duration) -> Self {
        self.initial_backoff = d;
        self
    }

    /// Hard ceiling on the per-attempt delay. Defaults to 4 seconds.
    pub fn max_backoff(mut self, d: Duration) -> Self {
        self.max_backoff = d;
        self
    }

    /// Geometric multiplier applied between retries. Must be finite and ≥ 1.
    pub fn backoff_multiplier(mut self, m: f64) -> Self {
        self.multiplier = m;
        self
    }

    /// Basis-point tip bump applied between retries on drop. `10000` = 100 %.
    /// Set to `0` to keep the tip constant.
    pub fn tip_bump_bps(mut self, bps: u32) -> Self {
        self.tip_bump_bps = bps;
        self
    }

    /// Accessor used by the builder.
    pub(crate) fn max_attempts_get(&self) -> u32 {
        self.max_attempts
    }

    /// Computes the delay before the given 1-indexed attempt. Attempt 1 has
    /// no pre-delay.
    pub(crate) fn backoff_for(&self, attempt: u32) -> Duration {
        if attempt <= 1 {
            return Duration::ZERO;
        }
        let exp = (attempt - 1) as i32 - 1;
        let scale = self.multiplier.powi(exp).max(1.0);
        let nanos = (self.initial_backoff.as_nanos() as f64 * scale) as u128;
        let capped = nanos.min(self.max_backoff.as_nanos());
        Duration::from_nanos(capped as u64)
    }

    /// Applies the configured bump to the tip amount. Saturates on overflow so
    /// a pathological multiplier cannot underflow the next retry.
    pub(crate) fn bump_tip(&self, current_lamports: u64) -> u64 {
        let bump = (current_lamports as u128)
            .saturating_mul(self.tip_bump_bps as u128)
            / 10_000u128;
        current_lamports.saturating_add(bump.min(u64::MAX as u128) as u64)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if !self.multiplier.is_finite() || self.multiplier < 1.0 {
            return Err(invalid_config(format!(
                "backoff_multiplier must be finite and >= 1.0 (got {})",
                self.multiplier
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let p = RetryPolicy::new()
            .initial_backoff(Duration::from_millis(100))
            .backoff_multiplier(2.0)
            .max_backoff(Duration::from_millis(500));

        assert_eq!(p.backoff_for(1), Duration::ZERO);
        assert_eq!(p.backoff_for(2), Duration::from_millis(100));
        assert_eq!(p.backoff_for(3), Duration::from_millis(200));
        assert_eq!(p.backoff_for(4), Duration::from_millis(400));
        assert_eq!(p.backoff_for(5), Duration::from_millis(500)); // capped
    }

    #[test]
    fn tip_bump_compounds() {
        let p = RetryPolicy::new().tip_bump_bps(2500); // +25 %
        let t1 = p.bump_tip(10_000);
        assert_eq!(t1, 12_500);
        let t2 = p.bump_tip(t1);
        assert_eq!(t2, 15_625);
    }

    #[test]
    fn tip_bump_zero_is_noop() {
        let p = RetryPolicy::new().tip_bump_bps(0);
        assert_eq!(p.bump_tip(10_000), 10_000);
    }
}
