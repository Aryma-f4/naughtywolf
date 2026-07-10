//! Sliver gRPC connection management -- mTLS + bearer token authentication.

use std::path::Path;

use tonic::metadata::AsciiMetadataValue;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use crate::sliver::profiles::SliverCfg;
use crate::sliver::proto::rpcpb::sliver_rpc_client::SliverRpcClient;

/// An established gRPC connection to a Sliver server.
pub struct SliverConnection {
    pub client: SliverRpcClient<InterceptedService<Channel, AuthInterceptor>>,
    pub profile: SliverCfg,
}

/// Interceptor that injects the bearer token into each gRPC call.
#[derive(Clone)]
pub struct AuthInterceptor {
    token: AsciiMetadataValue,
}

impl AuthInterceptor {
    pub fn new(token: &str) -> Self {
        Self {
            token: format!("Bearer {}", token).parse().unwrap(),
        }
    }
}

impl tonic::service::Interceptor for AuthInterceptor {
    fn call(&mut self, mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        req.metadata_mut()
            .insert("authorization", self.token.clone());
        Ok(req)
    }
}

impl SliverConnection {
    /// Connect to a Sliver server using the mTLS credentials and bearer token
    /// from the given profile.
    pub async fn connect(profile: &SliverCfg) -> Result<Self, ConnectionError> {
        // Parse mTLS identity
        let identity = Identity::from_pem(
            profile.certificate.as_bytes(),
            profile.private_key.as_bytes(),
        );

        let tls = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(profile.ca_certificate.as_bytes()))
            .identity(identity)
            .domain_name("multiplayer");

        let addr = format!("{}:{}", profile.lhost, profile.lport);

        let channel = Channel::from_shared(format!("https://{}", addr))
            .map_err(|e| ConnectionError::Address(format!("Invalid address {}: {}", addr, e)))?
            .tls_config(tls)
            .map_err(|e| ConnectionError::Tls(format!("TLS config for {}: {}", addr, e)))?
            .connect_timeout(std::time::Duration::from_secs(10))
            .connect()
            .await
            .map_err(|e| {
                let err_msg = format!("Failed to connect to {}: {}", addr, e);
                tracing::error!("{}", err_msg);
                ConnectionError::Connect(err_msg)
            })?;

        let interceptor = AuthInterceptor::new(&profile.token);
        let client = SliverRpcClient::with_interceptor(channel, interceptor)
            .max_decoding_message_size(100 * 1024 * 1024);

        Ok(Self {
            client,
            profile: profile.clone(),
        })
    }

    /// Connect using a profile loaded from a .cfg file path.
    pub async fn from_config(path: &Path) -> Result<Self, ConnectionError> {
        let cfg =
            SliverCfg::from_file(path).map_err(|e| ConnectionError::Profile(e.to_string()))?;
        Self::connect(&cfg).await
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("Address error: {0}")]
    Address(String),
    #[error("TLS error: {0}")]
    Tls(String),
    #[error("Connect error: {0}")]
    Connect(String),
    #[error("Profile error: {0}")]
    Profile(String),
}
