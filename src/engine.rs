//! High-level [`JitoEngine`] — the entry point to the crate.
//!
//! Most users create a `JitoEngine` once at process start, clone it across
//! workers, and call [`JitoEngine::bundle`] each time they want to submit.

use std::sync::Arc;
use std::time::Duration;

use solana_sdk::signature::{Keypair, Signer};

use crate::bundle::BundleBuilder;
use crate::client::json_rpc::JsonRpcClient;
use crate::client::{BundleClient, Transport};
use crate::error::Result;
use crate::region::Region;
use crate::retry::RetryPolicy;
use crate::status::BundleStatus;
use crate::tip::{TipOracle, TipStrategy};

/// Top-level handle to a Jito block engine.
///
/// `JitoEngine` is cheap to clone (internally an `Arc`). Spin one up, hand
/// clones to your worker tasks, and submit bundles through each clone without
/// coordination.
///
/// # Quickstart
/// ```no_run
/// use jito_bundler::{JitoEngine, TipStrategy};
/// # use solana_sdk::signature::Keypair;
/// # async fn demo(signed_tx: solana_sdk::transaction::Transaction) -> anyhow::Result<()> {
/// let engine = JitoEngine::auto_region()
///     .await?
///     .with_tip_payer(Keypair::new());
///
/// let outcome = engine
///     .bundle()
///     .add_tx(signed_tx)
///     .tip(TipStrategy::Percentile75)
///     .submit()
///     .await?;
///
/// println!("landed at slot {}", outcome.slot);
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct JitoEngine {
    inner: Arc<EngineInner>,
}

struct EngineInner {
    region: Region,
    client: Arc<dyn BundleClient>,
    transport: Transport,
    tip_oracle: TipOracle,
    tip_payer: Arc<Keypair>,
    default_retry: RetryPolicy,
    default_strategy: TipStrategy,
}

/// Fluent configuration builder for [`JitoEngine`]. Created via
/// [`JitoEngine::builder`].
pub struct JitoEngineBuilder {
    region: Option<Region>,
    transport: Transport,
    tip_oracle: Option<TipOracle>,
    tip_payer: Option<Keypair>,
    retry: RetryPolicy,
    strategy: TipStrategy,
    tip_stream_url: Option<String>,
}

impl JitoEngine {
    /// Starts a builder. Prefer this in production code where you want to
    /// wire a concrete keypair and transport.
    pub fn builder() -> JitoEngineBuilder {
        JitoEngineBuilder {
            region: None,
            transport: Transport::JsonRpc,
            tip_oracle: None,
            tip_payer: None,
            retry: RetryPolicy::default(),
            strategy: TipStrategy::default(),
            tip_stream_url: None,
        }
    }

    /// Runs [`Region::auto_select`] and returns an engine pointed at the
    /// fastest region, using the JSON-RPC transport and an **ephemeral
    /// tip payer** generated on the fly.
    ///
    /// The ephemeral keypair is convenient for tutorials and smoke tests but
    /// holds no lamports, so real submissions will fail at tip-transfer
    /// signing time. For real use, call [`Self::with_tip_payer`] to swap in
    /// your funded keypair.
    pub async fn auto_region() -> Result<Self> {
        let region = Region::auto_select().await?;
        JitoEngineBuilder {
            region: Some(region),
            transport: Transport::JsonRpc,
            tip_oracle: None,
            tip_payer: None,
            retry: RetryPolicy::default(),
            strategy: TipStrategy::default(),
            tip_stream_url: None,
        }
        .build()
        .await
    }

    /// Replaces the tip payer. Consumes `self` and returns a new handle so
    /// chaining with [`Self::auto_region`] reads naturally.
    pub fn with_tip_payer(self, payer: Keypair) -> Self {
        let mut inner = (*self.inner).clone_shallow();
        inner.tip_payer = Arc::new(payer);
        Self {
            inner: Arc::new(inner),
        }
    }

    /// The region this engine is pointing at.
    pub fn region(&self) -> Region {
        self.inner.region
    }

    /// The transport this engine is using.
    pub fn transport(&self) -> Transport {
        self.inner.transport
    }

    /// Access to the tip oracle. In default configurations the oracle starts
    /// empty; if the `tip-stream` feature is enabled and a stream URL was
    /// supplied, background samples are flowing in.
    pub fn tip_oracle(&self) -> &TipOracle {
        &self.inner.tip_oracle
    }

    /// Starts a new [`BundleBuilder`] pre-populated with the engine's
    /// defaults (transport, tip payer, tip strategy, retry policy).
    pub fn bundle(&self) -> BundleBuilder {
        BundleBuilder {
            client: self.inner.client.clone(),
            tip_oracle: self.inner.tip_oracle.clone(),
            tip_payer: self.inner.tip_payer.clone(),
            recent_blockhash: None,
            txs: Vec::new(),
            strategy: self.inner.default_strategy.clone(),
            retry: self.inner.default_retry.clone(),
            tip_account_index: fastrand_account_index(&self.inner.tip_payer),
            poll_interval: Duration::from_millis(400),
            poll_timeout: Duration::from_secs(20),
        }
    }

    /// Low-level: fetches the current status for an arbitrary bundle id.
    /// Useful for out-of-band status lookups (e.g. driving a dashboard).
    pub async fn bundle_status(&self, bundle_id: &str) -> Result<BundleStatus> {
        self.inner.client.bundle_status(bundle_id).await
    }

    /// Polls until the bundle reaches a terminal state or the timeout
    /// elapses. Returns the last observed status (terminal or pending).
    pub async fn wait_for_bundle(
        &self,
        bundle_id: &str,
        timeout: Duration,
    ) -> Result<BundleStatus> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let status = self.inner.client.bundle_status(bundle_id).await?;
            if status.is_terminal() {
                return Ok(status);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

impl EngineInner {
    fn clone_shallow(&self) -> EngineInner {
        EngineInner {
            region: self.region,
            client: self.client.clone(),
            transport: self.transport,
            tip_oracle: self.tip_oracle.clone(),
            tip_payer: self.tip_payer.clone(),
            default_retry: self.default_retry.clone(),
            default_strategy: self.default_strategy.clone(),
        }
    }
}

impl JitoEngineBuilder {
    /// Pins the engine to a specific region, skipping auto-selection.
    pub fn region(mut self, region: Region) -> Self {
        self.region = Some(region);
        self
    }

    /// Selects the transport. `Grpc` requires the `grpc` cargo feature.
    pub fn transport(mut self, transport: Transport) -> Self {
        self.transport = transport;
        self
    }

    /// Supplies the tip-paying keypair. Required before a real submission.
    pub fn tip_payer(mut self, payer: Keypair) -> Self {
        self.tip_payer = Some(payer);
        self
    }

    /// Supplies a pre-built tip oracle. If omitted, a fresh one is created.
    pub fn tip_oracle(mut self, oracle: TipOracle) -> Self {
        self.tip_oracle = Some(oracle);
        self
    }

    /// URL for the tip-stream WebSocket feed (only meaningful with the
    /// `tip-stream` feature).
    pub fn tip_stream_url(mut self, url: impl Into<String>) -> Self {
        self.tip_stream_url = Some(url.into());
        self
    }

    /// Sets the default retry policy applied to new bundles.
    pub fn retry(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }

    /// Sets the default tip strategy applied to new bundles.
    pub fn tip_strategy(mut self, strategy: TipStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Finishes the build, auto-selecting the region if none was pinned.
    pub async fn build(self) -> Result<JitoEngine> {
        let region = match self.region {
            Some(r) => r,
            None => Region::auto_select().await?,
        };

        let client: Arc<dyn BundleClient> = match self.transport {
            Transport::JsonRpc => Arc::new(JsonRpcClient::new(region.json_rpc_url())?),
            #[cfg(feature = "grpc")]
            Transport::Grpc => Arc::new(
                crate::client::grpc::GrpcClient::connect(region.grpc_endpoint()).await?,
            ),
            #[cfg(not(feature = "grpc"))]
            Transport::Grpc => {
                return Err(crate::error::Error::InvalidConfig(
                    "grpc transport requested but the `grpc` feature is disabled".into(),
                ))
            }
        };

        let tip_oracle = self.tip_oracle.unwrap_or_default();

        #[cfg(feature = "tip-stream")]
        {
            let url = self
                .tip_stream_url
                .unwrap_or_else(|| region.tip_stream_url().to_owned());
            let _ = tip_oracle.spawn_stream(&url);
        }
        #[cfg(not(feature = "tip-stream"))]
        {
            let _ = self.tip_stream_url;
        }

        let tip_payer = Arc::new(self.tip_payer.unwrap_or_else(Keypair::new));

        Ok(JitoEngine {
            inner: Arc::new(EngineInner {
                region,
                client,
                transport: self.transport,
                tip_oracle,
                tip_payer,
                default_retry: self.retry,
                default_strategy: self.strategy,
            }),
        })
    }
}

/// Deterministic-but-spread tip account index derived from the payer pubkey.
/// Does not require `rand` — the payer pubkey already hashes well enough.
fn fastrand_account_index(payer: &Keypair) -> usize {
    let bytes = payer.pubkey().to_bytes();
    bytes[0] as usize
}
