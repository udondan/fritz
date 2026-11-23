//! `fritz hosts`: the host table, or one host with all details, found by
//! its name or address.

use std::io::Write;

use anyhow::{Context as _, bail};
use clap::Args;
use fritz_core::host::{self, Attachment, Detail, Lookup};
use fritz_core::status;

use super::{duration, host_table, pairs, rates, reader, shortfall, yes_no};
use crate::context::Context;
use crate::output::{Format, show};

/// The filters of `fritz hosts`.
#[derive(Debug, Args)]
pub(crate) struct HostsArgs {
    /// Only the hosts with this name, ignoring case: the hosts with exactly
    /// this name, or if there are none, the hosts whose name contains it.
    /// One host is shown with all details.
    #[arg(conflicts_with_all = ["mac", "ip", "search"])]
    name: Option<String>,
    /// The host with this MAC address, with all details.
    #[arg(long, value_name = "MAC", value_parser = Lookup::mac, conflicts_with_all = ["ip", "search"])]
    mac: Option<Lookup>,
    /// The host with this IPv4 address, with all details.
    #[arg(long, value_name = "IP", value_parser = Lookup::ip, conflicts_with = "search")]
    ip: Option<Lookup>,
    /// Only the hosts whose name, IP or MAC address contains this text,
    /// ignoring case.
    #[arg(long, value_name = "TEXT")]
    search: Option<String>,
    /// Only the hosts that are connected now.
    #[arg(long)]
    active: bool,
}

/// Prints the hosts that match the filters, or one host with all details.
/// Fails if a lookup by name or address finds no host.
pub(crate) async fn hosts(
    context: &Context,
    args: HostsArgs,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let which = if args.active { "active host" } else { "host" };
    if let Some(lookup) = args.mac.or(args.ip) {
        let detail = host::lookup(&mut reader, &lookup)
            .await
            .context("cannot read the host")?
            .filter(|d| !args.active || d.host.active)
            .with_context(|| format!("no {which} has {lookup}"))?;
        return show(&detail, format, out, host_detail);
    }
    let mut hosts = status::hosts(&mut reader)
        .await
        .context("cannot read the hosts")?;
    eprint!("{}", shortfall(&hosts));
    hosts.hosts.retain(|h| {
        (!args.active || h.active) && args.search.as_deref().is_none_or(|s| host::contains(h, s))
    });
    if let Some(name) = &args.name {
        hosts.hosts = host::named(hosts.hosts, name);
        match hosts.hosts.pop() {
            None => bail!("no {which} is named {name:?}"),
            Some(found) if hosts.hosts.is_empty() => {
                let detail = host::detail(&mut reader, found)
                    .await
                    .context("cannot read the host")?;
                return show(&detail, format, out, host_detail);
            }
            Some(found) => hosts.hosts.push(found),
        }
    }
    show(&hosts, format, out, host_table)
}

/// One host with all details, a line for each.
fn host_detail(detail: &Detail, out: &mut impl Write) -> anyhow::Result<()> {
    let host = &detail.host;
    pairs(
        &[
            ("Name", host.name.clone()),
            ("IP", host.ip.clone()),
            ("MAC", host.mac.clone()),
            ("Active", Some(yes_no(host.active))),
            ("Address source", host.address_source.clone()),
            ("Lease remaining", detail.lease_remaining.map(duration)),
            ("Interface", host.interface.clone()),
            ("Speed", host.speed.map(|s| format!("{s} Mbit/s"))),
            ("Guest", host.guest.map(yes_no)),
            ("Internet", host.internet_access.clone()),
            ("Attached to", detail.attached_to.as_ref().map(attachment)),
        ],
        out,
    )
}

/// The node a host is connected to and how, like
/// `device-50, WLAN 5 GHz ↓ 866.7 Mbit/s  ↑ 433.3 Mbit/s`.
fn attachment(attachment: &Attachment) -> String {
    // A LAN link reports no rates, as 0.
    let rates = rates(attachment.rate_down, attachment.rate_up).filter(|_| {
        attachment.rate_down.unwrap_or_default() + attachment.rate_up.unwrap_or_default() > 0
    });
    let link = [attachment.kind.clone(), attachment.band.clone(), rates]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let node = attachment.node.as_deref().unwrap_or("?");
    if link.is_empty() {
        node.to_owned()
    } else {
        format!("{node}, {link}")
    }
}

#[cfg(test)]
mod tests {
    use fritz_core::status::Host;

    use super::super::tests::text;
    use super::*;

    #[test]
    fn prints_a_host_with_all_details() {
        let detail = Detail {
            host: Host {
                name: Some("device-2".to_owned()),
                ip: Some("192.0.2.2".to_owned()),
                mac: Some("AA:BB:CC:00:00:03".to_owned()),
                active: true,
                interface: Some("802.11".to_owned()),
                address_source: Some("DHCP".to_owned()),
                guest: Some(false),
                internet_access: Some("granted".to_owned()),
                speed: Some(866),
            },
            lease_remaining: Some(3 * 3600 + 120),
            attached_to: Some(Attachment {
                node: Some("device-50".to_owned()),
                kind: Some("WLAN".to_owned()),
                interface: Some("AP:5G:0".to_owned()),
                band: Some("5 GHz".to_owned()),
                rate_down: Some(866_700),
                rate_up: Some(433_300),
            }),
        };
        assert_eq!(
            text(|out| host_detail(&detail, out)),
            "Name             device-2\n\
             IP               192.0.2.2\n\
             MAC              AA:BB:CC:00:00:03\n\
             Active           yes\n\
             Address source   DHCP\n\
             Lease remaining  3h 2m\n\
             Interface        802.11\n\
             Speed            866 Mbit/s\n\
             Guest            no\n\
             Internet         granted\n\
             Attached to      device-50, WLAN 5 GHz ↓ 866.7 Mbit/s  ↑ 433.3 Mbit/s\n"
        );
        let lan = Attachment {
            node: Some("device-51".to_owned()),
            kind: Some("LAN".to_owned()),
            rate_down: Some(0),
            rate_up: Some(0),
            ..Attachment::default()
        };
        assert_eq!(attachment(&lan), "device-51, LAN");
        assert_eq!(attachment(&Attachment::default()), "?");
    }
}
