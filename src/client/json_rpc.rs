//! JSON-RPC transport for the Jito block engine.
//!
//! Uses `reqwest` with connection pooling. All requests target the engine's
//! `/api/v1/bundles` endpoint, which is the path Jito exposes for bundle
//! submission.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::BundleClient;
use crate::error::{Error, Result};
use crate::status::BundleStatus;

/// JSON-RPC bundle client. Cheaper to construct than `reqwest::Client`, so
/// clone freely.
#[derive(Clone)]
pub struct JsonRpcClient {
    http: reqwest::Client,
    base_url: String,
}

impl JsonRpcClient {
    /// Constructs a client against the given base URL, e.g.
    /// `https://mainnet.block-engine.jito.wtf`. No trailing slash.
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .tcp_keepalive(std::time::Duration::from_secs(60))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.into(),
        })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        #[derive(Serialize)]
        struct Req<'a> {
            jsonrpc: &'static str,
            id: u32,
            method: &'a str,
            params: Value,
        }
        #[derive(Deserialize)]
        struct RpcErr {
            code: i64,
            message: String,
        }
        #[derive(Deserialize)]
        struct Res {
            result: Option<Value>,
            error: Option<RpcErr>,
        }

        let url = format!("{}/api/v1/bundles", self.base_url);
        let body = Req {
            jsonrpc: "2.0",
            id: 1,
            method,
            params,
        };
        let res: Res = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(err) = res.error {
            return Err(Error::Rpc {
                code: err.code,
                message: err.message,
            });
        }
        res.result.ok_or_else(|| Error::Rpc {
            code: -32603,
            message: "empty result".into(),
        })
    }
}

#[async_trait]
impl BundleClient for JsonRpcClient {
    async fn send_bundle(&self, txns_base64: Vec<String>) -> Result<String> {
        let params = serde_json::json!([
            txns_base64,
            { "encoding": "base64" }
        ]);
        let v = self.call("sendBundle", params).await?;
        v.as_str()
            .map(|s| s.to_owned())
            .ok_or_else(|| Error::Rpc {
                code: -32603,
                message: format!("unexpected sendBundle response: {v}"),
            })
    }

    async fn bundle_status(&self, bundle_id: &str) -> Result<BundleStatus> {
        let params = serde_json::json!([[bundle_id]]);
        let v = self.call("getBundleStatuses", params).await?;

        // Jito returns `{ context, value: [ { bundle_id, slot, confirmation_status, transactions, err } ] }`.
        let entry = v
            .get("value")
            .and_then(|x| x.get(0))
            .cloned()
            .unwrap_or(Value::Null);

        if entry.is_null() {
            return Ok(BundleStatus::Pending {
                bundle_id: bundle_id.to_owned(),
            });
        }

        let slot = entry.get("slot").and_then(|x| x.as_u64());
        let confirmation = entry
            .get("confirmation_status")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let err = entry.get("err").cloned().unwrap_or(Value::Null);
        let txs: Vec<String> = entry
            .get("transactions")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_owned()))
                    .collect()
            })
            .unwrap_or_default();

        match (slot, confirmation, err.is_null()) {
            (Some(slot), conf, true)
                if conf == "confirmed" || conf == "finalized" =>
            {
                Ok(BundleStatus::Landed {
                    bundle_id: bundle_id.to_owned(),
                    slot,
                    signatures: txs,
                })
            }
            (_, _, false) => Ok(BundleStatus::Dropped {
                bundle_id: bundle_id.to_owned(),
                reason: err.to_string(),
            }),
            _ => Ok(BundleStatus::Pending {
                bundle_id: bundle_id.to_owned(),
            }),
        }
    }
}
