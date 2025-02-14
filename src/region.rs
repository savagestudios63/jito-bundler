//! Block-engine region catalog and latency-based auto-selection.
//!
//! Jito operates several geographically distributed block engines; submitting
//! to the nearest one materially reduces bundle land-time. [`Region`]
//! enumerates the public endpoints and [`Region::auto_select`] probes each one
//! in parallel and returns the fastest responder.
//!
//! ```no_run
//! use jito_bundler::Region;
//!
//! # async fn demo() -> anyhow::Result<()> {
//! let region = Region::auto_select().await?;
//! println!("using {region:?} ({})", region.json_rpc_url());
//! # Ok(()) }
//! ```

use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// A Jito block-engine region.
///
/// Endpoints are the public defaults published by Jito Labs. The crate does
/// not hard-code any IP addresses; all selection goes through DNS at probe
/// time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Region {
    /// `mainnet.block-engine.jito.wtf` — geo-routed global entry point.
    Mainnet,
    /// Amsterdam.
    Amsterdam,
    /// Frankfurt.
    Frankfurt,
    /// New York.
    NewYork,
    /// Salt Lake City.
    SaltLakeCity,
    /// Tokyo.
    Tokyo,
    /// A user-supplied endpoint; useful for staging or private deployments.
    Custom {
        /// Fully-qualified JSON-RPC base URL (no trailing slash).
        json_rpc: &'static str,
        /// Fully-qualified gRPC endpoint (`host:port`).
        grpc: &'static str,
    },
}

impl Region {
    /// Returns all publicly-known regions in a stable order.
    pub fn all() -> &'static [Region] {
        &[
            Region::Mainnet,
            Region::Amsterdam,
            Region::Frankfurt,
            Region::NewYork,
            Region::SaltLakeCity,
            Region::Tokyo,
        ]
    }

    /// Short, stable identifier suitable for logs and metrics labels.
    pub fn as_str(&self) -> &'static str {
        match self {
            Region::Mainnet => "mainnet",
            Region::Amsterdam => "amsterdam",
            Region::Frankfurt => "frankfurt",
            Region::NewYork => "ny",
            Region::SaltLakeCity => "slc",
            Region::Tokyo => "tokyo",
            Region::Custom { .. } => "custom",
        }
    }

    /// JSON-RPC endpoint URL, without a trailing slash.
    pub fn json_rpc_url(&self) -> &'static str {
        match self {
            Region::Mainnet => "https://mainnet.block-engine.jito.wtf",
            Region::Amsterdam => "https://amsterdam.mainnet.block-engine.jito.wtf",
            Region::Frankfurt => "https://frankfurt.mainnet.block-engine.jito.wtf",
            Region::NewYork => "https://ny.mainnet.block-engine.jito.wtf",
            Region::SaltLakeCity => "https://slc.mainnet.block-engine.jito.wtf",
            Region::Tokyo => "https://tokyo.mainnet.block-engine.jito.wtf",
            Region::Custom { json_rpc, .. } => json_rpc,
        }
    }

    /// gRPC endpoint in `host:port` form, matching Jito's public schema.
    pub fn grpc_endpoint(&self) -> &'static str {
        match self {
            Region::Mainnet => "https://mainnet.block-engine.jito.wtf",
            Region::Amsterdam => "https://amsterdam.mainnet.block-engine.jito.wtf",
            Region::Frankfurt => "https://frankfurt.mainnet.block-engine.jito.wtf",
            Region::NewYork => "https://ny.mainnet.block-engine.jito.wtf",
            Region::SaltLakeCity => "https://slc.mainnet.block-engine.jito.wtf",
            Region::Tokyo => "https://tokyo.mainnet.block-engine.jito.wtf",
            Region::Custom { grpc, .. } => grpc,
        }
    }

    /// WebSocket tip-stream endpoint.
    pub fn tip_stream_url(&self) -> &'static str {
        // Jito exposes the tip stream at a single URL; keeping it per-region
        // lets custom regions override it.
        match self {
            Region::Custom { .. } => "ws://bundles.jito.wtf/api/v1/bundles/tip_stream",
            _ => "ws://bundles.jito.wtf/api/v1/bundles/tip_stream",
        }
    }

    /// Probes every public region in parallel with an HTTP `getHealth`-style
    /// request and returns the fastest responder.
    ///
    /// Each probe has a 1500 ms budget. If every probe fails the method
    /// returns [`Error::Transport`] so the caller can fall back to an explicit
    /// region.
    pub async fn auto_select() -> Result<Region> {
        Self::auto_select_with_timeout(Duration::from_millis(1500)).await
    }

    /// Same as [`Region::auto_select`] but lets the caller control the probe
    /// budget. Useful in tests where you want fast failures, or in bot
    /// startup where you can afford a longer probe.
    pub async fn auto_select_with_timeout(timeout: Duration) -> Result<Region> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;

        let mut handles = Vec::new();
        for region in Self::all().iter().copied() {
            let client = client.clone();
            handles.push(tokio::spawn(async move {
                let started = Instant::now();
                // A plain GET on the health path is the cheapest round-trip
                // the engine exposes; failures here imply the region is
                // unreachable from our vantage point.
                let url = format!("{}/api/v1/bundles", region.json_rpc_url());
                let res = client.head(&url).send().await;
                let elapsed = started.elapsed();
                (region, res.is_ok(), elapsed)
            }));
        }

        let mut best: Option<(Region, Duration)> = None;
        for handle in handles {
            if let Ok((region, ok, elapsed)) = handle.await {
                if !ok {
                    continue;
                }
                match best {
                    Some((_, cur)) if elapsed >= cur => {}
                    _ => best = Some((region, elapsed)),
                }
            }
        }

        best.map(|(r, _)| r)
            .ok_or_else(|| Error::Transport("no Jito region reachable".into()))
    }
}
