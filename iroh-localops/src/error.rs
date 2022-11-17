#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("couldn't daemonize binary")]
    CloudNotDaemonize,

    #[error(transparent)]
    Nix(#[from] nix::Error),

    #[error("daemonizing processes is not supported on your operating system")]
    DaemonizingNotSupported,

    #[error("daemonizing processes on windows is not supported")]
    DaemonizingNotSupportedOnWindows,
}
