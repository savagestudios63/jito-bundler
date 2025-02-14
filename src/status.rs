//! Typed bundle outcomes.
//!
//! The engine reports bundle state asynchronously: a bundle first enters the
//! `Pending` state on submission, then transitions to either [`Landed`],
//! [`Dropped`], or [`Rejected`]. Callers can either await a terminal state via
//! [`crate::JitoEngine::wait_for_bundle`] or subscribe to the full stream via
//! [`crate::JitoEngine::bundle_status_stream`].
//!
//! [`Landed`]: BundleStatus::Landed
//! [`Dropped`]: BundleStatus::Dropped
//! [`Rejected`]: BundleStatus::Rejected

use serde::{Deserialize, Serialize};

/// Terminal or pending state of a submitted bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BundleStatus {
    /// Engine has accepted the bundle but it has not yet been auctioned.
    Pending {
        /// Bundle id returned at submission time.
        bundle_id: String,
    },
    /// Bundle landed in a block. Always a terminal state.
    Landed {
        /// Bundle id.
        bundle_id: String,
        /// Slot the bundle landed in.
        slot: u64,
        /// Signatures of the transactions in submission order.
        signatures: Vec<String>,
    },
    /// Bundle was dropped (auction lost, tip too low, expired blockhash, etc.).
    Dropped {
        /// Bundle id.
        bundle_id: String,
        /// Engine-reported drop reason.
        reason: String,
    },
    /// Bundle was rejected without ever being auctioned.
    Rejected {
        /// Bundle id, if the engine minted one.
        bundle_id: Option<String>,
        /// Engine-reported rejection reason.
        reason: String,
    },
}

impl BundleStatus {
    /// Returns `true` once the bundle has reached a terminal state.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, BundleStatus::Pending { .. })
    }

    /// Returns the bundle id, if one has been assigned.
    pub fn bundle_id(&self) -> Option<&str> {
        match self {
            BundleStatus::Pending { bundle_id }
            | BundleStatus::Landed { bundle_id, .. }
            | BundleStatus::Dropped { bundle_id, .. } => Some(bundle_id),
            BundleStatus::Rejected { bundle_id, .. } => bundle_id.as_deref(),
        }
    }
}

/// Successful landing outcome returned by [`crate::BundleBuilder::submit`]
/// when the bundle reaches [`BundleStatus::Landed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleOutcome {
    /// Bundle id assigned by the engine.
    pub bundle_id: String,
    /// Slot the bundle landed in.
    pub slot: u64,
    /// Signatures of the transactions, in submission order.
    pub signatures: Vec<String>,
    /// Number of submit attempts it took before the bundle landed. `1`
    /// indicates first-try success.
    pub attempts: u32,
    /// Tip in lamports that eventually landed.
    pub landed_tip_lamports: u64,
}
