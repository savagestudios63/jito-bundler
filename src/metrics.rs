//! Prometheus-compatible metrics (feature-gated by `metrics`).
//!
//! All counters and histograms live under the `jito_bundler_` prefix:
//!
//! | metric | type | labels |
//! |---|---|---|
//! | `jito_bundler_bundles_submitted_total` | counter | `region`, `transport` |
//! | `jito_bundler_bundles_landed_total`    | counter | `region`, `transport` |
//! | `jito_bundler_bundles_dropped_total`   | counter | `region`, `reason` |
//! | `jito_bundler_bundles_rejected_total`  | counter | `region`, `reason` |
//! | `jito_bundler_submit_latency_seconds`  | histogram | `region`, `transport` |
//! | `jito_bundler_tip_lamports`            | histogram | `strategy` |
//!
//! Wire a `metrics-exporter-prometheus` exporter in your binary to expose
//! these via `/metrics`.

#![cfg(feature = "metrics")]

use crate::client::Transport;
use crate::region::Region;

pub(crate) fn record_submit(region: Region, transport: Transport) {
    ::metrics::counter!(
        "jito_bundler_bundles_submitted_total",
        "region" => region.as_str(),
        "transport" => transport_label(transport),
    )
    .increment(1);
}

pub(crate) fn record_landed(region: Region, transport: Transport, latency_s: f64) {
    ::metrics::counter!(
        "jito_bundler_bundles_landed_total",
        "region" => region.as_str(),
        "transport" => transport_label(transport),
    )
    .increment(1);
    ::metrics::histogram!(
        "jito_bundler_submit_latency_seconds",
        "region" => region.as_str(),
        "transport" => transport_label(transport),
    )
    .record(latency_s);
}

pub(crate) fn record_dropped(region: Region, reason: &str) {
    ::metrics::counter!(
        "jito_bundler_bundles_dropped_total",
        "region" => region.as_str(),
        "reason" => reason.to_owned(),
    )
    .increment(1);
}

pub(crate) fn record_rejected(region: Region, reason: &str) {
    ::metrics::counter!(
        "jito_bundler_bundles_rejected_total",
        "region" => region.as_str(),
        "reason" => reason.to_owned(),
    )
    .increment(1);
}

pub(crate) fn record_tip(strategy: &'static str, lamports: u64) {
    ::metrics::histogram!(
        "jito_bundler_tip_lamports",
        "strategy" => strategy,
    )
    .record(lamports as f64);
}

fn transport_label(t: Transport) -> &'static str {
    match t {
        Transport::JsonRpc => "json_rpc",
        Transport::Grpc => "grpc",
    }
}
