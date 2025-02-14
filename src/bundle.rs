//! Fluent bundle builder.
//!
//! A [`BundleBuilder`] is obtained from [`crate::JitoEngine::bundle`] and
//! accumulates signed transactions plus a tip policy. When [`submit`] is
//! called, the builder:
//!
//! 1. Validates the bundle size (1–5 transactions),
//! 2. Resolves the tip strategy into a concrete lamport amount,
//! 3. Injects a tip transfer instruction (wrapped as its own transaction,
//!    signed by the configured payer),
//! 4. Hands the fully-signed bundle to the transport,
//! 5. Tracks the bundle's status and retries on drops per the configured
//!    [`RetryPolicy`].
//!
//! Because the library accepts **already-signed** payer transactions, the
//! builder needs a separate keypair purely for the tip transaction. That
//! keypair is provided once on the engine ([`crate::JitoEngine`]) rather than
//! per bundle.
//!
//! [`submit`]: BundleBuilder::submit

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;

use crate::client::BundleClient;
use crate::error::{invalid_config, Error, Result};
use crate::retry::RetryPolicy;
use crate::status::{BundleOutcome, BundleStatus};
use crate::tip::{TipAccount, TipOracle, TipStrategy};

/// Upper bound imposed by the block engine.
pub const MAX_BUNDLE_SIZE: usize = 5;

/// A fluent builder that accumulates signed transactions and submits them as
/// an atomic bundle.
///
/// The builder is consuming: each method returns `self` by value. This lets
/// the compiler reject misuse (e.g. mutating after submit) and keeps the
/// call-site clean.
///
/// # Example
/// ```no_run
/// use jito_bundler::{JitoEngine, TipStrategy};
/// # use solana_sdk::transaction::Transaction;
/// # async fn demo(signed_tx: Transaction) -> anyhow::Result<()> {
/// let engine = JitoEngine::auto_region().await?;
/// let outcome = engine
///     .bundle()
///     .add_tx(signed_tx)
///     .tip(TipStrategy::Percentile75)
///     .max_attempts(3)
///     .submit()
///     .await?;
/// println!("landed at slot {}", outcome.slot);
/// # Ok(()) }
/// ```
pub struct BundleBuilder {
    pub(crate) client: Arc<dyn BundleClient>,
    pub(crate) tip_oracle: TipOracle,
    pub(crate) tip_payer: Arc<Keypair>,
    pub(crate) recent_blockhash: Option<Hash>,
    pub(crate) txs: Vec<Transaction>,
    pub(crate) strategy: TipStrategy,
    pub(crate) retry: RetryPolicy,
    pub(crate) tip_account_index: usize,
    pub(crate) poll_interval: Duration,
    pub(crate) poll_timeout: Duration,
}

impl BundleBuilder {
    /// Appends a signed transaction to the bundle.
    ///
    /// Transactions are replayed in the order they are added; the engine
    /// treats the bundle as all-or-nothing.
    pub fn add_tx(mut self, tx: Transaction) -> Self {
        self.txs.push(tx);
        self
    }

    /// Appends multiple signed transactions at once.
    pub fn add_txs(mut self, txs: impl IntoIterator<Item = Transaction>) -> Self {
        self.txs.extend(txs);
        self
    }

    /// Sets the tip strategy.
    pub fn tip(mut self, strategy: TipStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Convenience: sets [`TipStrategy::Fixed`].
    pub fn fixed_tip(mut self, lamports: u64) -> Self {
        self.strategy = TipStrategy::Fixed(lamports);
        self
    }

    /// Overrides the retry policy.
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }

    /// Shorthand for `retry_policy(RetryPolicy::default().max_attempts(n))`.
    pub fn max_attempts(mut self, n: u32) -> Self {
        self.retry = self.retry.max_attempts(n);
        self
    }

    /// Selects which of the eight tip receivers to pay. Defaults to picking
    /// by payer pubkey hash so long-running bots naturally spread across
    /// accounts.
    pub fn tip_account_index(mut self, index: usize) -> Self {
        self.tip_account_index = index;
        self
    }

    /// Supplies a recent blockhash for the tip transaction. If not set, the
    /// builder copies the blockhash from the last user transaction in the
    /// bundle — normally what you want.
    pub fn recent_blockhash(mut self, hash: Hash) -> Self {
        self.recent_blockhash = Some(hash);
        self
    }

    /// Interval between `bundle_status` polls while waiting for a terminal
    /// state. Default 400 ms.
    pub fn poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Per-attempt wall clock budget for reaching a terminal state. Drops
    /// after this point are treated as retriable. Default 20 seconds.
    pub fn poll_timeout(mut self, d: Duration) -> Self {
        self.poll_timeout = d;
        self
    }

    /// Submits the bundle and drives it to a terminal state, retrying on
    /// drops per the configured [`RetryPolicy`].
    ///
    /// Returns [`BundleOutcome`] on land, or an [`Error`] on the terminal
    /// failure of the last attempt.
    pub async fn submit(self) -> Result<BundleOutcome> {
        self.retry.validate()?;
        if self.txs.is_empty() || self.txs.len() > MAX_BUNDLE_SIZE - 1 {
            // MAX_BUNDLE_SIZE - 1 accounts for the injected tip tx.
            return Err(Error::InvalidBundleSize {
                actual: self.txs.len(),
            });
        }

        let BundleBuilder {
            client,
            tip_oracle,
            tip_payer,
            recent_blockhash,
            txs,
            strategy,
            retry,
            tip_account_index,
            poll_interval,
            poll_timeout,
        } = self;

        let max_attempts = retry.max_attempts_get();
        let mut last_err: Option<Error> = None;
        let mut tip_lamports = resolve_tip(&strategy, &tip_oracle).await?;
        let tip_account = TipAccount::pick(tip_account_index);
        let blockhash = recent_blockhash
            .or_else(|| txs.last().map(|t| t.message.recent_blockhash))
            .ok_or_else(|| invalid_config("no recent blockhash available"))?;

        for attempt in 1..=max_attempts {
            let delay = retry.backoff_for(attempt);
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }

            let tip_tx = build_tip_tx(&tip_payer, tip_account, tip_lamports, blockhash);
            let wire = encode_bundle(&txs, &tip_tx)?;

            tracing::debug!(
                target: "jito_bundler::bundle",
                attempt,
                tip_lamports,
                tip_account = %tip_account.0,
                "submitting bundle"
            );

            let bundle_id = match client.send_bundle(wire).await {
                Ok(id) => id,
                Err(e) if e.is_retriable() && attempt < max_attempts => {
                    last_err = Some(e);
                    continue;
                }
                Err(e) => return Err(e),
            };

            match await_terminal(&*client, &bundle_id, poll_interval, poll_timeout).await {
                Ok(BundleStatus::Landed {
                    bundle_id,
                    slot,
                    signatures,
                }) => {
                    return Ok(BundleOutcome {
                        bundle_id,
                        slot,
                        signatures,
                        attempts: attempt,
                        landed_tip_lamports: tip_lamports,
                    })
                }
                Ok(BundleStatus::Rejected { bundle_id, reason }) => {
                    return Err(Error::Rejected { bundle_id, reason })
                }
                Ok(BundleStatus::Dropped { bundle_id, reason }) => {
                    last_err = Some(Error::Dropped {
                        bundle_id,
                        reason: reason.clone(),
                    });
                    tip_lamports = retry.bump_tip(tip_lamports);
                    tracing::info!(
                        target: "jito_bundler::bundle",
                        reason = %reason,
                        next_tip = tip_lamports,
                        "bundle dropped; retrying"
                    );
                    continue;
                }
                Ok(BundleStatus::Pending { .. }) => {
                    // Poll timed out; treat as drop for retry purposes.
                    last_err = Some(Error::Dropped {
                        bundle_id: bundle_id.clone(),
                        reason: "poll timeout".into(),
                    });
                    tip_lamports = retry.bump_tip(tip_lamports);
                    continue;
                }
                Err(e) if e.is_retriable() && attempt < max_attempts => {
                    last_err = Some(e);
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Err(Error::RetriesExhausted {
            attempts: max_attempts,
            last: Box::new(last_err.unwrap_or(Error::StatusStreamClosed)),
        })
    }
}

async fn resolve_tip(strategy: &TipStrategy, oracle: &TipOracle) -> Result<u64> {
    match strategy {
        TipStrategy::Fixed(n) => Ok(*n),
        TipStrategy::Percentile50 => oracle.p50().await,
        TipStrategy::Percentile75 => oracle.p75().await,
        TipStrategy::Percentile95 => oracle.p95().await,
        TipStrategy::Dynamic(f) => Ok(f()),
    }
}

fn build_tip_tx(
    payer: &Keypair,
    account: TipAccount,
    lamports: u64,
    blockhash: Hash,
) -> Transaction {
    let ix: Instruction =
        system_instruction::transfer(&payer.pubkey(), &account.0, lamports);
    let msg = Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &blockhash);
    let mut tx = Transaction::new_unsigned(msg);
    tx.sign(&[payer], blockhash);
    tx
}

fn encode_bundle(user: &[Transaction], tip: &Transaction) -> Result<Vec<String>> {
    let engine = base64::engine::general_purpose::STANDARD;
    let mut out = Vec::with_capacity(user.len() + 1);
    for tx in user.iter().chain(std::iter::once(tip)) {
        let bytes = bincode::serialize(tx)
            .map_err(|e| Error::Serde(format!("bincode: {e}")))?;
        out.push(engine.encode(bytes));
    }
    Ok(out)
}

async fn await_terminal(
    client: &dyn BundleClient,
    bundle_id: &str,
    interval: Duration,
    timeout: Duration,
) -> Result<BundleStatus> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let status = client.bundle_status(bundle_id).await?;
        if status.is_terminal() {
            return Ok(status);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(status); // still pending
        }
        tokio::time::sleep(interval).await;
    }
}

