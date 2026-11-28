//! `fritz reboot`: restarts the device and waits until it is back, in
//! builds with the feature `write`.

use std::io::Write;
use std::time::Duration;

use anyhow::bail;
use clap::Args;
use fritz_core::change::reboot::{RebootPlan, Report, State, Wait};

use crate::change::{StateDir, changer};
use crate::context::Context;
use crate::output::{Format, show};
use crate::status::{duration, pairs};

/// The longest time to wait for a device.
const LONGEST: Duration = Duration::from_hours(1);

#[derive(Debug, Args)]
pub(crate) struct RebootArgs {
    #[command(flatten)]
    state: StateDir,
    /// Restart with the token of the plan. Without it, only shows the plan
    /// and issues the token.
    #[arg(long)]
    token: Option<String>,
    /// Wait until the device is back. Reads only `DeviceInfo:GetInfo`.
    #[arg(long, requires = "token")]
    wait: bool,
    /// The time between two reads while waiting, like `5s`.
    #[arg(long, requires = "wait", default_value = "5s", value_parser = parse_duration)]
    interval: Duration,
    /// The longest time to wait, like `5m`, at most `1h`.
    #[arg(long, requires = "wait", default_value = "5m", value_parser = parse_duration)]
    budget: Duration,
}

/// Parses a duration like `500ms`, `5s` or `2m`; a bare number is seconds.
fn parse_duration(value: &str) -> Result<Duration, String> {
    let (number, unit) = value
        .find(|c: char| !c.is_ascii_digit())
        .map_or((value, "s"), |i| value.split_at(i));
    let number: u64 = number
        .parse()
        .map_err(|_| format!("{value:?} is not a duration like 5s"))?;
    let duration = match unit {
        "ms" => Duration::from_millis(number),
        "s" => Duration::from_secs(number),
        "m" => Duration::from_mins(number),
        _ => return Err(format!("{value:?} has an unknown unit, use ms, s or m")),
    };
    if duration.is_zero() || duration > LONGEST {
        return Err(format!("{value:?} is not between 1ms and 1h"));
    }
    Ok(duration)
}

/// Runs `fritz reboot`. Prints progress to `progress`, and fails if the
/// outcome is a timeout or unknown.
pub(crate) async fn run(
    context: &Context,
    args: RebootArgs,
    format: Format,
    out: &mut impl Write,
    progress: &mut impl Write,
) -> anyhow::Result<()> {
    let mut changer = changer(context, args.state).await?;
    let Some(token) = args.token else {
        let plan = changer.plan_reboot().await?;
        return show(&plan, format, out, |p, out| pairs(&plan_pairs(p), out));
    };
    let reboot = changer.reboot(&token).await?;
    let report = if args.wait && reboot.report().outcome == State::Accepted {
        let wait = Wait {
            interval: args.interval,
            budget: args.budget,
        };
        changer
            .wait(&reboot, wait, |state| {
                // Progress only; a closed stderr does not stop the wait.
                let _ = writeln!(progress, "{state}: {}", state.meaning());
            })
            .await?
    } else {
        reboot.report().clone()
    };
    show(&report, format, out, |r, out| pairs(&report_pairs(r), out))?;
    match report.outcome {
        State::Timeout | State::Unknown => bail!("{}", report.outcome.meaning()),
        _ => Ok(()),
    }
}

fn plan_pairs(plan: &RebootPlan) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("device", Some(plan.device.to_string())),
        ("model", plan.model.clone()),
        ("name", plan.name.clone()),
        ("uptime", plan.uptime.map(duration)),
        (
            "action",
            Some(format!("DeviceConfig1 Reboot ({})", plan.class)),
        ),
        ("warning", Some(plan.warning.to_owned())),
        (
            "token",
            Some(format!("{} (valid for {} s)", plan.token, plan.expires_in)),
        ),
    ]
}

fn report_pairs(report: &Report) -> Vec<(&'static str, Option<String>)> {
    let outcome = format!("{}: {}", report.outcome, report.outcome.meaning());
    vec![
        ("device", Some(report.device.to_string())),
        ("outcome", Some(outcome)),
        ("uptime before", report.uptime_before.map(duration)),
        ("uptime", report.uptime.map(duration)),
        ("waited", report.waited.map(|s| format!("{s} s"))),
        ("error", report.error.clone()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("10ms"), Ok(Duration::from_millis(10)));
        assert_eq!(parse_duration("5s"), Ok(Duration::from_secs(5)));
        assert_eq!(parse_duration("7"), Ok(Duration::from_secs(7)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_mins(2)));
        assert_eq!(parse_duration("60m"), Ok(LONGEST));
        for wrong in ["", "s", "5h", "0s", "61m", "-1s"] {
            assert!(parse_duration(wrong).is_err(), "{wrong}");
        }
    }
}
