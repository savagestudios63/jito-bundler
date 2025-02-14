//! Error types for the `jito-bundler` crate.
//!
//! Every fallible public API returns [`Result<T, Error>`]. The [`Error`] enum
//! groups failures into a small, stable set of variants so callers can pattern
//! match without worrying about the exact transport in use.
//!
//! ```no_run
//! use jito_bundler::{Error, JitoEngine};
//!
//! # async fn demo() {
//! # let engine = JitoEngine::auto_region().await.unwrap();
//! # let bundle = engine.bundle();
//! match bundle.submit().await {
//!     Ok(result) => println!("landed: {result:?}"),
//!     Err(Error::Rejected { reason, .. }) => eprintln!("rejected: {reason}"),
//!     Err(Error::Transport(e)) => eprintln!("transport: {e}"),
//!     Err(e) => eprintln!("other: {e}"),
//! }
//! # }
//! ```

use std::fmt;

/// Shorthand for `Result<T, jito_bundler::Error>`.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced by this crate.
///
/// Transport-specific failures (HTTP, gRPC, WebSocket) all collapse into
/// [`Error::Transport`], so callers don't have to pivot on the configured
/// transport. Bundle-level outcomes (drops, rejections) are surfaced as
/// dedicated variants so retry code can target them precisely.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A network-level failure while talking to the block engine.
    #[error("transport error: {0}")]
    Transport(String),

    /// The engine accepted the bundle over the wire, but the JSON-RPC or gRPC
    /// layer returned an application-level error (e.g. malformed bundle).
    #[error("rpc error {code}: {message}")]
    Rpc {
        /// JSON-RPC or gRPC status code.
        code: i64,
        /// Engine-supplied message.
        message: String,
    },

    /// The bundle was dropped before landing on-chain. Typically retriable;
    /// the retry layer will bump the tip and resubmit automatically unless the
    /// caller disabled retries.
    #[error("bundle {bundle_id} dropped: {reason}")]
    Dropped {
        /// Bundle id returned by the engine on submission.
        bundle_id: String,
        /// Engine-provided drop reason.
        reason: String,
    },

    /// The bundle was rejected outright (bad signature, blockhash too old,
    /// simulation failure, etc.). Not retriable without changing the bundle.
    #[error("bundle rejected: {reason}")]
    Rejected {
        /// Bundle id, if the engine assigned one before rejecting.
        bundle_id: Option<String>,
        /// Human-readable rejection reason.
        reason: String,
    },

    /// The retry policy exhausted all attempts.
    #[error("retries exhausted after {attempts} attempts: {last}")]
    RetriesExhausted {
        /// Number of attempts made.
        attempts: u32,
        /// The terminal error (usually the last drop).
        last: Box<Error>,
    },

    /// A bundle-status subscription ended before the bundle reached a terminal
    /// state.
    #[error("status stream closed before terminal state")]
    StatusStreamClosed,

    /// The tip oracle has not yet received enough samples to answer the query.
    /// Use [`crate::TipOracle::warmup`] to await sufficient data.
    #[error("tip oracle has not warmed up (only {samples} samples)")]
    TipOracleCold {
        /// Number of samples currently held.
        samples: usize,
    },

    /// A bundle must contain at least one user transaction and leave room
    /// for the injected tip transaction. With the engine's 5-tx cap, that
    /// means 1–4 user transactions.
    #[error("invalid bundle size {actual}: must be between 1 and 4 user transactions")]
    InvalidBundleSize {
        /// The offending size.
        actual: usize,
    },

    /// A value supplied by the caller failed local validation before any
    /// network call was attempted.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// Serialization or deserialization of an on-wire payload failed.
    #[error("serde error: {0}")]
    Serde(String),
}

impl Error {
    /// Returns `true` if retrying the same bundle (optionally with a higher
    /// tip) is expected to make progress.
    ///
    /// The retry loop uses this to decide whether to back off or bail out.
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            Error::Transport(_) | Error::Dropped { .. } | Error::StatusStreamClosed
        )
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Transport(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Serde(e.to_string())
    }
}

impl From<url::ParseError> for Error {
    fn from(e: url::ParseError) -> Self {
        Error::InvalidConfig(format!("url parse: {e}"))
    }
}

/// Convenience for returning an [`Error::InvalidConfig`] with a formatted
/// message.
pub(crate) fn invalid_config(msg: impl fmt::Display) -> Error {
    Error::InvalidConfig(msg.to_string())
}
