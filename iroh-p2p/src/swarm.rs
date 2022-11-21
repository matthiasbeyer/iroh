use std::time::Duration;

use iroh_rpc_client::Client;
use libp2p::{
    core::{
        self,
        muxing::StreamMuxerBox,
        transport::{timeout::TransportTimeout, Boxed, OrTransport},
    },
    dns,
    identity::Keypair,
    mplex, noise,
    swarm::{ConnectionLimits, Executor, SwarmBuilder},
    yamux::{self, WindowUpdateMode},
    PeerId, Swarm, Transport,
};

use crate::error::Error;
use crate::{behaviour::NodeBehaviour, Libp2pConfig};

/// Builds the transport stack that LibP2P will communicate over.
async fn build_transport(
    keypair: &Keypair,
    config: &Libp2pConfig,
) -> (
    Boxed<(PeerId, StreamMuxerBox)>,
    Option<libp2p::relay::v2::client::Client>,
) {
    // TODO: make transports configurable

    let tcp_config = libp2p::tcp::Config::default().port_reuse(true);
    let transport = libp2p::tcp::tokio::Transport::new(tcp_config.clone());
    let transport =
        libp2p::websocket::WsConfig::new(libp2p::tcp::tokio::Transport::new(tcp_config))
            .or_transport(transport);

    // TODO: configurable
    let transport = TransportTimeout::new(transport, Duration::from_secs(10));
    let dns_cfg = dns::ResolverConfig::cloudflare();
    let dns_opts = dns::ResolverOpts::default();
    let transport = dns::TokioDnsConfig::custom(transport, dns_cfg, dns_opts).unwrap();

    let auth_config = {
        let dh_keys = noise::Keypair::<noise::X25519Spec>::new()
            .into_authentic(keypair)
            .expect("Noise key generation failed");

        noise::NoiseConfig::xx(dh_keys).into_authenticated()
    };

    let muxer_config = {
        let mut mplex_config = mplex::MplexConfig::new();
        mplex_config.set_max_buffer_size(usize::MAX);

        let mut yamux_config = yamux::YamuxConfig::default();
        yamux_config.set_max_buffer_size(16 * 1024 * 1024); // TODO: configurable
        yamux_config.set_receive_window_size(16 * 1024 * 1024); // TODO: configurable
        yamux_config.set_window_update_mode(WindowUpdateMode::on_receive());
        core::upgrade::SelectUpgrade::new(yamux_config, mplex_config)
    };

    // TODO: configurable
    let connection_timeout = Duration::from_secs(30);
    if config.relay_client {
        let (relay_transport, relay_client) =
            libp2p::relay::v2::client::Client::new_transport_and_behaviour(
                keypair.public().to_peer_id(),
            );

        let transport = OrTransport::new(relay_transport, transport);
        let transport = transport
            .upgrade(core::upgrade::Version::V1Lazy)
            .authenticate(auth_config)
            .multiplex(muxer_config)
            .timeout(connection_timeout)
            .boxed();

        (transport, Some(relay_client))
    } else {
        let transport = transport
            .upgrade(core::upgrade::Version::V1Lazy)
            .authenticate(auth_config)
            .multiplex(muxer_config)
            .timeout(connection_timeout)
            .boxed();

        (transport, None)
    }
}

pub(crate) async fn build_swarm(
    config: &Libp2pConfig,
    keypair: &Keypair,
    rpc_client: Client,
) -> Result<Swarm<NodeBehaviour>, Error> {
    let peer_id = keypair.public().to_peer_id();

    let (transport, relay_client) = build_transport(keypair, config).await;
    let behaviour = NodeBehaviour::new(keypair, config, relay_client, rpc_client).await?;

    let limits = ConnectionLimits::default()
        .with_max_pending_incoming(Some(config.max_conns_pending_in))
        .with_max_pending_outgoing(Some(config.max_conns_pending_out))
        .with_max_established_incoming(Some(config.max_conns_in))
        .with_max_established_outgoing(Some(config.max_conns_out))
        .with_max_established_per_peer(Some(config.max_conns_per_peer));
    let swarm = SwarmBuilder::with_executor(transport, behaviour, peer_id, Tokio)
        .connection_limits(limits)
        .notify_handler_buffer_size(config.notify_handler_buffer_size.try_into()?)
        .connection_event_buffer_size(config.connection_event_buffer_size)
        .dial_concurrency_factor(config.dial_concurrency_factor.try_into().unwrap())
        .build();

    Ok(swarm)
}

struct Tokio;
impl Executor for Tokio {
    fn exec(&self, fut: std::pin::Pin<Box<dyn futures::Future<Output = ()> + Send>>) {
        tokio::task::spawn(fut);
    }
}
