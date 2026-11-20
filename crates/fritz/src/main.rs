//! `fritz`: CLI and MCP server for FRITZ!Box routers.
//!
//! Unofficial, not affiliated with FRITZ! GmbH.

use anyhow::{Context as _, bail};
use clap::{Args, Parser, Subcommand};
use fritz_core::capture;
use tr064::{Client, Url};

/// CLI and MCP server for FRITZ!Box routers (unofficial).
#[derive(Debug, Parser)]
#[command(name = "fritz", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Record the answers of read-only actions as test fixtures.
    Capture(CaptureArgs),
}

#[derive(Debug, Args)]
struct CaptureArgs {
    /// Only list what a capture would read. Loads the device and service
    /// descriptions, which need no login, and calls no action.
    #[arg(long)]
    plan: bool,

    /// The TR-064 address of the device.
    #[arg(long, default_value = "http://fritz.box:49000")]
    url: Url,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Capture(args) => capture(args).await,
    }
}

async fn capture(args: CaptureArgs) -> anyhow::Result<()> {
    if !args.plan {
        bail!("only `fritz capture --plan` is available so far");
    }
    let client = Client::builder(args.url.clone()).build()?;
    let plan = capture::plan(&client)
        .await
        .with_context(|| format!("cannot read the descriptions of {}", args.url))?;
    println!("{plan}");
    Ok(())
}
