//! Serves a fixture as a fake TR-064 device, for trying the CLI without a
//! real device.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use clap::Parser;
use fritz_sim::{Fixture, Options, Sim};

/// Serves a recorded fixture as a fake TR-064 device until interrupted.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// The fixture directory, like fixtures/fritzbox-7690/8.25.
    #[arg(long)]
    fixture: PathBuf,
    /// The port to listen on. 0 picks a free one.
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// The address to listen on.
    #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    address: IpAddr,
    /// The user name to accept.
    #[arg(long, default_value = fritz_sim::USERNAME)]
    username: String,
    /// The password to accept.
    #[arg(long, default_value = fritz_sim::PASSWORD)]
    password: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let fixture = Fixture::load(&args.fixture)?;
    let options = Options {
        address: SocketAddr::new(args.address, args.port),
        username: args.username,
        password: args.password,
    };
    let sim = Sim::start_with(fixture, options).await?;
    println!("listening on {}", sim.url());
    tokio::signal::ctrl_c().await?;
    Ok(())
}
