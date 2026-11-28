//! `--watch`: read again and again, and print what changed between two
//! reads, like rates from counters, until the count is reached or Ctrl-C.

use std::io::Write;
use std::time::Duration;

use anyhow::Context as _;

use clap::Args;
use serde::Serialize;
use tokio::time::{self, Instant, MissedTickBehavior};

use crate::output::{self, Format};

/// The options of commands that can watch. A command can set another
/// default of `--interval` with
/// `#[command(mut_arg("interval", |a| a.default_value("2")))]`.
#[derive(Debug, Clone, Copy, Args)]
pub(crate) struct Options {
    /// Read again every interval, and print the rates and the changes since
    /// the read before. JSON prints one line per interval.
    #[arg(long)]
    pub(crate) watch: bool,
    /// The seconds between two reads.
    #[arg(
        long,
        default_value_t = 5,
        requires = "watch",
        value_parser = clap::value_parser!(u64).range(1..=86_400),
    )]
    pub(crate) interval: u64,
    /// Stop after this many intervals. Without, keep going until
    /// interrupted.
    #[arg(long, requires = "watch", value_parser = clap::value_parser!(u64).range(1..))]
    pub(crate) count: Option<u64>,
}

impl Options {
    /// The time between two reads.
    pub(crate) fn every(&self) -> Duration {
        Duration::from_secs(self.interval)
    }
}

/// A read that can be watched: what changed since the read before, like
/// rates from counters.
pub(crate) trait Reading {
    /// What changed between two reads.
    type Sample: Serialize;

    /// What changed since `before`, which was read `seconds` earlier.
    fn since(&self, before: &Self, seconds: f64) -> Self::Sample;

    /// Whether text samples are separated by a blank line. Samples of one
    /// line each are rows without one.
    const BLANK_LINE: bool = true;

    /// Prints a sample as text.
    fn text(sample: &Self::Sample, out: &mut impl Write) -> anyhow::Result<()>;
}

/// Reads with `read` every interval, and prints a sample for each read
/// after the first, see [`run_until`]. Ctrl-C ends it without an error.
pub(crate) async fn run<R: Reading>(
    options: Options,
    read: impl AsyncFnMut() -> anyhow::Result<R>,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    run_until(options, read, format, out, tokio::signal::ctrl_c()).await
}

/// Reads with `read` every interval, and prints a sample for each read
/// after the first, with a blank line between text samples unless they are
/// rows. Stops after `--count` samples, on the first error, or when `stop`
/// is done. Samples are written whole: `stop` only ends a wait or a read.
pub(crate) async fn run_until<R: Reading>(
    options: Options,
    read: impl AsyncFnMut() -> anyhow::Result<R>,
    format: Format,
    out: &mut impl Write,
    stop: impl Future<Output = std::io::Result<()>>,
) -> anyhow::Result<()> {
    tokio::select! {
        result = samples(options, read, format, out) => result,
        stopped = stop => stopped.context("cannot wait for Ctrl-C"),
    }
}

async fn samples<R: Reading>(
    options: Options,
    mut read: impl AsyncFnMut() -> anyhow::Result<R>,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut ticks = time::interval(options.every());
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticks.tick().await;
    let mut before = (read().await?, Instant::now());
    let mut samples = 0;
    while options.count.is_none_or(|count| samples < count) {
        ticks.tick().await;
        let now = (read().await?, Instant::now());
        let seconds = (now.1 - before.1).as_secs_f64();
        let sample = now.0.since(&before.0, seconds);
        output::sample(&sample, format, out, |sample, out| {
            if samples > 0 && R::BLANK_LINE {
                writeln!(out)?;
            }
            R::text(sample, out)
        })?;
        before = now;
        samples += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use serde_json::{Value, json};

    use super::*;

    #[derive(Debug, Parser)]
    struct Command {
        #[command(flatten)]
        watch: Options,
    }

    #[test]
    fn parses_the_options() {
        let parse = |args: &[&str]| Command::try_parse_from([&["fritz"], args].concat());
        let watch = parse(&[]).unwrap().watch;
        assert!(!watch.watch);
        assert_eq!(watch.every(), Duration::from_secs(5));
        let watch = parse(&["--watch", "--interval", "2", "--count", "3"])
            .unwrap()
            .watch;
        assert!(watch.watch);
        assert_eq!(watch.every(), Duration::from_secs(2));
        assert_eq!(watch.count, Some(3));
        assert!(parse(&["--interval", "2"]).is_err());
        assert!(parse(&["--count", "2"]).is_err());
        assert!(parse(&["--watch", "--interval", "0"]).is_err());
        assert!(parse(&["--watch", "--count", "0"]).is_err());
    }

    /// A counter, whose text samples are separated by blank lines unless
    /// `BLANK` is false.
    struct Count<const BLANK: bool>(u64);

    impl<const BLANK: bool> Reading for Count<BLANK> {
        type Sample = Value;

        const BLANK_LINE: bool = BLANK;

        fn since(&self, before: &Self, seconds: f64) -> Value {
            json!({ "grown": self.0 - before.0, "seconds": seconds })
        }

        fn text(sample: &Value, out: &mut impl Write) -> anyhow::Result<()> {
            let (grown, seconds) = (&sample["grown"], &sample["seconds"]);
            writeln!(out, "+{grown} in {seconds}s").map_err(anyhow::Error::from)
        }
    }

    /// Watches a [`Count`] every 2 seconds until `stop`, and fails the read
    /// `fail_at`.
    async fn watch<const BLANK: bool>(
        count: Option<u64>,
        fail_at: u64,
        format: Format,
        stop: impl Future<Output = std::io::Result<()>>,
    ) -> (String, anyhow::Result<()>) {
        let options = Options {
            watch: true,
            interval: 2,
            count,
        };
        let mut reads = 0;
        let mut out = Vec::new();
        let read = async || {
            reads += 1;
            if reads == fail_at {
                anyhow::bail!("cannot read");
            }
            Ok(Count::<BLANK>(reads * 100))
        };
        let result = run_until(options, read, format, &mut out, stop).await;
        (String::from_utf8(out).unwrap(), result)
    }

    #[tokio::test(start_paused = true)]
    async fn prints_a_sample_per_interval() {
        let never = std::future::pending;
        let (output, result) = watch::<true>(Some(2), 0, Format::Text, never()).await;
        result.unwrap();
        assert_eq!(output, "+100 in 2.0s\n\n+100 in 2.0s\n");
        let (output, result) = watch::<false>(Some(2), 0, Format::Text, never()).await;
        result.unwrap();
        assert_eq!(output, "+100 in 2.0s\n+100 in 2.0s\n");
        let (output, result) = watch::<true>(Some(1), 0, Format::Json, never()).await;
        result.unwrap();
        assert_eq!(output, "{\"grown\":100,\"seconds\":2.0}\n");
    }

    #[tokio::test(start_paused = true)]
    async fn stops_at_the_first_error() {
        let never = std::future::pending();
        let (output, result) = watch::<true>(None, 3, Format::Text, never).await;
        assert_eq!(output, "+100 in 2.0s\n");
        assert_eq!(result.unwrap_err().to_string(), "cannot read");
    }

    #[tokio::test(start_paused = true)]
    async fn stops_when_interrupted() {
        let after = |seconds, result: std::io::Result<()>| async move {
            time::sleep(Duration::from_secs(seconds)).await;
            result
        };
        let (output, result) = watch::<false>(None, 0, Format::Json, after(5, Ok(()))).await;
        result.unwrap();
        assert_eq!(output.lines().count(), 2, "{output}");
        let failed = Err(std::io::Error::other("no signals"));
        let (output, result) = watch::<false>(None, 0, Format::Text, after(1, failed)).await;
        assert_eq!(output, "");
        let error = format!("{:#}", result.unwrap_err());
        assert_eq!(error, "cannot wait for Ctrl-C: no signals");
    }
}
