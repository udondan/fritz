//! Serves a fixture as a fake TR-064 device, for trying the CLI without a
//! real device.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use clap::Parser;
use fritz_sim::{Fixture, Options, Sim};

/// Serves a recorded fixture as a fake TR-064 device until interrupted.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// A fixture that comes with fritz-sim, like fritzbox-7690/8.25, or the
    /// directory of another one.
    #[arg(long, required_unless_present = "list")]
    fixture: Option<PathBuf>,
    /// Print the names of the fixtures that come with fritz-sim.
    #[arg(long)]
    list: bool,
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
    /// Serve HTTPS with a new self-signed certificate.
    #[arg(long)]
    https: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.list {
        for name in Fixture::bundled_names() {
            println!("{name}");
        }
        return Ok(());
    }
    let fixture = load(&args.fixture.unwrap_or_default())?;
    let options = Options {
        address: SocketAddr::new(args.address, args.port),
        username: args.username,
        password: args.password,
        https: args.https,
    };
    let sim = Sim::start_with(fixture, options).await?;
    println!("listening on {}", sim.url());
    if let Some(fingerprint) = sim.fingerprint() {
        println!("certificate {fingerprint}");
    }
    tokio::signal::ctrl_c().await?;
    Ok(())
}

/// Loads the bundled fixture of the name, unless a directory has that name,
/// or else the fixture in the directory.
fn load(fixture: &Path) -> Result<Fixture, fritz_sim::Error> {
    let name = fixture.to_string_lossy();
    if !fixture.is_dir() && Fixture::bundled_names().iter().any(|n| *n == name) {
        Fixture::bundled(&name)
    } else {
        Fixture::load(fixture)
    }
}
