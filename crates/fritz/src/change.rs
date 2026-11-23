//! `fritz change`: plans and applies changes, in builds with the feature
//! `write`.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context as _, bail};
use clap::{Args, Subcommand};
use fritz_core::change::{self, Changer, Outcome, Permit, Plan, Settings};

use crate::context::Context;
use crate::output::{Format, show};
use crate::status::pairs;
use crate::tr64::parse_arguments;

#[derive(Debug, Args)]
pub(crate) struct ChangeArgs {
    #[command(flatten)]
    state: StateDir,
    #[command(subcommand)]
    command: ChangeCommand,
}

/// Where the tokens of plans and the audit log are kept.
#[derive(Debug, Args)]
pub(crate) struct StateDir {
    /// The directory of the tokens of plans and the audit log, by default
    /// the state directory of the user.
    #[arg(long, env = "FRITZ_STATE_DIR")]
    state_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum ChangeCommand {
    /// Print the identity of the device, for `allow` and `deny` in the
    /// section `[write]` of the configuration.
    Identity,
    /// Show what an action would change, and issue a token to apply it
    /// with, valid once and for five minutes. Changes nothing.
    Plan(Call),
    /// Apply a planned change, with the same arguments and the token of the
    /// plan, and read the values back.
    Apply {
        #[command(flatten)]
        call: Call,
        /// The token of the plan.
        #[arg(long)]
        token: String,
    },
}

/// An action with its arguments.
#[derive(Debug, Args)]
struct Call {
    /// The service, like `WLANConfiguration1`.
    service: String,
    /// The action, like `SetEnable`.
    action: String,
    /// The inputs, like `NewEnable=0`.
    #[arg(value_name = "NAME=VALUE")]
    arguments: Vec<String>,
}

/// Runs `fritz change`.
pub(crate) async fn run(
    context: &Context,
    args: ChangeArgs,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut changer = changer(context, args.state).await?;
    match args.command {
        ChangeCommand::Identity => {
            let identity = changer.identity().await?;
            writeln!(out, "{identity}")?;
            Ok(())
        }
        ChangeCommand::Plan(call) => plan(&mut changer, &call, format, out).await,
        ChangeCommand::Apply { call, token } => {
            apply(&mut changer, &call, &token, format, out).await
        }
    }
}

/// A changer of the device of the context, with the password, the
/// permit of the environment and the lists of the configuration.
pub(crate) async fn changer(context: &Context, state: StateDir) -> anyhow::Result<Changer> {
    let dir = state
        .state_dir
        .or_else(change::default_dir)
        .context("cannot find the home directory, pass --state-dir")?;
    let settings = Settings {
        permit: Permit::from_env(),
        targets: context.write_targets().clone(),
        dir,
        source: "cli",
    };
    Ok(Changer::new(context.client().await?, settings))
}

/// Plans a change and prints it with its token.
async fn plan(
    changer: &mut Changer,
    call: &Call,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let arguments = parse_arguments(&call.arguments)?;
    let plan = changer
        .plan(&call.service, &call.action, &arguments)
        .await?;
    show(&plan, format, out, |p, out| pairs(&plan_pairs(p), out))
}

fn plan_pairs(plan: &Plan) -> Vec<(&'static str, Option<String>)> {
    let mut pairs = vec![
        ("device", Some(plan.device.to_string())),
        (
            "action",
            Some(format!("{} {} ({})", plan.service, plan.action, plan.class)),
        ),
    ];
    for change in &plan.changes {
        let before = change.before.as_deref().unwrap_or("?");
        let line = format!("{}: {before} -> {}", change.argument, change.after);
        pairs.push(("change", Some(line)));
    }
    pairs.push(("note", Some(plan.note.to_owned())));
    let token = format!("{} (valid for {} s)", plan.token, plan.expires_in);
    pairs.push(("token", Some(token)));
    pairs
}

/// Applies a planned change with its token and prints what the device
/// returns afterwards. Fails if it does not return the values it was given.
async fn apply(
    changer: &mut Changer,
    call: &Call,
    token: &str,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let arguments = parse_arguments(&call.arguments)?;
    let outcome = changer
        .apply(&call.service, &call.action, &arguments, token)
        .await?;
    show(&outcome, format, out, |o, out| {
        pairs(&outcome_pairs(o), out)
    })?;
    if outcome.verified == Some(false) {
        bail!("the device does not return the values it was given");
    }
    Ok(())
}

fn outcome_pairs(outcome: &Outcome) -> Vec<(&'static str, Option<String>)> {
    let mut pairs = vec![
        ("device", Some(outcome.device.to_string())),
        (
            "applied",
            Some(format!("{} {}", outcome.service, outcome.action)),
        ),
    ];
    for check in &outcome.checks {
        let actual = check.actual.as_deref().unwrap_or("?");
        let line = format!("{}: {actual} (sent {})", check.argument, check.expected);
        pairs.push(("now", Some(line)));
    }
    let verified = match outcome.verified {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    };
    pairs.push(("verified", Some(verified.to_owned())));
    if outcome.side_effects.is_empty() {
        let line = format!("nothing else in {}", outcome.service);
        pairs.push(("also changed", Some(line)));
    }
    for effect in &outcome.side_effects {
        let before = effect.before.as_deref().unwrap_or("(none)");
        let after = effect.after.as_deref().unwrap_or("(none)");
        let line = format!("{} {}: {before} -> {after}", effect.getter, effect.argument);
        pairs.push(("also changed", Some(line)));
    }
    pairs
}
