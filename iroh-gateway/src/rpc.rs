use async_trait::async_trait;
use iroh_rpc_types::gateway::{Gateway as RpcGateway, GatewayServerAddr, VersionResponse};

use crate::error::Error;

#[derive(Default)]
pub struct Gateway {}

#[async_trait]
impl RpcGateway for Gateway {
    type Error = Error;

    #[tracing::instrument(skip(self))]
    async fn version(&self, _: ()) -> Result<VersionResponse, Self::Error> {
        let version = env!("CARGO_PKG_VERSION").to_string();
        Ok(VersionResponse { version })
    }
}

#[cfg(feature = "grpc")]
impl iroh_rpc_types::NamedService for Gateway {
    const NAME: &'static str = "gateway";
}

pub async fn new(addr: GatewayServerAddr, gateway: Gateway) -> Result<(), Error> {
    iroh_rpc_types::gateway::serve(addr, gateway).await.map_err(Error::from)
}
