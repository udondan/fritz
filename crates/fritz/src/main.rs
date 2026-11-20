//! `fritz`: CLI and MCP server for FRITZ!Box routers.
//!
//! Unofficial, not affiliated with FRITZ! GmbH.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use clap::{Args, Parser, Subcommand};
use fritz_core::anonymize;
use fritz_core::capture::{self, Options, Plan};
use tr064::{Client, Fingerprint, SecretString, Url};

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
    /// Turn raw captures into fixtures, and check fixtures for personal data.
    #[command(subcommand)]
    Fixture(FixtureCommand),
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

    /// The TR-064 address of the device.
    #[arg(long, default_value = "http://fritz.box:49000")]
    url: Url,

    /// The SHA-256 fingerprint of the device certificate, required for https.
    #[arg(long, env = "FRITZ_TLS_FINGERPRINT")]
    fingerprint: Option<Fingerprint>,

    /// The user on the device. The password is read from `FRITZ_PASSWORD`.
    #[arg(long, env = "FRITZ_USERNAME", required_unless_present = "plan")]
    user: Option<String>,

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
    match Cli::parse().command {
        Command::Capture(args) => capture(args).await,
        Command::Fixture(command) => fixture(&command),
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

async fn capture(args: CaptureArgs) -> anyhow::Result<()> {
    let mut builder = Client::builder(args.url.clone());
    if let Some(fingerprint) = args.fingerprint {
        builder = builder.pin(fingerprint);
    }
    if args.plan {
        let plan = read_plan(&builder.build()?, &args.url).await?;
        println!("{plan}");
        return Ok(());
    }
    let user = args.user.expect("clap requires --user without --plan");
    let out = args.out.expect("clap requires --out without --plan");
    check_outside_git(&out)?;
    let password = std::env::var("FRITZ_PASSWORD")
        .with_context(|| format!("set FRITZ_PASSWORD to the password of {user}"))?;
    if args.url.scheme() == "http" {
        eprintln!(
            "warning: the answers travel unencrypted, use the https address with --fingerprint"
        );
    }
    let client = builder
        .credentials(user, SecretString::from(password))
        .build()?;
    let plan = read_plan(&client, &args.url).await?;
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
