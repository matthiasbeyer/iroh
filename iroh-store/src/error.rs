use rkyv::ser::serializers::{AllocScratchError, SharedSerializeMapError, CompositeSerializerError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    RocksDb(#[from] rocksdb::Error),

    #[error(transparent)]
    Multihash(#[from] multihash::Error),

    #[error(transparent)]
    TryFromSlice(#[from] std::array::TryFromSliceError),

    #[error(transparent)]
    Composite(#[from] CompositeSerializerError<std::convert::Infallible, AllocScratchError, SharedSerializeMapError>),

    #[error(transparent)]
    Util(#[from] iroh_util::UtilError),

    #[error(transparent)]
    RpcClient(#[from] iroh_rpc_client::Error),

    #[error(transparent)]
    RpcTypes(#[from] iroh_rpc_types::error::Error),

    #[error(transparent)]
    Cid(#[from] cid::Error),

    #[error(transparent)]
    TokioJoin(#[from] tokio::task::JoinError),

    /// TODO: We cannot wrap the appropriate rkyv type here because it is not Send
    #[error("Checking archived root failed {}", .0)]
    RkyvCheckArchivedRoot(String),

    #[error("Invalid link {}", .0)]
    InvalidLink(u64),

    #[error("Missing column family: {}", .0)]
    MissingColFam(&'static str),

    #[error("can not derive rpc_addr for mem addr")]
    CannotDeriveRpcAddrForMem,

    #[error("invalid rpc_addr")]
    InvalidRpcAddr,
}
