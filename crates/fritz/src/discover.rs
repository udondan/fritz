//! `fritz discover`: finds routers and repeaters on the local network, and
//! saves one to a profile.

use std::io::{self, BufRead, IsTerminal as _, Write};

use anyhow::{Context as _, bail};
use clap::Args;
use fritz_core::discover::{Device, Discovery, Gateway, Report, Resolver, Ssdp};

use crate::context::{Context, read_line};
use crate::output::{self, Format};
use crate::settings;
use crate::status::{clean, table};

#[derive(Debug, Args)]
pub(crate) struct DiscoverArgs {
    /// Save the address of the device found to a profile, like `fritz
    /// config init --host`: to the given one, else the one of --profile or
    /// the default one. With several devices, asks which one on a terminal.
    #[arg(long, value_name = "PROFILE")]
    #[expect(clippy::option_option, reason = "--save, --save PROFILE or neither")]
    save: Option<Option<String>>,
}

/// Runs `fritz discover` on the network of this machine, asking on stdin
/// when it is a terminal.
pub(crate) async fn system(
    context: &mut Context,
    args: DiscoverArgs,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut stdin = io::stdin().lock();
    let terminal = io::stdin()
        .is_terminal()
        .then_some(&mut stdin as &mut dyn BufRead);
    let err = &mut io::stderr();
    let streams = Io { out, terminal, err };
    run(context, args, &Discovery::system(), format, streams).await
}

/// Where `fritz discover` writes to and reads from.
pub(crate) struct Io<'a, O, E> {
    /// The output: the report and, with text output, the note on the saved
    /// profile.
    pub(crate) out: &'a mut O,
    /// The input of an interactive terminal, to ask which device to save.
    pub(crate) terminal: Option<&'a mut dyn BufRead>,
    /// Questions and, without text output, the note on the saved profile.
    pub(crate) err: &'a mut E,
}

/// Looks for devices, prints them and saves one if asked to.
pub(crate) async fn run<N: Resolver + Gateway, S: Ssdp, O: Write, E: Write>(
    context: &mut Context,
    args: DiscoverArgs,
    discovery: &Discovery<N, S>,
    format: Format,
    io: Io<'_, O, E>,
) -> anyhow::Result<()> {
    let Io { out, terminal, err } = io;
    let target = context.target()?;
    let host = target.url.host_str().unwrap_or_default();
    let report = discovery.run(host).await;
    output::show(&report, format, out, text)?;
    let Some(profile) = args.save else {
        return Ok(());
    };
    let device = choose(&report.devices, terminal, err)?;
    if let Some(profile) = profile {
        context.set_profile(profile);
    }
    context.set_host(device.host.clone());
    if format == Format::Text {
        settings::init(context, None, out)
    } else {
        settings::init(context, None, err)
    }
}

/// The warning about a host that resolves to addresses outside the local
/// network.
const NOT_LOCAL: &str = "not only on the local network: commands may run into timeouts, \
                         pass a device below with --host or save it with --save";

fn text(report: &Report, out: &mut impl Write) -> anyhow::Result<()> {
    let host = clean(&report.host);
    let addresses: Vec<String> = report.addresses.iter().map(ToString::to_string).collect();
    let addresses = addresses.join(", ");
    if report.addresses.is_empty() {
        writeln!(out, "{host} does not resolve")?;
    } else if report.local {
        writeln!(out, "{host} resolves to {addresses}, on the local network")?;
    } else {
        writeln!(out, "{host} resolves to {addresses}, {NOT_LOCAL}")?;
    }
    writeln!(out)?;
    if report.devices.is_empty() {
        writeln!(out, "no router or repeater found on the local network")?;
        return Ok(());
    }
    let dash = |text: &Option<String>| text.clone().unwrap_or_else(|| "-".to_owned());
    table(
        &["ADDRESS", "MODEL", "FIRMWARE", "ROLE", "FOUND BY"],
        report.devices.iter().map(|device| {
            let found_by: Vec<String> = device.found_by.iter().map(ToString::to_string).collect();
            vec![
                device.address.clone(),
                dash(&device.model),
                dash(&device.firmware),
                device.role.to_string(),
                found_by.join(", "),
            ]
        }),
        out,
    )
}

/// The device to save: the only one, else the one chosen on the terminal.
fn choose<'a>(
    devices: &'a [Device],
    terminal: Option<&mut dyn BufRead>,
    err: &mut impl Write,
) -> anyhow::Result<&'a Device> {
    let mut input = match (devices, terminal) {
        ([], _) => bail!("found no device to save"),
        ([device], _) => return Ok(device),
        (_, None) => bail!(
            "found several devices, choose one on a terminal or save it with \
             `fritz config init --host <address>`"
        ),
        (_, Some(input)) => input,
    };
    writeln!(err, "found several devices:")?;
    for (number, device) in devices.iter().enumerate() {
        let model = clean(device.model.as_deref().unwrap_or("unknown model"));
        let (number, address, role) = (number + 1, &device.address, device.role);
        writeln!(err, "{number:>3}  {address}  {model} ({role})")?;
    }
    write!(err, "which one to save? [1-{}] ", devices.len())?;
    err.flush()?;
    let answer = read_line(&mut input)?;
    let chosen = answer.trim().parse::<usize>().ok();
    chosen
        .and_then(|number| devices.get(number.checked_sub(1)?))
        .with_context(|| {
            format!(
                "{:?} is not one of the devices, saved nothing",
                clean(&answer)
            )
        })
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    use fritz_core::connection::Overrides;
    use fritz_core::discover::{Fixed, Options};
    use fritz_sim::{Fixture, Sim};

    use super::*;
    use crate::context::tests::context;

    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    async fn sim(fixture: &str) -> Sim {
        Sim::start(Fixture::bundled(fixture).unwrap())
            .await
            .unwrap()
    }

    /// A discovery that finds the simulators over SSDP.
    fn discovery(sims: &[&Sim], addresses: Vec<IpAddr>) -> Discovery<Fixed, Fixed> {
        let locations = sims
            .iter()
            .map(|sim| format!("{}tr64desc.xml", sim.url()))
            .collect();
        Discovery {
            network: Fixed {
                addresses,
                ..Fixed::default()
            },
            ssdp: Fixed {
                locations,
                ..Fixed::default()
            },
            options: Options {
                defaults: Vec::new(),
                search_time: Duration::ZERO,
                timeout: Duration::from_millis(500),
                ..Options::default()
            },
        }
    }

    struct Run {
        out: String,
        err: String,
        result: anyhow::Result<()>,
    }

    #[expect(clippy::option_option, reason = "--save, --save PROFILE or neither")]
    async fn discover(
        context: &mut Context,
        discovery: &Discovery<Fixed, Fixed>,
        save: Option<Option<&str>>,
        format: Format,
        terminal: Option<&str>,
    ) -> Run {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let args = DiscoverArgs {
            save: save.map(|profile| profile.map(str::to_owned)),
        };
        let mut input = terminal.map(str::as_bytes);
        let terminal = input.as_mut().map(|input| input as &mut dyn BufRead);
        let io = Io {
            out: &mut out,
            terminal,
            err: &mut err,
        };
        let result = run(context, args, discovery, format, io).await;
        Run {
            out: String::from_utf8(out).unwrap(),
            err: String::from_utf8(err).unwrap(),
            result,
        }
    }

    #[tokio::test]
    async fn lists_the_devices_and_saves_the_only_one() {
        let router = sim("fritzbox-7690/8.25").await;
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path(), Overrides::default());
        let discovery = discovery(&[&router], vec![LOOPBACK]);
        let run = discover(&mut context, &discovery, Some(None), Format::Text, None).await;
        run.result.unwrap();
        let address = router.url().as_str().trim_start_matches("http://");
        let address = address.trim_end_matches('/');
        let path = dir.path().join("config.toml");
        assert_eq!(
            run.out,
            format!(
                "fritz.box resolves to 127.0.0.1, on the local network\n\n\
                 ADDRESS{0}  MODEL           FIRMWARE  ROLE    FOUND BY\n\
                 {address}  FRITZ!Box 7690  8.25      router  ssdp\n\
                 saved the profile default to {1}\n",
                " ".repeat(address.len() - "ADDRESS".len()),
                path.display()
            )
        );
        assert_eq!(context.profile().unwrap().host, format!("http://{address}"));
    }

    #[tokio::test]
    async fn asks_which_device_to_save() {
        let router = sim("fritzbox-7690/8.25").await;
        let repeater = sim("fritz-repeater-2400/8.25").await;
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path(), Overrides::default());
        let discovery = discovery(&[&router, &repeater], Vec::new());

        let run = discover(&mut context, &discovery, Some(None), Format::Text, None).await;
        assert!(run.out.starts_with("fritz.box does not resolve\n\n"));
        assert!(
            run.out
                .contains("FRITZ!Repeater 2400  -         repeater  ssdp\n")
        );
        assert_eq!(
            run.result.unwrap_err().to_string(),
            "found several devices, choose one on a terminal or save it with \
             `fritz config init --host <address>`"
        );

        let run = discover(
            &mut context,
            &discovery,
            Some(None),
            Format::Json,
            Some("3\n"),
        )
        .await;
        assert!(
            run.err
                .starts_with("found several devices:\n  1  127.0.0.1:")
        );
        assert!(
            run.err
                .ends_with("FRITZ!Repeater 2400 (repeater)\nwhich one to save? [1-2] ")
        );
        assert_eq!(
            run.result.unwrap_err().to_string(),
            "\"3\" is not one of the devices, saved nothing"
        );
        assert!(context.profile().is_none());

        let run = discover(
            &mut context,
            &discovery,
            Some(Some("repeater")),
            Format::Json,
            Some("2\n"),
        )
        .await;
        run.result.unwrap();
        let report: serde_json::Value = serde_json::from_str(&run.out).unwrap();
        assert_eq!(report["devices"][1]["model"], "FRITZ!Repeater 2400");
        assert!(run.err.ends_with(&format!(
            "saved the profile repeater to {}\n",
            dir.path().join("config.toml").display()
        )));
        let saved = &context.config().profiles["repeater"].host;
        assert_eq!(saved, report["devices"][1]["host"].as_str().unwrap());
    }

    #[tokio::test]
    async fn warns_about_a_host_outside_the_local_network() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path(), Overrides::default());
        let discovery = discovery(&[], vec!["203.0.113.1".parse().unwrap()]);
        let run = discover(&mut context, &discovery, None, Format::Text, None).await;
        run.result.unwrap();
        assert_eq!(
            run.out,
            "fritz.box resolves to 203.0.113.1, not only on the local network: commands \
             may run into timeouts, pass a device below with --host or save it with --save\n\n\
             no router or repeater found on the local network\n"
        );
        let run = discover(&mut context, &discovery, Some(None), Format::Text, None).await;
        assert_eq!(
            run.result.unwrap_err().to_string(),
            "found no device to save"
        );
    }
}
