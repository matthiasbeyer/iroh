use std::path::PathBuf;

use crossterm::style::Stylize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Api(#[from] iroh_api::Error),

    #[error(transparent)]
    Util(#[from] iroh_util::UtilError),

    #[error(transparent)]
    Lock(#[from] iroh_util::lock::LockError),

    #[error(transparent)]
    LocalOps(#[from] iroh_localops::error::Error),

    #[error(transparent)]
    Config(#[from] config::ConfigError),

    #[error(transparent)]
    IndicatifTemplate(#[from] indicatif::style::TemplateError),

    #[error("invalid peer id or multiaddress")]
    InvalidPeerIdIOrMultiAddr,

    #[error("can't find {} daemon binary on your $PATH. please install {}.\n visit https://iroh.computer/docs/install for more info", .0, .0)]
    CannotFindDaemonBinary(String),

    #[error("File processing failed")]
    FileProcessingFailed,

    #[error("Path does not exist: {}", .0.display())]
    PathNotExist(PathBuf),

    #[error("Path is not a file or directory: {}", .0.display())]
    PathNotFileOrDir(PathBuf),

    #[error("{} is a directory, use --recursive to add it", .0.display())]
    PathIsDirNoRecursive(PathBuf),

    #[error("Add provides content to the IPFS network by default, but the p2p service is not running.\n{}",
            "hint: try using the --offline flag, or run 'iroh start p2p'".yellow()
            )]
    AddButNoP2pService,

    #[error("Unknown service(s): {}", .0.join(", "))]
    UnknownServices(Vec<String>),

    #[error("")]
    Empty, // TODO
}
