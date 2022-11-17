#[cfg(any(target_os = "macos", target_os = "linux"))]
use nix::sys::signal::{kill, Signal};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use nix::unistd::Pid;
use std::path::PathBuf;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::{Command, Stdio};

use crate::error::Error;

// TODO(b5): instead of using u32's for Process Identifiers, use a proper Pid type
// something along the lines of:

// #[cfg(unix)]
// #[derive(Debug, Clone, Copy, PartialEq, Eq)]
// pub struct Pid(nix::unistd::Pid);

// #[cfg(not(unix))]
// #[derive(Debug, Clone, Copy, PartialEq, Eq)]
// pub struct Pid; // TODO: fill in for each platform when supported

// // #[cfg(unix)]
// impl From nix::Pid for Pid {
//  // ..
// }

// impl std::fmt::Display for Pid {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         write!(f, "{}", self.to_string())
//     }
// }

pub fn daemonize(bin_path: PathBuf, log_path: PathBuf) -> Result<(), Error> {
    daemonize_process(bin_path, log_path)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn daemonize_process(_bin_path: PathBuf, _log_path: PathBuf) -> Result<(), Error> {
    Err(Error::DaemonizingNotSupported)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn daemonize_process(bin_path: PathBuf, log_path: PathBuf) -> Result<(), Error> {
    std::fs::create_dir_all(log_path.parent().unwrap())?;
    // ¯\_(ツ)_/¯
    let status = Command::new("bash")
        .arg("-c")
        // TODO(b5): might be nice to capture output in a log file at some point?
        .arg(format!(
            "nohup {} > {} 2>&1 &",
            bin_path.to_str().unwrap(),
            log_path.to_str().unwrap(),
        ))
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .status()?;

    if !status.success() {
        return Err(Error::CloudNotDaemonize)
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn daemonize_process(_bin_path: PathBuf, _log_path: PathBuf) -> Result<(), Error> {
    Err(Error::DaemonizingNotSupportedOnWindows)
}

// TODO(b5) - this level of indirection isn't necessary, factor `stop_process`
// directly into `stop`
// https://github.com/n0-computer/iroh/pull/360#discussion_r1002000769
pub fn stop(pid: u32) -> Result<(), Error> {
    stop_process(pid)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn stop_process(pid: u32) -> Result<()> {
    Err(Error::DaemonizingNotSupported)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn stop_process(pid: u32) -> Result<(), Error> {
    let id = Pid::from_raw(pid as i32);
    kill(id, Signal::SIGINT).map_err(Error::from)
}

#[cfg(target_os = "windows")]
fn stop_process(_pid: u32) -> Result<()> {
    Err(Error::DaemonizingNotSupportedOnWindows)
}
