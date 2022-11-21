use std::io::Cursor;

use async_trait::async_trait;
use bytes::BytesMut;
use cid::Cid;
use iroh_rpc_types::store::{
    GetLinksRequest, GetLinksResponse, GetRequest, GetResponse, GetSizeRequest, GetSizeResponse,
    HasRequest, HasResponse, PutManyRequest, PutRequest, Store as RpcStore, StoreServerAddr,
    VersionResponse,
};
use tracing::info;

use crate::error::Error;
use crate::store::Store;

#[cfg(feature = "rpc-grpc")]
impl iroh_rpc_types::NamedService for Store {
    const NAME: &'static str = "store";
}

#[async_trait]
impl RpcStore for Store {
    type Error = Error;

    #[tracing::instrument(skip(self))]
    async fn version(&self, _: ()) -> Result<VersionResponse, Self::Error> {
        let version = env!("CARGO_PKG_VERSION").to_string();
        Ok(VersionResponse { version })
    }

    #[tracing::instrument(skip(self, req))]
    async fn put(&self, req: PutRequest) -> Result<(), Self::Error> {
        let cid = cid_from_bytes(req.cid)?;
        let links = links_from_bytes(req.links)?;
        let res = self
            .spawn_blocking(move |x| x.put(cid, req.blob, links))
            .await?;

        info!("store rpc call: put cid {}", cid);
        Ok(res)
    }

    #[tracing::instrument(skip(self, req))]
    async fn put_many(&self, req: PutManyRequest) -> Result<(), Self::Error> {
        let req = req
            .blocks
            .into_iter()
            .map(|req| {
                let cid = cid_from_bytes(req.cid)?;
                let links = links_from_bytes(req.links)?;
                Ok((cid, req.blob, links))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        self.spawn_blocking(move |x| x.put_many(req)).await
    }

    #[tracing::instrument(skip(self))]
    async fn get(&self, req: GetRequest) -> Result<GetResponse, Self::Error> {
        let cid = cid_from_bytes(req.cid)?;
        self.spawn_blocking(move |x| {
            if let Some(res) = x.get(&cid)? {
                Ok(GetResponse {
                    data: Some(BytesMut::from(&res[..]).freeze()),
                })
            } else {
                Ok(GetResponse { data: None })
            }
        })
        .await
    }

    #[tracing::instrument(skip(self))]
    async fn has(&self, req: HasRequest) -> Result<HasResponse, Self::Error> {
        let cid = cid_from_bytes(req.cid)?;
        self.spawn_blocking(move |self| {
            let has = self.has(&cid)?;
            Ok(HasResponse { has })
        })
        .await
    }

    #[tracing::instrument(skip(self))]
    async fn get_links(&self, req: GetLinksRequest) -> Result<GetLinksResponse, Self::Error> {
        let cid = cid_from_bytes(req.cid)?;
        self.spawn_blocking(move |self| {
            if let Some(res) = self.get_links(&cid)? {
                let links = res.into_iter().map(|cid| cid.to_bytes()).collect();
                Ok(GetLinksResponse { links })
            } else {
                Ok(GetLinksResponse { links: Vec::new() })
            }
        })
        .await
    }

    #[tracing::instrument(skip(self))]
    async fn get_size(&self, req: GetSizeRequest) -> Result<GetSizeResponse, Self::Error> {
        let cid = cid_from_bytes(req.cid)?;
        self.spawn_blocking(move |self| {
            if let Some(size) = self.get_size(&cid)? {
                Ok(GetSizeResponse {
                    size: Some(size as u64),
                })
            } else {
                Ok(GetSizeResponse { size: None })
            }
        })
        .await
    }
}

#[tracing::instrument(skip(store))]
pub async fn new(addr: StoreServerAddr, store: Store) -> Result<(), Error> {
    info!("rpc listening on: {}", addr);
    iroh_rpc_types::store::serve(addr, store)
        .await
        .map_err(Error::from)
}

#[tracing::instrument]
fn cid_from_bytes(b: Vec<u8>) -> Result<Cid, Error> {
    Cid::read_bytes(Cursor::new(b)).map_err(Error::from)
}

#[tracing::instrument]
fn links_from_bytes(l: Vec<Vec<u8>>) -> Result<Vec<Cid>, Error> {
    l.into_iter().map(cid_from_bytes).collect()
}
