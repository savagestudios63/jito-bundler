//! # jito-bundler
//!
//! Ergonomic Rust client for [Jito](https://jito.wtf) block-engine bundles.
//!
//! This crate wraps Jito's JSON-RPC and gRPC surfaces behind a single
//! fluent API. It handles the tedious parts of bundle submission — region
//! selection, tip injection, retries, status tracking — so application code
//! can focus on building transactions.
//!
//! ## Feature flags
//!
//! | flag | default | what it enables |
//! |---|---|---|
//! | `tip-stream` | ✅ | WebSocket tip-stream oracle |
//! | `grpc`       | ❌ | gRPC transport (faster; pulls in `tonic` + `prost`) |
//! | `metrics`    | ❌ | Prometheus metrics under `jito_bundler_*` |
//!
//! ## 30-second tour
//!
//! ```no_run
//! use jito_bundler::{JitoEngine, TipStrategy};
//! # use solana_sdk::{signature::Keypair, transaction::Transaction};
//! # async fn demo(signed_tx: Transaction, funded: Keypair) -> anyhow::Result<()> {
//! // 1. Pick the fastest region and connect.
//! let engine = JitoEngine::auto_region()
//!     .await?
//!     .with_tip_payer(funded);
//!
//! // 2. Ask the tip oracle what a competitive p75 tip looks like right now.
//! let tip = engine.tip_oracle().p75().await?;
//!
//! // 3. Build, submit, wait.
//! let outcome = engine
//!     .bundle()
//!     .add_tx(signed_tx)
//!     .tip(TipStrategy::Fixed(tip))
//!     .max_attempts(3)
//!     .submit()
//!     .await?;
//!
//! println!("landed at slot {} (attempt {})", outcome.slot, outcome.attempts);
//! # Ok(()) }
//! ```
//!
//! ## Design notes
//!
//! * **Transport-agnostic.** The [`BundleBuilder`] only knows about a
//!   [`client::BundleClient`] trait object. Swapping JSON-RPC for gRPC
//!   doesn't touch submission code.
//! * **Cheap clones.** [`JitoEngine`], [`tip::TipOracle`], and the gRPC
//!   channel are all `Arc`-based, so per-task workers share a single
//!   connection pool.
//! * **No global state.** Multiple engines — possibly pointed at different
//!   regions — can coexist in one process.
//!
//! See the [`examples/`](https://github.com/jito-bundler/jito-bundler/tree/main/examples)
//! directory for end-to-end runnable code.

#![deny(missing_docs, rustdoc::broken_intra_doc_links)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod bundle;
pub mod client;
pub mod engine;
pub mod error;
#[cfg(feature = "metrics")]
pub mod metrics;
pub mod region;
pub mod retry;
pub mod status;
pub mod tip;

pub use bundle::{BundleBuilder, MAX_BUNDLE_SIZE};
pub use client::Transport;
pub use engine::{JitoEngine, JitoEngineBuilder};
pub use error::{Error, Result};
pub use region::Region;
pub use retry::RetryPolicy;
pub use status::{BundleOutcome, BundleStatus};
pub use tip::{TipAccount, TipOracle, TipStrategy};
