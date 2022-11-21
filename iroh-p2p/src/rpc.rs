use std::collections::{HashMap, HashSet};
use std::io;
use std::pin::Pin;

use bytes::Bytes;
use cid::Cid;
use futures::{Stream, StreamExt};
use libp2p::gossipsub::{
    error::{PublishError, SubscriptionError},
    MessageId, TopicHash,
};
use libp2p::identify::Info as IdentifyInfo;
use libp2p::kad::record::Key;
use libp2p::Multiaddr;
use libp2p::PeerId;
use tokio::sync::mpsc::{channel, Sender};
use tokio::sync::oneshot;
use tracing::{debug, trace};

use async_trait::async_trait;
use iroh_bitswap::Block;
use iroh_rpc_types::p2p::{
    BitswapRequest, BitswapResponse, ConnectByPeerIdRequest, ConnectRequest, DisconnectRequest,
    GetListeningAddrsResponse, GetPeersResponse, GossipsubAllPeersResponse, GossipsubPeerAndTopics,
    GossipsubPeerIdMsg, GossipsubPeersResponse, GossipsubPublishRequest, GossipsubPublishResponse,
    GossipsubSubscribeResponse, GossipsubTopicHashMsg, GossipsubTopicsResponse, Key as ProviderKey,
    LookupRequest, Multiaddrs, NotifyNewBlocksBitswapRequest, P2p as RpcP2p, P2pServerAddr,
    PeerIdResponse, PeerInfo, Providers, StopSessionBitswapRequest, VersionResponse,
};

use super::node::DEFAULT_PROVIDER_LIMIT;

use crate::error::Error;

struct P2p {
    sender: Sender<RpcMessage>,
}

#[async_trait]
impl RpcP2p for P2p {
    type Error = crate::error::Error;

    #[tracing::instrument(skip(self))]
    async fn version(&self, _: ()) -> Result<VersionResponse, Self::Error> {
        let version = env!("CARGO_PKG_VERSION").to_string();
        Ok(VersionResponse { version })
    }

    #[tracing::instrument(skip(self))]
    async fn shutdown(&self, _: ()) -> Result<(), Self::Error> {
        self.sender.send(RpcMessage::Shutdown).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn external_addrs(&self, _: ()) -> Result<Multiaddrs, Self::Error> {
        trace!("received ExternalAddrs request");

        let (s, r) = oneshot::channel();
        let msg = RpcMessage::ExternalAddrs(s);

        self.sender.send(msg).await?;

        let addrs = r.await?;

        Ok(Multiaddrs {
            addrs: addrs.into_iter().map(|addr| addr.to_vec()).collect(),
        })
    }

    #[tracing::instrument(skip(self))]
    async fn listeners(&self, _: ()) -> Result<Multiaddrs, Self::Error> {
        trace!("received Listeners request");

        let (s, r) = oneshot::channel();
        let msg = RpcMessage::Listeners(s);

        self.sender.send(msg).await?;

        let addrs = r.await?;

        Ok(Multiaddrs {
            addrs: addrs.into_iter().map(|addr| addr.to_vec()).collect(),
        })
    }

    #[tracing::instrument(skip(self))]
    async fn local_peer_id(&self, _: ()) -> Result<PeerIdResponse, Self::Error> {
        trace!("received LocalPeerId request");

        let (s, r) = oneshot::channel();
        let msg = RpcMessage::LocalPeerId(s);

        self.sender.send(msg).await?;

        let peer_id = r.await?;

        Ok(PeerIdResponse {
            peer_id: peer_id.to_bytes(),
        })
    }

    // TODO: expand to handle multiple cids at once. Probably not a tough fix, just want to push
    // forward right now
    #[tracing::instrument(skip(self, req))]
    async fn fetch_bitswap(&self, req: BitswapRequest) -> Result<BitswapResponse, Self::Error> {
        let ctx = req.ctx;
        let cid = Cid::read_bytes(io::Cursor::new(req.cid))?;

        trace!("context:{}, received fetch_bitswap: {:?}", ctx, cid);
        let providers = req.providers.ok_or_else(|| Error::MissingProviders(cid))?;

        let providers: HashSet<PeerId> = providers
            .providers
            .into_iter()
            .map(|p| PeerId::from_bytes(&p).map_err(Error::from))
            .collect::<Result<_, Self::Error>>()?;

        let (s, r) = oneshot::channel();
        let msg = RpcMessage::BitswapRequest {
            ctx,
            cids: vec![cid],
            providers,
            response_channels: vec![s],
        };

        trace!("context:{} making bitswap request for {:?}", ctx, cid);
        self.sender.send(msg).await?;
        let block = r
            .await
            .map_err(|_| Error::BitswapReqShutdown)?
            .map_err(Error::Str)?;

        if !(cid == block.cid) {
            return Err(Error::UnexpBitswapResponse {
                expected: cid,
                got: block.cid,
            });
        }

        trace!("context:{} got bitswap response for {:?}", ctx, cid);

        Ok(BitswapResponse {
            data: block.data,
            ctx,
        })
    }

    #[tracing::instrument(skip(self, req))]
    async fn stop_session_bitswap(
        &self,
        req: StopSessionBitswapRequest,
    ) -> Result<(), Self::Error> {
        let ctx = req.ctx;
        debug!("stop session bitswap {}", ctx);

        let (s, r) = oneshot::channel();
        let msg = RpcMessage::BitswapStopSession {
            ctx,
            response_channel: s,
        };

        self.sender.send(msg).await?;
        r.await??;
        debug!("stop session bitwap {} done", ctx);

        Ok(())
    }

    #[tracing::instrument(skip(self, req))]
    async fn notify_new_blocks_bitswap(
        &self,
        req: NotifyNewBlocksBitswapRequest,
    ) -> Result<(), Self::Error> {
        let blocks = req
            .blocks
            .into_iter()
            .map(|block| {
                let cid = Cid::read_bytes(io::Cursor::new(block.cid))?;
                Ok(Block::new(block.data, cid))
            })
            .collect::<Result<Vec<Block>, Self::Error>>()?;

        let (s, r) = oneshot::channel();
        let msg = RpcMessage::BitswapNotifyNewBlocks {
            blocks,
            response_channel: s,
        };

        self.sender.send(msg).await?;
        r.await??;

        Ok(())
    }

    #[tracing::instrument(skip(self, req))]
    async fn fetch_provider_dht(
        &self,
        req: ProviderKey,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<Providers, iroh_rpc_types::error::Error>> + Send>>,
        Self::Error,
    > {
        let cid: Cid = req.key.clone().try_into()?;
        trace!("received fetch_provider_dht: {}", cid);
        let (s, r) = channel(64);

        let msg = RpcMessage::ProviderRequest {
            key: ProviderRequestKey::Dht(req.key.into()),
            response_channel: s,
            limit: DEFAULT_PROVIDER_LIMIT,
        };

        self.sender.send(msg).await?;
        let r = tokio_stream::wrappers::ReceiverStream::new(r);

        Ok(Box::pin(r.map(|providers| {
            let providers = providers
                .map_err(iroh_rpc_types::error::Error::Str)?
                .into_iter()
                .map(|p| p.to_bytes())
                .collect();

            Ok(Providers { providers })
        })))
    }

    #[tracing::instrument(skip(self, req))]
    async fn start_providing(&self, req: ProviderKey) -> Result<(), Self::Error> {
        trace!("received StartProviding request: {:?}", req.key);
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::StartProviding(s, req.key.clone().into());

        self.sender.send(msg).await?;

        let query_id = r.await??;

        tracing::debug!("StartProviding query_id: {:?}", query_id);
        Ok(())
    }

    #[tracing::instrument(skip(self, req))]
    async fn stop_providing(&self, req: ProviderKey) -> Result<(), Self::Error> {
        trace!("received StopProviding request: {:?}", req.key);
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::StopProviding(s, req.key.clone().into());

        self.sender.send(msg).await?;

        r.await??;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn get_listening_addrs(&self, _: ()) -> Result<GetListeningAddrsResponse, Self::Error> {
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::NetListeningAddrs(s);
        self.sender.send(msg).await?;

        let (peer_id, addrs) = r.await?;

        Ok(GetListeningAddrsResponse {
            peer_id: peer_id.to_bytes(),
            addrs: addrs.into_iter().map(|addr| addr.to_vec()).collect(),
        })
    }

    #[tracing::instrument(skip(self))]
    async fn get_peers(&self, _: ()) -> Result<GetPeersResponse, Self::Error> {
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::NetPeers(s);
        self.sender.send(msg).await?;

        let peers = r.await?;
        let mut p: HashMap<String, Multiaddrs> = Default::default();
        for (id, addrs) in peers.into_iter() {
            p.insert(
                id.to_string(),
                Multiaddrs {
                    addrs: addrs.into_iter().map(|addr| addr.to_vec()).collect(),
                },
            );
        }
        Ok(GetPeersResponse { peers: p })
    }

    #[tracing::instrument(skip(self, req))]
    /// First attempts to find the peer on the DHT, if found, it will then ensure we have
    /// a connection to the peer.
    async fn peer_connect_by_peer_id(
        &self,
        req: ConnectByPeerIdRequest,
    ) -> Result<(), Self::Error> {
        let peer_id = peer_id_from_bytes(req.peer_id)?;
        let (s, r) = oneshot::channel();
        // ask the swarm if we already have address for this peer
        let msg = RpcMessage::AddressesOfPeer(s, peer_id);
        self.sender.send(msg).await?;
        let res = r.await?;
        if res.is_empty() {
            // if we don't have the addr info for this peer, we need to try to
            // find it on the dht
            let (s, r) = oneshot::channel();
            let msg = RpcMessage::FindPeerOnDHT(s, peer_id);
            self.sender.send(msg).await?;
            r.await??;
        }
        // now we know we have found the peer on the dht,
        // we can attempt to dial it
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::NetConnectByPeerId(s, peer_id);
        self.sender.send(msg).await?;
        r.await?
    }

    #[tracing::instrument(skip(self, req))]
    /// Dial the peer directly using the PeerId and Multiaddr
    async fn peer_connect(&self, req: ConnectRequest) -> Result<(), Self::Error> {
        let peer_id = peer_id_from_bytes(req.peer_id)?;
        let addrs = addrs_from_bytes(req.addrs)?;
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::NetConnect(s, peer_id, addrs);
        self.sender.send(msg).await?;
        r.await?
    }

    #[tracing::instrument(skip(self, req))]
    async fn peer_disconnect(&self, req: DisconnectRequest) -> Result<(), Self::Error> {
        let peer_id = peer_id_from_bytes(req.peer_id)?;
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::NetDisconnect(s, peer_id);
        self.sender.send(msg).await?;
        let ack = r.await?;

        Ok(ack)
    }

    #[tracing::instrument(skip(self, req))]
    async fn lookup(&self, req: LookupRequest) -> Result<PeerInfo, Self::Error> {
        let (s, r) = oneshot::channel();
        let peer_id = peer_id_from_bytes(req.peer_id.clone())?;

        // check if we have already encountered this peer, and already
        // that the peer info
        let msg = RpcMessage::LookupPeerInfo(s, peer_id);
        self.sender.send(msg).await?;
        if let Some(info) = r.await? {
            return Ok(peer_info_from_identify_info(info));
        }

        // listen for if any peer info for this peer gets sent to us
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::ListenForIdentify(s, peer_id);
        self.sender.send(msg).await?;

        // once we connect to the peer, the idenitfy protocol
        // will attempt to exchange peer info
        let res = match req.addr {
            Some(addr) => {
                self.peer_connect(ConnectRequest {
                    peer_id: req.peer_id,
                    addrs: vec![addr],
                })
                .await
            }
            None => {
                self.peer_connect_by_peer_id(ConnectByPeerIdRequest {
                    peer_id: req.peer_id,
                })
                .await
            }
        };

        if let Err(e) = res {
            let (s, r) = oneshot::channel();
            self.sender
                .send(RpcMessage::CancelListenForIdentify(s, peer_id))
                .await?;
            r.await?;
            return Err(Error::from(e));
        }

        let info = r.await??;

        Ok(peer_info_from_identify_info(info))
    }

    #[tracing::instrument(skip(self, req))]
    async fn gossipsub_add_explicit_peer(
        &self,
        req: GossipsubPeerIdMsg,
    ) -> Result<(), Self::Error> {
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::Gossipsub(GossipsubMessage::AddExplicitPeer(
            s,
            peer_id_from_bytes(req.peer_id)?,
        ));
        self.sender.send(msg).await?;
        r.await?;

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn gossipsub_all_mesh_peers(&self, _: ()) -> Result<GossipsubPeersResponse, Self::Error> {
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::Gossipsub(GossipsubMessage::AllMeshPeers(s));
        self.sender.send(msg).await?;
        let peers = r.await?;

        let peers = peers.into_iter().map(|p| p.to_bytes()).collect();
        Ok(GossipsubPeersResponse { peers })
    }

    #[tracing::instrument(skip(self))]
    async fn gossipsub_all_peers(&self, _: ()) -> Result<GossipsubAllPeersResponse, Self::Error> {
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::Gossipsub(GossipsubMessage::AllPeers(s));
        self.sender.send(msg).await?;

        let all_peers = r.await?;
        let all = all_peers
            .into_iter()
            .map(|(p, t)| GossipsubPeerAndTopics {
                peer_id: p.to_bytes(),
                topics: t.into_iter().map(|t| t.into_string()).collect(),
            })
            .collect();

        Ok(GossipsubAllPeersResponse { all })
    }

    #[tracing::instrument(skip(self, req))]
    async fn gossipsub_mesh_peers(
        &self,
        req: GossipsubTopicHashMsg,
    ) -> Result<GossipsubPeersResponse, Self::Error> {
        let topic = TopicHash::from_raw(req.topic_hash);
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::Gossipsub(GossipsubMessage::MeshPeers(s, topic));
        self.sender.send(msg).await?;

        let res = r.await?;
        let peers = res.into_iter().map(|p| p.to_bytes()).collect();

        Ok(GossipsubPeersResponse { peers })
    }

    #[tracing::instrument(skip(self, req))]
    async fn gossipsub_publish(
        &self,
        req: GossipsubPublishRequest,
    ) -> Result<GossipsubPublishResponse, Self::Error> {
        let data = req.data;
        let topic_hash = TopicHash::from_raw(req.topic_hash);
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::Gossipsub(GossipsubMessage::Publish(s, topic_hash, data));
        self.sender.send(msg).await?;

        let message_id = r.await??;

        Ok(GossipsubPublishResponse {
            message_id: message_id.0,
        })
    }

    #[tracing::instrument(skip(self, req))]
    async fn gossipsub_remove_explicit_peer(
        &self,
        req: GossipsubPeerIdMsg,
    ) -> Result<(), Self::Error> {
        let peer_id = peer_id_from_bytes(req.peer_id)?;
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::Gossipsub(GossipsubMessage::RemoveExplicitPeer(s, peer_id));
        self.sender.send(msg).await?;

        r.await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, req))]
    async fn gossipsub_subscribe(
        &self,
        req: GossipsubTopicHashMsg,
    ) -> Result<GossipsubSubscribeResponse, Self::Error> {
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::Gossipsub(GossipsubMessage::Subscribe(
            s,
            TopicHash::from_raw(req.topic_hash),
        ));

        self.sender.send(msg).await?;

        let was_subscribed = r.await??;

        Ok(GossipsubSubscribeResponse { was_subscribed })
    }

    #[tracing::instrument(skip(self))]
    async fn gossipsub_topics(&self, _: ()) -> Result<GossipsubTopicsResponse, Self::Error> {
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::Gossipsub(GossipsubMessage::Topics(s));

        self.sender.send(msg).await.map_err(Error::from)?;

        let topics: Vec<String> = r
            .await
            .map_err(Error::from)?
            .into_iter()
            .map(|t| t.into_string())
            .collect();

        Ok(GossipsubTopicsResponse { topics })
    }

    #[tracing::instrument(skip(self, req))]
    async fn gossipsub_unsubscribe(
        &self,
        req: GossipsubTopicHashMsg,
    ) -> Result<GossipsubSubscribeResponse, Self::Error> {
        let (s, r) = oneshot::channel();
        let msg = RpcMessage::Gossipsub(GossipsubMessage::Unsubscribe(
            s,
            TopicHash::from_raw(req.topic_hash),
        ));

        self.sender.send(msg).await.map_err(Error::from)?;
        let was_subscribed = r.await??;

        Ok(GossipsubSubscribeResponse { was_subscribed })
    }
}

pub async fn new(addr: P2pServerAddr, sender: Sender<RpcMessage>) -> Result<(), Error> {
    let p2p = P2p { sender };

    iroh_rpc_types::p2p::serve(addr, p2p)
        .await
        .map_err(Error::from)
}

fn peer_info_from_identify_info(i: IdentifyInfo) -> PeerInfo {
    let peer_id = i.public_key.to_peer_id();
    PeerInfo {
        peer_id: peer_id.to_bytes(),
        protocol_version: i.protocol_version,
        agent_version: i.agent_version,
        listen_addrs: i
            .listen_addrs
            .into_iter()
            .map(|addr| addr.to_vec())
            .collect(),
        protocols: i.protocols,
        observed_addr: i.observed_addr.to_vec(),
    }
}

fn peer_id_from_bytes(p: Vec<u8>) -> Result<PeerId, Error> {
    PeerId::from_bytes(&p[..]).map_err(Error::from)
}

fn addr_from_bytes(m: Vec<u8>) -> Result<Multiaddr, Error> {
    Multiaddr::try_from(m).map_err(Error::from)
}

fn addrs_from_bytes(a: Vec<Vec<u8>>) -> Result<Vec<Multiaddr>, Error> {
    a.into_iter().map(addr_from_bytes).collect()
}

#[derive(Debug)]
pub enum ProviderRequestKey {
    // TODO: potentially change this to Cid, as that is the only key we use for providers
    Dht(Key),
    Bitswap(u64, Cid),
}

/// Rpc specific messages handled by the p2p node
#[derive(Debug)]
pub enum RpcMessage {
    ExternalAddrs(oneshot::Sender<Vec<Multiaddr>>),
    Listeners(oneshot::Sender<Vec<Multiaddr>>),
    LocalPeerId(oneshot::Sender<PeerId>),
    BitswapRequest {
        ctx: u64,
        cids: Vec<Cid>,
        response_channels: Vec<oneshot::Sender<Result<Block, String>>>,
        providers: HashSet<PeerId>,
    },
    BitswapNotifyNewBlocks {
        blocks: Vec<Block>,
        response_channel: oneshot::Sender<Result<(), Error>>,
    },
    BitswapStopSession {
        ctx: u64,
        response_channel: oneshot::Sender<Result<(), Error>>,
    },
    ProviderRequest {
        key: ProviderRequestKey,
        response_channel: Sender<Result<HashSet<PeerId>, String>>,
        limit: usize,
    },
    StartProviding(oneshot::Sender<Result<libp2p::kad::QueryId, Error>>, Key),
    StopProviding(oneshot::Sender<Result<(), Error>>, Key),
    NetListeningAddrs(oneshot::Sender<(PeerId, Vec<Multiaddr>)>),
    NetPeers(oneshot::Sender<HashMap<PeerId, Vec<Multiaddr>>>),
    NetConnectByPeerId(oneshot::Sender<Result<(), Error>>, PeerId),
    NetConnect(oneshot::Sender<Result<(), Error>>, PeerId, Vec<Multiaddr>),
    NetDisconnect(oneshot::Sender<()>, PeerId),
    Gossipsub(GossipsubMessage),
    FindPeerOnDHT(oneshot::Sender<Result<(), Error>>, PeerId),
    LookupPeerInfo(oneshot::Sender<Option<IdentifyInfo>>, PeerId),
    ListenForIdentify(oneshot::Sender<Result<IdentifyInfo, Error>>, PeerId),
    CancelListenForIdentify(oneshot::Sender<()>, PeerId),
    AddressesOfPeer(oneshot::Sender<Vec<Multiaddr>>, PeerId),
    Shutdown,
}

#[derive(Debug)]
pub enum GossipsubMessage {
    AddExplicitPeer(oneshot::Sender<()>, PeerId),
    AllMeshPeers(oneshot::Sender<Vec<PeerId>>),
    AllPeers(oneshot::Sender<Vec<(PeerId, Vec<TopicHash>)>>),
    MeshPeers(oneshot::Sender<Vec<PeerId>>, TopicHash),
    Publish(
        oneshot::Sender<Result<MessageId, PublishError>>,
        TopicHash,
        Bytes,
    ),
    RemoveExplicitPeer(oneshot::Sender<()>, PeerId),
    Subscribe(oneshot::Sender<Result<bool, SubscriptionError>>, TopicHash),
    Topics(oneshot::Sender<Vec<TopicHash>>),
    Unsubscribe(oneshot::Sender<Result<bool, PublishError>>, TopicHash),
}
