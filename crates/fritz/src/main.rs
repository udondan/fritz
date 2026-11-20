//! `fritz`: CLI and MCP server for FRITZ!Box routers.
//!
//! Unofficial, not affiliated with FRITZ! GmbH.

use clap::Parser;

/// CLI and MCP server for FRITZ!Box routers (unofficial).
#[derive(Debug, Parser)]
#[command(name = "fritz", version, about)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
