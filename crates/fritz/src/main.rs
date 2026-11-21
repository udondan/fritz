//! `fritz`: CLI and MCP server for FRITZ!Box routers.
//!
//! Unofficial, not affiliated with FRITZ! GmbH.
//!
//! Released as binaries, a Homebrew formula and a Docker image, see the
//! installation section of the README.

mod auth;
#[cfg(feature = "write")]
mod change;
mod context;
mod device;
mod mcp;
mod personal;
mod settings;
mod status;
mod system;
mod tls;
mod tr64;

use std::io::{self, IsTerminal as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use clap::{Args, Parser, Subcommand};
use fritz_core::anonymize;
use fritz_core::capture::{self, Options, Plan};
use fritz_core::connection::Overrides;
use fritz_core::secrets::ENV_PASSWORD;
use tr064::{Client, SecretString, Url};

use crate::context::Context;
use crate::status::Format;

/// CLI and MCP server for FRITZ!Box routers (unofficial).
///
/// The password comes from the environment variable `FRITZ_PASSWORD`, else
/// from the keychain (see `fritz auth login`). `FRITZ_NO_KEYCHAIN=1` turns
/// the keychain off.
#[derive(Debug, Parser)]
#[command(name = "fritz", version, about)]
struct Cli {
    #[command(flatten)]
    global: Global,
    #[command(subcommand)]
    command: Command,
}

/// Where and how to connect, ahead of the profile.
#[derive(Debug, Args)]
struct Global {
    /// The configuration file.
    #[arg(long, global = true, env = "FRITZ_CONFIG")]
    config: Option<PathBuf>,
    /// The profile in the configuration file, else the default one.
    #[arg(long, global = true, env = "FRITZ_PROFILE")]
    profile: Option<String>,
    /// The device: a host name or address, optionally with a port, for HTTPS
    /// on port 49443, or a full address like `http://192.168.178.1:49000`.
    /// Defaults to fritz.box.
    #[arg(long, global = true, env = "FRITZ_HOST")]
    host: Option<String>,
    /// The user on the device.
    #[arg(long, global = true, env = "FRITZ_USERNAME")]
    user: Option<String>,
    /// The SHA-256 fingerprint of the certificate to trust, instead of the
    /// one of the profile.
    #[arg(long, global = true, env = "FRITZ_TLS_FINGERPRINT")]
    fingerprint: Option<String>,
    /// How to print results: text, or JSON, which is the default when the
    /// output is not a terminal.
    #[arg(long, global = true, value_enum)]
    output: Option<Format>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// The model, firmware, uptime, internet connection and the rights of
    /// the user.
    Overview,
    /// The hosts the device knows.
    Hosts {
        /// Only the hosts that are connected now.
        #[arg(long)]
        active: bool,
    },
    /// The mesh: its nodes and the links between all devices.
    Mesh,
    /// The wireless networks.
    Wlan,
    /// The internet connection and, for DSL, the line.
    Internet,
    /// The Smart Home devices and their values, read through the web
    /// interface (FRITZ!OS 8.20 and later).
    Smarthome,
    /// The port forwardings, and whether hosts may open ports through UPnP.
    Ports,
    /// The numbers, accounts, call deflections, answering machines and DECT
    /// handsets.
    Telephony,
    /// The features the device offers and the rights of the user.
    Capabilities,
    /// The settings of the device, like DNS and IPv6, read through the web
    /// interface. Secrets are hidden.
    Settings {
        /// The modules to read, like ipv6 or dnscfg. Without, the common
        /// ones.
        modules: Vec<String>,
    },
    /// The latest calls, the latest first. Personal data.
    Calls {
        /// The most calls to show.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// The contacts of the phonebooks. Personal data.
    Phonebook {
        /// Only the contacts whose name contains this, ignoring case, or
        /// whose number contains its digits.
        search: Option<String>,
    },
    /// The latest entries of the event log, the latest first. Personal
    /// data.
    Log {
        /// The most entries to show.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Manage the profiles in the configuration file.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Trust the certificate of the device.
    #[command(subcommand)]
    Tls(TlsCommand),
    /// Manage the password in the keychain.
    #[command(subcommand)]
    Auth(AuthCommand),
    /// The services and actions of the device. Calls only actions that read.
    #[command(subcommand)]
    Tr64(Tr64Command),
    /// Plan and apply changes. Needs `FRITZ_ALLOW_WRITE=1`, and the identity
    /// of the device on the allow list in the configuration.
    #[cfg(feature = "write")]
    Change(change::ChangeArgs),
    /// Serve the state of the device over MCP on stdin and stdout. Only
    /// reads.
    Mcp {
        /// Also offer the calls, the phonebooks and the event log, which
        /// name people and numbers.
        #[arg(long, env = "FRITZ_MCP_PERSONAL")]
        personal: bool,
    },
    /// Record the answers of read-only actions as test fixtures.
    Capture(CaptureArgs),
    /// Turn raw captures into fixtures, and check fixtures for personal data.
    #[command(subcommand)]
    Fixture(FixtureCommand),
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Save --host, --user and --fingerprint to the profile, which becomes
    /// the default if there is none.
    Init,
    /// Show the settings in effect, without the password.
    Show,
}

#[derive(Debug, Subcommand)]
enum TlsCommand {
    /// Print the fingerprint of the certificate the device presents.
    Fingerprint,
    /// Trust the certificate the device presents, after confirmation, and
    /// save it to the profile.
    Trust,
    /// Remove the trusted certificate from the profile.
    Forget,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    /// Store the password in the keychain. Reads it from the terminal, or a
    /// line from the standard input.
    Login,
    /// Remove the password from the keychain.
    Logout,
    /// Tell where the password comes from.
    Status,
}

#[derive(Debug, Subcommand)]
enum Tr64Command {
    /// List the services. Needs no login.
    Services,
    /// List the actions of a service with their classes: read, personal,
    /// secret, write or critical. Needs no login.
    Actions {
        /// The service, like `DeviceInfo1` or `DeviceInfo:1`.
        service: String,
    },
    /// Show the class and the arguments of an action. Needs no login.
    Describe {
        /// The service, like `DeviceInfo1` or `DeviceInfo:1`.
        service: String,
        /// The action, like `GetInfo`.
        action: String,
    },
    /// Call an action that reads, and print its output with secrets hidden.
    /// Other actions are refused before anything is sent.
    Call {
        /// The service, like `DeviceInfo1` or `DeviceInfo:1`.
        service: String,
        /// The action, like `GetInfo`.
        action: String,
        /// The inputs, like `NewIndex=0`.
        #[arg(value_name = "NAME=VALUE")]
        arguments: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum FixtureCommand {
    /// Replace the personal values of a raw capture with placeholders.
    Anonymize {
        /// The directory of the raw capture.
        raw: PathBuf,
        /// A new directory for the fixture, like `fixtures/<model>/<version>`.
        #[arg(long)]
        out: PathBuf,
        #[command(flatten)]
        private: PrivateArgs,
    },
    /// Look for personal values in fixtures. Reports the files and the kinds
    /// of values, never the values.
    Check {
        /// The directory to check.
        #[arg(default_value = "fixtures")]
        dir: PathBuf,
        #[command(flatten)]
        private: PrivateArgs,
    },
}

#[derive(Debug, Args)]
struct PrivateArgs {
    /// A file with texts that must not be in a fixture, one per line, like
    /// one's own name or street. Defaults to `.private-patterns` if it exists.
    #[arg(long)]
    private_patterns: Option<PathBuf>,
}

impl PrivateArgs {
    fn load(&self) -> anyhow::Result<Vec<String>> {
        let default = Path::new(".private-patterns");
        let path = match &self.private_patterns {
            Some(path) => path,
            None if default.exists() => default,
            None => return Ok(Vec::new()),
        };
        anonymize::read_private_patterns(path)
            .with_context(|| format!("cannot read {}", path.display()))
    }
}

#[derive(Debug, Args)]
struct CaptureArgs {
    /// Only list what a capture would read. Loads the device and service
    /// descriptions, which need no login, and calls no action.
    #[arg(long)]
    plan: bool,

    /// A new directory for the raw capture. It contains personal data, so it
    /// must not be inside a git repository.
    #[arg(long, required_unless_present = "plan")]
    out: Option<PathBuf>,

    /// Also read call lists, phonebooks and voice messages.
    #[arg(long)]
    include_personal: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let global = cli.global;
    let format = global
        .output
        .unwrap_or_else(|| Format::for_terminal(io::stdout().is_terminal()));
    let load = move || {
        let overrides = Overrides {
            host: global.host,
            username: global.user,
            fingerprint: global.fingerprint,
        };
        let password = std::env::var(ENV_PASSWORD).ok().map(SecretString::from);
        Context::load(
            global.config,
            global.profile,
            overrides,
            password,
            system::keychains(),
        )
    };
    if let Command::Mcp { personal } = cli.command {
        // Before stdout is locked: the transport writes to it.
        return mcp::serve(load()?, personal).await;
    }
    let out = &mut io::stdout().lock();
    match cli.command {
        Command::Mcp { .. } => unreachable!("served above"),
        Command::Overview => status::overview(&load()?, format, out).await,
        Command::Hosts { active } => status::hosts(&load()?, active, format, out).await,
        Command::Mesh => status::mesh(&load()?, format, out).await,
        Command::Wlan => status::wlan(&load()?, format, out).await,
        Command::Internet => status::internet(&load()?, format, out).await,
        Command::Smarthome => status::smarthome(&load()?, format, out).await,
        Command::Ports => device::ports(&load()?, format, out).await,
        Command::Telephony => device::telephony(&load()?, format, out).await,
        Command::Capabilities => device::capabilities(&load()?, format, out).await,
        Command::Settings { modules } => device::settings(&load()?, &modules, format, out).await,
        Command::Calls { limit } => personal::calls(&load()?, limit, format, out).await,
        Command::Phonebook { search } => {
            personal::phonebook(&load()?, search.as_deref(), format, out).await
        }
        Command::Log { limit } => personal::log(&load()?, limit, format, out).await,
        Command::Config(ConfigCommand::Init) => settings::init(&mut load()?, out),
        Command::Config(ConfigCommand::Show) => settings::show(&load()?, out),
        Command::Tls(TlsCommand::Fingerprint) => tls::fingerprint(&load()?, out).await,
        Command::Tls(TlsCommand::Trust) => {
            tls::trust(&mut load()?, &mut io::stdin().lock(), out).await
        }
        Command::Tls(TlsCommand::Forget) => tls::forget(&mut load()?, out),
        Command::Auth(AuthCommand::Login) => auth::login(&load()?, system::read_password, out),
        Command::Auth(AuthCommand::Logout) => auth::logout(&load()?, out),
        Command::Auth(AuthCommand::Status) => auth::status(&load()?, out),
        Command::Tr64(command) => tr64(&load()?, command, out).await,
        #[cfg(feature = "write")]
        Command::Change(args) => change::run(&load()?, args, format, out).await,
        Command::Capture(args) => capture(&load()?, args).await,
        Command::Fixture(command) => fixture(&command),
    }
}

async fn tr64(
    context: &Context,
    command: Tr64Command,
    out: &mut impl io::Write,
) -> anyhow::Result<()> {
    match command {
        Tr64Command::Services => tr64::services(context, out).await,
        Tr64Command::Actions { service } => tr64::actions(context, &service, out).await,
        Tr64Command::Describe { service, action } => {
            tr64::describe(context, &service, &action, out).await
        }
        Tr64Command::Call {
            service,
            action,
            arguments,
        } => tr64::call(context, &service, &action, &arguments, out).await,
    }
}

fn fixture(command: &FixtureCommand) -> anyhow::Result<()> {
    match command {
        FixtureCommand::Anonymize { raw, out, private } => {
            match anonymize::anonymize(raw, out, &private.load()?) {
                Ok(count) => {
                    println!("anonymized {count} files into {}", out.display());
                    Ok(())
                }
                Err(anonymize::Error::Leftovers(findings)) => {
                    report(&findings);
                    bail!("personal values remain, nothing was written")
                }
                Err(error) => Err(error.into()),
            }
        }
        FixtureCommand::Check { dir, private } => {
            let findings = anonymize::check(dir, &private.load()?)?;
            report(&findings);
            if !findings.is_empty() {
                bail!(
                    "{} personal values found in {}",
                    findings.len(),
                    dir.display()
                );
            }
            println!("no personal values found in {}", dir.display());
            Ok(())
        }
    }
}

fn report(findings: &[anonymize::Finding]) {
    for finding in findings {
        eprintln!("{finding}");
    }
}

async fn capture(context: &Context, args: CaptureArgs) -> anyhow::Result<()> {
    let target = context.target()?;
    let url = &target.url;
    let builder = target.client()?;
    if args.plan {
        let plan = read_plan(&builder.build()?, url).await?;
        println!("{plan}");
        return Ok(());
    }
    let out = args.out.expect("clap requires --out without --plan");
    check_outside_git(&out)?;
    let (password, _) = context.password(&target)?;
    if url.scheme() == "http" {
        eprintln!("warning: the answers travel unencrypted, use https and run `fritz tls trust`");
    }
    let client = builder.credentials(target.username()?, password).build()?;
    let plan = read_plan(&client, url).await?;
    std::fs::create_dir(&out).with_context(|| format!("cannot create {}", out.display()))?;
    let options = Options {
        include_personal: args.include_personal,
        ..Options::default()
    };
    let result = capture::run(&client, &plan, &options, |r| eprintln!("{r}")).await;
    capture::write(&plan, &result, &out)
        .with_context(|| format!("cannot write the capture to {}", out.display()))?;
    println!("{result}, saved to {}", out.display());
    match result.stopped {
        Some(error) => Err(anyhow::Error::new(error).context("the capture stopped early")),
        None => Ok(()),
    }
}

async fn read_plan(client: &Client, url: &Url) -> anyhow::Result<Plan> {
    capture::plan(client)
        .await
        .with_context(|| format!("cannot read the descriptions of {url}"))
}

/// Refuses directories inside a git repository, so that raw captures do not
/// end up in a commit.
fn check_outside_git(dir: &Path) -> anyhow::Result<()> {
    let absolute = std::path::absolute(dir)?;
    if let Some(repository) = absolute.ancestors().find(|p| p.join(".git").exists()) {
        bail!(
            "{} is inside the git repository {}, but raw captures contain personal data",
            dir.display(),
            repository.display()
        );
    }
    Ok(())
}
