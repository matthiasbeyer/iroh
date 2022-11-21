use std::{collections::HashSet, str::FromStr, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use cid::{multibase::Base, Cid};
use futures::future::Either;
use iroh_rpc_client::Client;
use rand::seq::SliceRandom;
use reqwest::Url;
use tracing::{info, trace, warn};

use crate::{
    error::Error,
    indexer::Indexer,
    parse_links,
    resolver::{ContextId, LoadedCid, LoaderContext, Source, IROH_STORE},
};

#[async_trait]
pub trait ContentLoader: Sync + Send + std::fmt::Debug + Clone + 'static {
    /// Loads the actual content of a given cid.
    async fn load_cid(&self, cid: &Cid, ctx: &LoaderContext) -> Result<LoadedCid, Error>;
    /// Signal that the passend in session is not used anymore.
    async fn stop_session(&self, ctx: ContextId) -> Result<(), Error>;
    /// Checks if the given cid is present in the local storage.
    async fn has_cid(&self, cid: &Cid) -> Result<bool, Error>;
}

#[async_trait]
impl<T: ContentLoader> ContentLoader for Arc<T> {
    async fn load_cid(&self, cid: &Cid, ctx: &LoaderContext) -> Result<LoadedCid, Error> {
        self.as_ref().load_cid(cid, ctx).await
    }

    async fn stop_session(&self, ctx: ContextId) -> Result<(), Error> {
        self.as_ref().stop_session(ctx).await
    }

    async fn has_cid(&self, cid: &Cid) -> Result<bool, Error> {
        self.as_ref().has_cid(cid).await
    }
}

#[derive(Debug, Clone)]
pub struct FullLoader {
    /// RPC Client.
    client: Client,
    /// API to talk to the indexer nodes.
    indexer: Option<Indexer>,
    /// Gateway endpoints.
    http_gateways: Vec<GatewayUrl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullLoaderConfig {
    pub indexer: Option<Url>,
    pub http_gateways: Vec<GatewayUrl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayUrl {
    Full(Url),
    Subdomain(String),
}

impl FromStr for GatewayUrl {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.starts_with("http") || input.starts_with("https") {
            let url = input.parse()?;
            return Ok(GatewayUrl::Full(url));
        }

        Ok(GatewayUrl::Subdomain(input.to_string()))
    }
}

impl GatewayUrl {
    pub fn as_string(&self) -> String {
        match self {
            GatewayUrl::Full(url) => url.to_string(),
            GatewayUrl::Subdomain(s) => s.clone(),
        }
    }

    pub fn as_url(&self, cid: &Cid) -> Result<Url, Error> {
        let cid_str = cid.into_v1()?.to_string_of_base(Base::Base32Lower)?;
        let url = match self {
            GatewayUrl::Full(raw) => {
                let mut url = raw.join(&cid_str).unwrap();
                url.set_query(Some("format=raw"));
                url
            }
            GatewayUrl::Subdomain(raw) => {
                format!("https://{}.ipfs.{}?format=raw", cid_str, raw).parse()?
            }
        };
        Ok(url)
    }
}

impl FullLoader {
    pub fn new(client: Client, config: FullLoaderConfig) -> Result<Self, Error> {
        let indexer = config.indexer.map(Indexer::new).transpose()?;

        Ok(Self {
            client,
            indexer,
            http_gateways: config.http_gateways,
        })
    }

    /// Fetch the next gateway url, if configured.
    async fn next_gateway(&self) -> Option<&GatewayUrl> {
        // TODO: maybe roundrobin?
        if self.http_gateways.is_empty() {
            return None;
        }
        let gw = self.http_gateways.choose(&mut rand::thread_rng()).unwrap();
        Some(gw)
    }

    async fn fetch_store(&self, cid: &Cid) -> Result<Option<LoadedCid>, Error> {
        match self.client.try_store() {
            Ok(store) => Ok(store.get(*cid).await?.map(|data| LoadedCid {
                data,
                source: Source::Store(IROH_STORE),
            })),
            Err(err) => {
                info!("No store available: {:?}", err);
                Ok(None)
            }
        }
    }

    async fn fetch_bitswap(&self, ctx: ContextId, cid: &Cid) -> Result<Option<LoadedCid>, Error> {
        match self.client.try_p2p() {
            Ok(p2p) => {
                let providers: HashSet<_> = if let Some(ref indexer) = self.indexer {
                    if let Ok(providers) = indexer.find_providers(*cid).await {
                        providers.into_iter().map(|p| p.id).collect()
                    } else {
                        Default::default()
                    }
                } else {
                    Default::default()
                };

                let data = p2p.fetch_bitswap(ctx.into(), *cid, providers).await?;
                Ok(Some(LoadedCid {
                    data,
                    source: Source::Bitswap,
                }))
            }
            Err(err) => {
                info!("No p2p available: {:?}", err);
                Ok(None)
            }
        }
    }

    async fn fetch_gateway(&self, cid: &Cid) -> Result<Option<LoadedCid>, Error> {
        match self.next_gateway().await {
            Some(url) => {
                let response = reqwest::get(url.as_url(cid)?).await?;
                // Filter out non http 200 responses.
                if !response.status().is_success() {
                    return Err(Error::UnexpectedHttpStatus)
                }
                let data = response.bytes().await?;
                // Make sure the content is not tampered with.
                if iroh_util::verify_hash(cid, &data) == Some(true) {
                    Ok(Some(LoadedCid {
                        data,
                        source: Source::Http(url.as_string()),
                    }))
                } else {
                    Err(Error::InvalidHash)
                }
            }
            None => Ok(None),
        }
    }

    fn store_data(&self, cid: Cid, data: Bytes) {
        // trigger storage in the background
        let store = self.client.try_store();
        let p2p = self.client.try_p2p();

        tokio::spawn(async move {
            let links = tokio::task::spawn_blocking({
                let data = data.clone();
                move || parse_links(&cid, &data).unwrap_or_default()
            })
            .await
            .unwrap_or_default();

            if let Ok(store_rpc) = store {
                match store_rpc.put(cid, data.clone(), links).await {
                    Ok(_) => {
                        // Notify bitswap about new blocks
                        if let Ok(p2p) = p2p {
                            p2p.notify_new_blocks_bitswap(vec![(cid, data)]).await.ok();
                        }
                    }
                    Err(err) => {
                        warn!("failed to store {}: {:?}", cid, err);
                    }
                }
            } else {
                warn!("failed to store: missing store rpc conn");
            }
        });
    }
}

#[async_trait]
impl ContentLoader for FullLoader {
    async fn stop_session(&self, ctx: ContextId) -> Result<(), Error> {
        self.client
            .try_p2p()?
            .stop_session_bitswap(ctx.into())
            .await?;
        Ok(())
    }

    async fn load_cid(&self, cid: &Cid, ctx: &LoaderContext) -> Result<LoadedCid, Error> {
        trace!("{:?} loading {}", ctx.id(), cid);

        if let Some(loaded) = self.fetch_store(cid).await? {
            return Ok(loaded);
        }

        let bitswap_future = self.fetch_bitswap(ctx.id(), cid);
        let gateway_future = self.fetch_gateway(cid);

        tokio::pin!(bitswap_future);
        tokio::pin!(gateway_future);

        let res = futures::future::select(bitswap_future, gateway_future).await;
        let loaded = match res {
            Either::Left((bitswap, gateway_fut)) => {
                if let Ok(Some(loaded)) = bitswap {
                    loaded
                } else {
                    gateway_fut
                        .await?
                        .ok_or_else(|| Error::FailedToFind(*cid))?
                }
            }
            Either::Right((gateway, bitswap_future)) => {
                if let Ok(Some(loaded)) = gateway {
                    loaded
                } else {
                    bitswap_future
                        .await?
                        .ok_or_else(|| Error::FailedToFind(*cid))?
                }
            }
        };

        self.store_data(*cid, loaded.data.clone());
        Ok(loaded)
    }

    async fn has_cid(&self, cid: &Cid) -> Result<bool, Error> {
        self.client
            .try_store()?
            .has(*cid)
            .await
            .map_err(Error::from)
    }
}
