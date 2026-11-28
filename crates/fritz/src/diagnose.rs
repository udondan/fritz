//! `fritz diagnose`: checks a host end to end, or with `router` the device
//! itself, and fails if a check fails.

use std::io::Write;

use anyhow::bail;
use fritz_core::diagnose::{self, Options, ROUTER, Report, Status, System};

use crate::context::Context;
use crate::output::{Format, show};
use crate::status::{self, clean};

/// Diagnoses `host`, probing `ports` on it, and fails if a check fails.
pub(crate) async fn run(
    context: &Context,
    host: &str,
    ports: Vec<u16>,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let network = System::default();
    let report = if host == ROUTER {
        if !ports.is_empty() {
            bail!("--port is for hosts, not for the device itself");
        }
        diagnose::router(&context.target()?, &network).await
    } else {
        let mut reader = status::reader(context).await?;
        let options = Options {
            ports,
            private_only: false,
        };
        diagnose::host(&mut reader, &network, host, &options).await?
    };
    show(&report, format, out, text)?;
    match report.failures() {
        0 => Ok(()),
        1 => bail!("1 check failed"),
        n => bail!("{n} checks failed"),
    }
}

fn symbol(status: Status) -> char {
    match status {
        Status::Pass => '✓',
        Status::Fail => '✗',
        Status::Skip => '–',
        Status::Unknown => '?',
    }
}

pub(crate) fn text(report: &Report, out: &mut impl Write) -> anyhow::Result<()> {
    let ip = report.host.as_ref().and_then(|d| d.host.ip.as_deref());
    match ip {
        Some(ip) => writeln!(out, "Diagnose {}  →  {}", clean(&report.target), clean(ip))?,
        None => writeln!(out, "Diagnose {}", clean(&report.target))?,
    }
    let width = report
        .checks
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or_default();
    for check in &report.checks {
        let name = clean(&check.name);
        let mark = symbol(check.status);
        match &check.detail {
            Some(detail) => writeln!(out, "  {mark} {name:width$}  {}", clean(detail))?,
            None => writeln!(out, "  {mark} {name}")?,
        }
        if let Some(hint) = &check.hint {
            writeln!(out, "      {}", clean(hint))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use fritz_core::diagnose::Check;
    use fritz_core::host::Detail;
    use fritz_core::status::Host;

    use super::*;
    use crate::status::tests::text as render;

    fn check(name: &str, status: Status, detail: Option<&str>, hint: Option<&str>) -> Check {
        Check {
            name: name.to_owned(),
            status,
            detail: detail.map(str::to_owned),
            hint: hint.map(str::to_owned),
        }
    }

    #[test]
    fn prints_the_checks() {
        let host = Host {
            ip: Some("192.168.178.23".to_owned()),
            ..Host::default()
        };
        let report = Report {
            target: "nas".to_owned(),
            host: Some(Detail {
                host,
                ..Detail::default()
            }),
            checks: vec![
                check("active", Status::Pass, None, None),
                check("link", Status::Skip, Some("not connected"), None),
                check("name resolves", Status::Unknown, Some("10.0.0.1"), None),
                check(
                    "TCP 445",
                    Status::Fail,
                    Some("closed"),
                    Some("refused\u{1b}"),
                ),
            ],
        };
        assert_eq!(
            render(|out| text(&report, out)),
            "Diagnose nas  →  192.168.178.23\n  ✓ active\n  – link           not connected\n  \
             ? name resolves  10.0.0.1\n  ✗ TCP 445        closed\n      refused\u{FFFD}\n"
        );
        let unknown = Report {
            host: None,
            checks: Vec::new(),
            ..report
        };
        assert_eq!(render(|out| text(&unknown, out)), "Diagnose nas\n");
    }
}
