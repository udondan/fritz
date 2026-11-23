//! `fritz dns`: why a name does or does not resolve, from the view of the
//! router and of this machine. Only reads and asks; changes nothing.

use std::io::Write;
use std::net::{IpAddr, SocketAddr};

use anyhow::Context as _;
use clap::Args;
use fritz_core::dns::{self, Answer, ClientConfig, Report, Request, Status, System};

use crate::context::Context;
use crate::output::{Format, show};
use crate::status::{pairs, reader};

#[derive(Debug, Args)]
pub(crate) struct DnsArgs {
    /// The name, like `nas.fritz.box` or `nas`. Only this name is asked
    /// for; a name without a dot also with the local domain of the router.
    name: String,
    /// The address the answers should have.
    #[arg(long, value_name = "ADDRESS")]
    expected_ip: Option<IpAddr>,
    /// The DNS server asked as the router, like `192.168.178.1` or
    /// `192.168.178.1:53`. Default: the LAN address of the device.
    #[arg(long, value_name = "ADDRESS", value_parser = dns::parse_server)]
    router_dns: Option<SocketAddr>,
}

/// Checks the name and prints the evidence and the verdict.
pub(crate) async fn dns(
    context: &Context,
    args: DnsArgs,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let request = Request {
        name: dns::check_name(&args.name)?,
        expected: args.expected_ip,
        router_dns: args.router_dns,
    };
    let mut reader = reader(context).await?;
    let report = dns::diagnose(&mut reader, &System, &request)
        .await
        .context("cannot read the LAN settings of the device")?;
    show(&report, format, out, |report, out| {
        pairs(&report_pairs(report), out)
    })
}

/// The evidence and the verdict as lines.
fn report_pairs(report: &Report) -> Vec<(&'static str, Option<String>)> {
    let router = &report.router;
    let host = router.host.as_ref().map_or_else(
        || "no host of this name".to_owned(),
        |host| {
            format!(
                "{}, {}, {}",
                host.name,
                host.ip.as_deref().unwrap_or("no address"),
                if host.active { "active" } else { "inactive" }
            )
        },
    );
    let expected = report.expected.as_ref().map(|expected| {
        let matches = |found: Option<bool>| match found {
            Some(true) => "yes",
            Some(false) => "no",
            None => "no answer",
        };
        format!(
            "{}: router {}, this machine {}",
            expected.address,
            matches(expected.router),
            matches(expected.client)
        )
    });
    vec![
        ("Name", Some(report.name.clone())),
        (
            "Local domain",
            Some(router.domain.clone().unwrap_or_else(|| "none".to_owned())),
        ),
        ("DNS handed out", Some(list(&router.dns_servers))),
        ("Host table", Some(host)),
        ("Router answer", Some(answer(&report.router_answer))),
        ("Client answer", Some(answer(&report.client_answer))),
        (
            "Client resolvers",
            Some(resolvers(report.client_config.as_ref())),
        ),
        ("Expected", expected),
        (
            "Verdict",
            Some(format!(
                "{}: {}",
                serde_json::to_value(report.verdict.cause)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                report.verdict.message
            )),
        ),
    ]
}

/// Values separated by commas, or `none`.
fn list(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(", ")
    }
}

/// An answer, like `192.0.2.2 (192.0.2.1 for nas.fritz.box)`.
fn answer(answer: &Answer) -> String {
    let found = match answer.status {
        Status::Answered => answer
            .addresses
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        _ => answer
            .note
            .clone()
            .unwrap_or_else(|| "no address".to_owned()),
    };
    match &answer.server {
        Some(server) => format!("{found} ({server} for {})", answer.name),
        None => format!("{found} (for {})", answer.name),
    }
}

/// The resolvers of this machine, like
/// `default: 192.0.2.1, search fritz.box; local: mdns`.
fn resolvers(config: Option<&ClientConfig>) -> String {
    let Some(config) = config else {
        return "unknown".to_owned();
    };
    let resolvers: Vec<String> = config
        .resolvers
        .iter()
        .map(|resolver| {
            let mut parts = Vec::new();
            if !resolver.nameservers.is_empty() {
                parts.push(resolver.nameservers.join(", "));
            }
            if !resolver.search.is_empty() {
                parts.push(format!("search {}", resolver.search.join(" ")));
            }
            parts.extend(resolver.options.clone());
            format!(
                "{}: {}",
                resolver.domain.as_deref().unwrap_or("default"),
                list(&parts)
            )
        })
        .collect();
    let shown = if resolvers.is_empty() {
        "none".to_owned()
    } else {
        resolvers.join("; ")
    };
    format!("{shown} (from {})", config.source)
}

#[cfg(test)]
mod tests {
    use fritz_core::dns::{Cause, Expected, HostEntry, Resolver, RouterView, Verdict};

    use super::*;

    fn text(report: &Report) -> String {
        let mut out = Vec::new();
        pairs(&report_pairs(report), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn prints_the_evidence() {
        let report = Report {
            name: "nas".to_owned(),
            router: RouterView {
                domain: Some("fritz.box".to_owned()),
                dns_servers: vec!["192.0.2.1".to_owned()],
                address: Some("192.0.2.1".to_owned()),
                host: Some(HostEntry {
                    name: "nas".to_owned(),
                    ip: Some("192.0.2.2".to_owned()),
                    active: true,
                }),
            },
            router_answer: Answer {
                server: Some("192.0.2.1".to_owned()),
                name: "nas.fritz.box".to_owned(),
                status: Status::Answered,
                addresses: vec!["192.0.2.2".parse().unwrap()],
                note: None,
            },
            client_answer: Answer {
                server: None,
                name: "nas".to_owned(),
                status: Status::Failed,
                addresses: Vec::new(),
                note: Some("not known".to_owned()),
            },
            client_config: Some(ClientConfig {
                source: "scutil --dns".to_owned(),
                resolvers: vec![
                    Resolver {
                        nameservers: vec!["192.0.2.1".to_owned()],
                        search: vec!["example.org".to_owned()],
                        ..Resolver::default()
                    },
                    Resolver {
                        domain: Some("local".to_owned()),
                        options: Some("mdns".to_owned()),
                        ..Resolver::default()
                    },
                ],
            }),
            expected: Some(Expected {
                address: "192.0.2.2".parse().unwrap(),
                router: Some(true),
                client: None,
            }),
            verdict: Verdict {
                cause: Cause::SearchDomainMissing,
                message: "add the search domain".to_owned(),
            },
        };
        assert_eq!(
            text(&report),
            "Name              nas\n\
             Local domain      fritz.box\n\
             DNS handed out    192.0.2.1\n\
             Host table        nas, 192.0.2.2, active\n\
             Router answer     192.0.2.2 (192.0.2.1 for nas.fritz.box)\n\
             Client answer     not known (for nas)\n\
             Client resolvers  default: 192.0.2.1, search example.org; local: mdns (from scutil --dns)\n\
             Expected          192.0.2.2: router yes, this machine no answer\n\
             Verdict           search_domain_missing: add the search domain\n"
        );
    }

    #[test]
    fn prints_what_is_unknown() {
        let report = Report {
            name: "nas.example".to_owned(),
            router: RouterView {
                host: Some(HostEntry {
                    name: "nas".to_owned(),
                    ip: None,
                    active: false,
                }),
                ..RouterView::default()
            },
            router_answer: Answer {
                server: None,
                name: "nas.example".to_owned(),
                status: Status::NoAddress,
                addresses: Vec::new(),
                note: None,
            },
            client_answer: Answer {
                server: None,
                name: "nas.example".to_owned(),
                status: Status::Answered,
                addresses: vec!["192.0.2.2".parse().unwrap()],
                note: None,
            },
            client_config: None,
            expected: Some(Expected {
                address: "192.0.2.3".parse().unwrap(),
                router: None,
                client: Some(false),
            }),
            verdict: Verdict {
                cause: Cause::ClientOnly,
                message: "elsewhere".to_owned(),
            },
        };
        let text = text(&report);
        assert!(text.contains("Local domain      none\n"), "{text}");
        assert!(text.contains("DNS handed out    none\n"), "{text}");
        assert!(
            text.contains("Host table        nas, no address, inactive\n"),
            "{text}"
        );
        assert!(
            text.contains("Router answer     no address (for nas.example)\n"),
            "{text}"
        );
        assert!(text.contains("Client resolvers  unknown\n"), "{text}");
        assert!(
            text.contains("router no answer, this machine no\n"),
            "{text}"
        );
        let empty = ClientConfig {
            source: "/etc/resolv.conf".to_owned(),
            resolvers: vec![Resolver::default()],
        };
        assert_eq!(
            resolvers(Some(&empty)),
            "default: none (from /etc/resolv.conf)"
        );
        let none = ClientConfig {
            source: "/etc/resolv.conf".to_owned(),
            resolvers: Vec::new(),
        };
        assert_eq!(resolvers(Some(&none)), "none (from /etc/resolv.conf)");
    }
}
