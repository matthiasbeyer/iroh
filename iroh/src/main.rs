use anyhow::{anyhow, Result};
use clap::Parser;
use crossterm::style::Stylize;
use iroh_api::Error as ApiError;
use std::io;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cli = iroh::run::Cli::parse();
    // the `run` method exists in two versions:
    // When using the `testing` feature, the
    // version of `run` designed for testing purposes using mocked test
    // fixtures is invoked.
    // Without the `testing` feature, the version of
    // `run` that interacts with the real Iroh API is used.
    let r = cli.run().await;
    let r = transform_error(r);
    match r {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("Error: {:?}", e);
            std::process::exit(1);
        }
    }
}

fn transform_error(r: std::result::Result<(), iroh::error::Error>) -> Result<()> {
    match r {
        Ok(_) => Ok(()),
        Err(e) => match e {
            iroh::error::Error::Io(io_error) => {
                if io_error.kind() == io::ErrorKind::ConnectionRefused {
                    Err(anyhow!(
                        "Connection refused. Are services running?\n{}",
                        "hint: see 'iroh start' for more on starting services".yellow(),
                    ))
                } else {
                    Err(anyhow::Error::from(io_error))
                }
            }

            iroh::error::Error::Api(api_error) => match api_error {
                ApiError::ConnectionRefused { service } => Err(anyhow!(
                    "Connection refused. This command requires a running {} service.\n{}",
                    service,
                    format!("hint: try 'iroh start {}'", service).yellow(),
                )),
                _ => Err(anyhow::Error::from(api_error)),
            },

            other => Err(anyhow::Error::from(other)),
        },
    }
}
