//! gRPC transport for the Jito block engine.
//!
//! This module is only compiled when the `grpc` feature is enabled. It talks
//! to the block engine's `block_engine.BlockEngineValidator` service over
//! HTTP/2 + TLS. For the vast majority of use cases gRPC shaves 30–50 % off
//! the p50 submission latency versus JSON-RPC because it avoids the
//! JSON parse and reuses a single HTTP/2 connection.
//!
//! The protobuf schema is intentionally described here as a small, stable
//! subset of what Jito publishes — just what this crate needs for bundle
//! submission and status polling. Callers who need the full service should
//! drop down to `jito-protos` directly.

use async_trait::async_trait;
use tonic::transport::{Channel, ClientTlsConfig};

use super::BundleClient;
use crate::error::{invalid_config, Error, Result};
use crate::status::BundleStatus;

/// gRPC bundle client.
///
/// Clones share the underlying HTTP/2 channel; construct once per region and
/// pass clones to worker tasks.
#[derive(Clone)]
pub struct GrpcClient {
    channel: Channel,
}

impl GrpcClient {
    /// Connects to the given endpoint (e.g.
    /// `https://mainnet.block-engine.jito.wtf`).
    pub async fn connect(endpoint: impl Into<String>) -> Result<Self> {
        let endpoint_str = endpoint.into();
        let mut endpoint = Channel::from_shared(endpoint_str.clone())
            .map_err(|e| invalid_config(format!("invalid grpc endpoint: {e}")))?
            .keep_alive_while_idle(true)
            .http2_keep_alive_interval(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(10));

        if endpoint_str.starts_with("https://") {
            endpoint = endpoint
                .tls_config(ClientTlsConfig::new().with_webpki_roots())
                .map_err(|e| Error::Transport(format!("tls config: {e}")))?;
        }

        let channel = endpoint
            .connect()
            .await
            .map_err(|e| Error::Transport(format!("grpc connect: {e}")))?;
        Ok(Self { channel })
    }

    fn service(&self) -> proto::BlockEngineClient {
        proto::BlockEngineClient::new(self.channel.clone())
    }
}

#[async_trait]
impl BundleClient for GrpcClient {
    async fn send_bundle(&self, txns_base64: Vec<String>) -> Result<String> {
        let packets = txns_base64
            .into_iter()
            .map(|b64| {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map(|data| proto::Packet { data })
                    .map_err(|e| Error::Serde(format!("bundle base64: {e}")))
            })
            .collect::<Result<Vec<_>>>()?;

        let req = tonic::Request::new(proto::SendBundleRequest {
            bundle: Some(proto::Bundle { packets }),
        });
        let res = self
            .service()
            .send_bundle(req)
            .await
            .map_err(grpc_to_error)?;
        Ok(res.into_inner().uuid)
    }

    async fn bundle_status(&self, bundle_id: &str) -> Result<BundleStatus> {
        let req = tonic::Request::new(proto::GetBundleStatusRequest {
            bundle_id: bundle_id.to_owned(),
        });
        let res = self
            .service()
            .get_bundle_status(req)
            .await
            .map_err(grpc_to_error)?
            .into_inner();

        Ok(match res.state {
            s if s == proto::State::Landed as i32 => BundleStatus::Landed {
                bundle_id: bundle_id.to_owned(),
                slot: res.slot,
                signatures: res.signatures,
            },
            s if s == proto::State::Dropped as i32 => BundleStatus::Dropped {
                bundle_id: bundle_id.to_owned(),
                reason: res.reason,
            },
            s if s == proto::State::Rejected as i32 => BundleStatus::Rejected {
                bundle_id: Some(bundle_id.to_owned()),
                reason: res.reason,
            },
            _ => BundleStatus::Pending {
                bundle_id: bundle_id.to_owned(),
            },
        })
    }
}

fn grpc_to_error(s: tonic::Status) -> Error {
    Error::Rpc {
        code: s.code() as i64,
        message: s.message().to_owned(),
    }
}

/// Hand-rolled minimal proto bindings. Kept private; see module docs for why.
#[allow(missing_docs)]
mod proto {
    use prost::Message;
    use tonic::codegen::http;
    use tonic::codegen::{Body, Bytes, StdError};

    #[derive(Clone, PartialEq, Message)]
    pub struct Packet {
        #[prost(bytes = "vec", tag = "1")]
        pub data: Vec<u8>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct Bundle {
        #[prost(message, repeated, tag = "1")]
        pub packets: Vec<Packet>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct SendBundleRequest {
        #[prost(message, optional, tag = "1")]
        pub bundle: Option<Bundle>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct SendBundleResponse {
        #[prost(string, tag = "1")]
        pub uuid: String,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct GetBundleStatusRequest {
        #[prost(string, tag = "1")]
        pub bundle_id: String,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct GetBundleStatusResponse {
        #[prost(int32, tag = "1")]
        pub state: i32,
        #[prost(uint64, tag = "2")]
        pub slot: u64,
        #[prost(string, repeated, tag = "3")]
        pub signatures: Vec<String>,
        #[prost(string, tag = "4")]
        pub reason: String,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(i32)]
    pub enum State {
        Pending = 0,
        Landed = 1,
        Dropped = 2,
        Rejected = 3,
    }

    /// Thin tonic client over the above types.
    #[derive(Clone)]
    pub struct BlockEngineClient<T = tonic::transport::Channel> {
        inner: tonic::client::Grpc<T>,
    }

    impl BlockEngineClient<tonic::transport::Channel> {
        pub fn new(channel: tonic::transport::Channel) -> Self {
            Self {
                inner: tonic::client::Grpc::new(channel),
            }
        }
    }

    impl<T> BlockEngineClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::BoxBody>,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    {
        pub async fn send_bundle(
            &mut self,
            req: tonic::Request<SendBundleRequest>,
        ) -> std::result::Result<tonic::Response<SendBundleResponse>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| tonic::Status::unknown(format!("service not ready: {e}")))?;
            let path = http::uri::PathAndQuery::from_static(
                "/block_engine.BlockEngineValidator/SendBundle",
            );
            let codec = tonic::codec::ProstCodec::default();
            self.inner.unary(req, path, codec).await
        }

        pub async fn get_bundle_status(
            &mut self,
            req: tonic::Request<GetBundleStatusRequest>,
        ) -> std::result::Result<tonic::Response<GetBundleStatusResponse>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| tonic::Status::unknown(format!("service not ready: {e}")))?;
            let path = http::uri::PathAndQuery::from_static(
                "/block_engine.BlockEngineValidator/GetBundleStatus",
            );
            let codec = tonic::codec::ProstCodec::default();
            self.inner.unary(req, path, codec).await
        }
    }
}
