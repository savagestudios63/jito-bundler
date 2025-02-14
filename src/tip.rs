//! Tip strategies and the streaming tip oracle.
//!
//! Every Jito bundle must include a tip instruction paying one of the tip
//! receivers. This module provides:
//!
//! * [`TipStrategy`] — how to choose a tip amount at build time;
//! * [`TipAccount`] — the eight tip receivers, with round-robin pick;
//! * [`TipOracle`] — a rolling window of recently-landed tips with percentile
//!   queries, fed by Jito's public tip stream WebSocket (requires the
//!   `tip-stream` feature).
//!
//! ```no_run
//! use jito_bundler::{JitoEngine, TipStrategy};
//!
//! # async fn demo() -> anyhow::Result<()> {
//! let engine = JitoEngine::auto_region().await?;
//! let tip = engine.tip_oracle().p75().await?;
//! let _ = engine.bundle().tip(TipStrategy::Fixed(tip));
//! # Ok(()) }
//! ```

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use solana_sdk::pubkey::Pubkey;

use crate::error::{Error, Result};

/// How the bundle builder should choose a tip amount.
#[derive(Clone)]
pub enum TipStrategy {
    /// Always pay exactly `n` lamports.
    Fixed(u64),
    /// Pay the median (p50) of recent landed tips.
    Percentile50,
    /// Pay the 75th percentile.
    Percentile75,
    /// Pay the 95th percentile — aggressive land rate.
    Percentile95,
    /// Pay whatever the supplied closure returns. Useful for custom curves
    /// (e.g. function of the account being written to, or the notional value
    /// of the trade).
    Dynamic(Arc<dyn Fn() -> u64 + Send + Sync>),
}

impl std::fmt::Debug for TipStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TipStrategy::Fixed(n) => write!(f, "Fixed({n})"),
            TipStrategy::Percentile50 => f.write_str("Percentile50"),
            TipStrategy::Percentile75 => f.write_str("Percentile75"),
            TipStrategy::Percentile95 => f.write_str("Percentile95"),
            TipStrategy::Dynamic(_) => f.write_str("Dynamic(<closure>)"),
        }
    }
}

impl TipStrategy {
    /// A sane default for bots that just want their bundles to land without
    /// tuning: p75 of the last minute.
    pub fn reasonable() -> Self {
        TipStrategy::Percentile75
    }
}

impl Default for TipStrategy {
    fn default() -> Self {
        TipStrategy::Percentile75
    }
}

/// Jito tip receivers. Pick one per bundle; the block engine validates that
/// at least one transaction in the bundle transfers to one of these accounts.
#[derive(Debug, Clone, Copy)]
pub struct TipAccount(pub Pubkey);

impl TipAccount {
    /// The eight canonical tip accounts published by Jito.
    pub fn all() -> [TipAccount; 8] {
        const ACCOUNTS: [&str; 8] = [
            "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
            "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
            "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
            "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
            "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
            "ADuUkR4vqLUMWXxW9gh6D6L8pivKeVBBjQHSRRz9UpVM",
            "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
            "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
        ];
        let mut out = [TipAccount(Pubkey::default()); 8];
        for (i, s) in ACCOUNTS.iter().enumerate() {
            // Unwrap is safe: these addresses are constants and checked by
            // the unit test below.
            out[i] = TipAccount(Pubkey::from_str(s).expect("valid tip pubkey"));
        }
        out
    }

    /// Picks a tip account deterministically by index. Bundles can use the
    /// submission slot or a hash of the transaction set to rotate across
    /// receivers and avoid hotspotting one account.
    pub fn pick(index: usize) -> TipAccount {
        let all = Self::all();
        all[index % all.len()]
    }
}

/// Rolling window of recently-landed tip lamports. Fed by the tip-stream
/// WebSocket (feature `tip-stream`) or manually via [`TipOracle::push_sample`].
///
/// Percentile queries are `O(n log n)` over the current window (default
/// capacity 512 samples), which is negligible compared to bundle latency.
#[derive(Clone)]
pub struct TipOracle {
    inner: Arc<RwLock<TipWindow>>,
}

struct TipWindow {
    samples: Vec<u64>,
    capacity: usize,
    cursor: usize,
    filled: bool,
}

impl TipOracle {
    /// Creates an empty oracle with a capacity of 512 samples.
    pub fn new() -> Self {
        Self::with_capacity(512)
    }

    /// Creates an empty oracle with the given ring-buffer capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            inner: Arc::new(RwLock::new(TipWindow {
                samples: vec![0; capacity],
                capacity,
                cursor: 0,
                filled: false,
            })),
        }
    }

    /// Number of samples currently held (up to the capacity).
    pub fn len(&self) -> usize {
        let w = self.inner.read();
        if w.filled {
            w.capacity
        } else {
            w.cursor
        }
    }

    /// Returns `true` if no samples have been observed yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Pushes a single landed-tip sample (lamports). Typically called from
    /// the tip-stream subscriber, but exposed so callers can inject their own
    /// samples (e.g. from on-chain logs).
    pub fn push_sample(&self, lamports: u64) {
        let mut w = self.inner.write();
        let slot = w.cursor;
        w.samples[slot] = lamports;
        w.cursor = (slot + 1) % w.capacity;
        if w.cursor == 0 {
            w.filled = true;
        }
    }

    /// Median (50th percentile) of the current window.
    pub async fn p50(&self) -> Result<u64> {
        self.percentile(50.0).await
    }
    /// 75th percentile.
    pub async fn p75(&self) -> Result<u64> {
        self.percentile(75.0).await
    }
    /// 95th percentile.
    pub async fn p95(&self) -> Result<u64> {
        self.percentile(95.0).await
    }

    /// Arbitrary percentile in the range `[0.0, 100.0]`.
    ///
    /// Returns [`Error::TipOracleCold`] if the oracle has not yet received a
    /// sample. Call [`Self::warmup`] first if a guaranteed value is needed.
    pub async fn percentile(&self, p: f64) -> Result<u64> {
        let snapshot = {
            let w = self.inner.read();
            let end = if w.filled { w.capacity } else { w.cursor };
            if end == 0 {
                return Err(Error::TipOracleCold { samples: 0 });
            }
            let mut v = w.samples[..end].to_vec();
            v.sort_unstable();
            v
        };

        let p = p.clamp(0.0, 100.0);
        let idx = ((snapshot.len() as f64 - 1.0) * (p / 100.0)).round() as usize;
        Ok(snapshot[idx.min(snapshot.len() - 1)])
    }

    /// Awaits until at least `n` samples are available or the timeout elapses.
    pub async fn warmup(&self, n: usize, timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.len() >= n {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::TipOracleCold {
                    samples: self.len(),
                });
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl Default for TipOracle {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TipOracle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TipOracle")
            .field("samples", &self.len())
            .finish()
    }
}

#[cfg(feature = "tip-stream")]
mod tip_stream {
    use std::time::Duration;

    use futures_util::StreamExt;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;

    use super::TipOracle;
    use crate::error::{Error, Result};

    #[derive(serde::Deserialize)]
    struct TipStreamFrame {
        // The upstream schema has several fields; we only depend on the 50th
        // percentile lamports value per batch. Anything else is tolerated.
        landed_tips_50th_percentile: Option<f64>,
        landed_tips_75th_percentile: Option<f64>,
        landed_tips_95th_percentile: Option<f64>,
    }

    impl TipOracle {
        /// Connects to the tip-stream WebSocket and feeds samples into the
        /// oracle in a background task. Returns immediately; the caller may
        /// `.await` [`TipOracle::warmup`] to wait for samples to arrive.
        ///
        /// The task reconnects with exponential backoff on disconnect. Drop
        /// the returned [`tokio::task::JoinHandle`] to cancel; the oracle
        /// remains usable with whatever samples it already collected.
        pub fn spawn_stream(&self, url: &str) -> tokio::task::JoinHandle<()> {
            let oracle = self.clone();
            let url = url.to_owned();
            tokio::spawn(async move {
                let mut backoff = Duration::from_millis(500);
                loop {
                    match connect_async(&url).await {
                        Ok((mut ws, _)) => {
                            backoff = Duration::from_millis(500);
                            while let Some(msg) = ws.next().await {
                                let Ok(Message::Text(txt)) = msg else { continue };
                                let Ok(frames) =
                                    serde_json::from_str::<Vec<TipStreamFrame>>(&txt)
                                else {
                                    continue;
                                };
                                for f in frames {
                                    // Feed the three published percentiles so
                                    // the internal window reflects the real
                                    // distribution even with sparse batches.
                                    for v in [
                                        f.landed_tips_50th_percentile,
                                        f.landed_tips_75th_percentile,
                                        f.landed_tips_95th_percentile,
                                    ]
                                    .into_iter()
                                    .flatten()
                                    {
                                        if v.is_finite() && v >= 0.0 {
                                            oracle.push_sample(v as u64);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(target: "jito_bundler::tip_oracle",
                                "tip-stream connect failed: {e}, retrying in {:?}",
                                backoff);
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(Duration::from_secs(30));
                            let _: Result<()> = Err(Error::Transport(e.to_string()));
                        }
                    }
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tip_accounts_all_parse() {
        let accounts = TipAccount::all();
        for a in accounts {
            assert_ne!(a.0, Pubkey::default());
        }
    }

    #[tokio::test]
    async fn percentile_on_known_distribution() {
        let oracle = TipOracle::with_capacity(100);
        for i in 1..=100u64 {
            oracle.push_sample(i * 1000);
        }
        assert_eq!(oracle.p50().await.unwrap(), 50_000);
        assert_eq!(oracle.p75().await.unwrap(), 75_000);
        assert_eq!(oracle.p95().await.unwrap(), 95_000);
    }

    #[tokio::test]
    async fn cold_oracle_errors() {
        let oracle = TipOracle::new();
        assert!(matches!(
            oracle.p50().await,
            Err(Error::TipOracleCold { samples: 0 })
        ));
    }

    #[tokio::test]
    async fn ring_buffer_wraps() {
        let oracle = TipOracle::with_capacity(4);
        for i in 0..10u64 {
            oracle.push_sample(i);
        }
        // Only the last 4 samples (6, 7, 8, 9) should be present.
        assert_eq!(oracle.len(), 4);
        assert_eq!(oracle.p95().await.unwrap(), 9);
    }
}
