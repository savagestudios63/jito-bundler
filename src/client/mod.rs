//! Transport abstraction for the block engine.
//!
//! Two implementations are provided:
//!
//! * [`json_rpc::JsonRpcClient`] — always available, talks HTTP/1.1 + JSON.
//! * [`grpc::GrpcClient`] — behind the `grpc` cargo feature, talks gRPC over
//!   HTTP/2. Typically 30–50 % lower submission latency.
//!
//! The [`BundleClient`] trait is what the rest of the crate consumes. Users of
//! the crate normally never touch these types directly; they configure a
//! transport at [`crate::JitoEngine`] construction time and everything else is
//! wired for them.

use async_trait::async_trait;

use crate::error::Result;
use crate::status::BundleStatus;

pub mod json_rpc;

#[cfg(feature = "grpc")]
pub mod grpc;

/// Transport-agnostic operations the engine requires.
#[async_trait]
pub trait BundleClient: Send + Sync {
    /// Submits a pre-serialized bundle and returns its assigned bundle id.
    ///
    /// `txns_base64` are full transactions, base64-encoded, in submission
    /// order. Tip injection and signing have already happened by this point;
    /// the transport layer is deliberately thin.
    async fn send_bundle(&self, txns_base64: Vec<String>) -> Result<String>;

    /// Fetches the current status for the given bundle id. Polled by the
    /// engine when no streaming status subscription is available.
    async fn bundle_status(&self, bundle_id: &str) -> Result<BundleStatus>;
}

/// Label used in logs and metrics to identify which transport is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Plain JSON-RPC over HTTPS.
    JsonRpc,
    /// Binary gRPC over HTTP/2.
    Grpc,
}
