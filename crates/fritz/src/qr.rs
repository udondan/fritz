//! `fritz wlan qr`: a QR code that a phone scans to join a wireless network.
//!
//! The code holds the passphrase, which only the narrow path of
//! [`fritz_core::wlan::join`] reads. It is shown only on a terminal or
//! written to a file that only the user can read, never piped. The MCP
//! server does not offer it. There is no default network: without one, the
//! networks are listed, and no passphrase is read.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use clap::{Args, ValueEnum};
use fritz_core::redact;
use fritz_core::wlan::join::{self, Join};
use fritz_core::wlan::{self, Views, Wlan};
use secrecy::ExposeSecret as _;

use self::render::{Modules, png, svg, text};
use crate::context::Context;
use crate::status;

mod render;

#[derive(Debug, Args)]
pub(crate) struct QrArgs {
    /// The network: `guest` for the guest network, its number, like `1`,
    /// its service, like `WLANConfiguration1`, or its SSID. Without it, the
    /// networks are listed.
    network: Option<String>,
    /// The format of the code. Without it, by the extension of `--out`, else
    /// text.
    #[arg(long, value_enum)]
    format: Option<Kind>,
    /// Write the code to this file, readable only by the user, instead of
    /// the terminal. Not on Windows, where `fritz` cannot restrict it to
    /// the user.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Pixels per module of PNG and SVG.
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u8).range(1..=64))]
    scale: u8,
    /// Modules of the light border around the code.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(0..=16))]
    border: u8,
    /// Also print the password below the code. Needs a terminal.
    #[arg(long)]
    show_password: bool,
}

/// The format of a code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Kind {
    /// Unicode half blocks.
    Text,
    Png,
    Svg,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Text => "text",
            Self::Png => "png",
            Self::Svg => "svg",
        })
    }
}

/// Prints or writes the QR code of a network. `terminal` tells whether
/// stdout is a terminal; without one, the code is only written to a file,
/// which is checked before the device is asked. Without a network, or if
/// none fits, lists the networks to `err` and fails, also without a
/// terminal: the list holds no secret.
pub(crate) async fn run(
    context: &Context,
    args: QrArgs,
    terminal: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> anyhow::Result<()> {
    let Some(network) = args.network.as_deref() else {
        let mut reader = status::reader(context).await?;
        let wlans = wlan::wlans(&mut reader, Views::default()).await?;
        list(&wlans, err)?;
        bail!("name the network of the code, {}", hint(&wlans));
    };
    let kind = kind(&args, terminal, cfg!(unix))?;
    let mut reader = status::reader(context).await?;
    let join = match join::join(&mut reader, network).await {
        Err(error @ (join::Error::Unknown(_) | join::Error::NoGuest)) => {
            let wlans = wlan::wlans(&mut reader, Views::default()).await?;
            list(&wlans, err)?;
            bail!("{error}, {}", hint(&wlans));
        }
        join => join?,
    };
    if join.enabled == Some(false) {
        eprintln!("note: {} is switched off", join.service);
    }
    show(&join, &args, kind, out)
}

/// Lists the networks to choose from: number, service, SSID (redacted, as
/// read), band, whether it is the guest network, and whether it is on.
fn list(wlans: &[Wlan], err: &mut impl Write) -> anyhow::Result<()> {
    let rows = wlans.iter().map(|w| {
        vec![
            number(&w.service).unwrap_or("-").to_owned(),
            w.service.clone(),
            w.ssid.clone().unwrap_or_default(),
            w.band.clone().unwrap_or_default(),
            if w.guest { join::GUEST } else { "" }.to_owned(),
            w.enabled.map_or_else(String::new, status::on_off),
        ]
    });
    let headers = ["NUMBER", "SERVICE", "SSID", "BAND", "GUEST", "STATUS"];
    status::table(&headers, rows, err)
}

/// The number of a service, like `1` of `WLANConfiguration1`.
fn number(service: &str) -> Option<&str> {
    service
        .strip_prefix("WLANConfiguration")
        .filter(|n| !n.is_empty())
}

/// How to name a network of `wlans`, like `fritz wlan qr 1`, and
/// `fritz wlan qr guest` if there is a guest network.
fn hint(wlans: &[Wlan]) -> String {
    let main = wlans
        .iter()
        .filter(|w| !w.guest)
        .find_map(|w| number(&w.service));
    let guest = wlans.iter().any(|w| w.guest).then_some(join::GUEST);
    let examples: Vec<String> = main
        .into_iter()
        .chain(guest)
        .map(|name| format!("fritz wlan qr {name}"))
        .collect();
    match examples.as_slice() {
        [] => "but the device has no wireless network".to_owned(),
        _ => format!("like: {}", examples.join(" or ")),
    }
}

/// The format of the code, or why it cannot be shown: PNG and SVG need a
/// file, the code and the password are only printed to a terminal, and a
/// file is only written where `private` tells that it can be made readable
/// only by the user, as on Unix.
fn kind(args: &QrArgs, terminal: bool, private: bool) -> anyhow::Result<Kind> {
    if args.out.is_some() && !private {
        bail!(
            "--out is not supported on this system, which cannot make the file readable only by you"
        );
    }
    let kind = match (args.format, &args.out) {
        (Some(kind), _) => kind,
        (None, Some(path)) => by_extension(path)?,
        (None, None) => Kind::Text,
    };
    match (&args.out, kind) {
        (None, Kind::Png | Kind::Svg) => bail!("--format {kind} needs --out <file>"),
        (None, Kind::Text) if !terminal => bail!(
            "stdout is not a terminal, and the code holds the password: \
             write it to a file with --out <file>"
        ),
        _ => {}
    }
    if args.show_password && !terminal {
        bail!("--show-password needs a terminal");
    }
    Ok(kind)
}

/// The format by the extension of a file.
fn by_extension(path: &Path) -> anyhow::Result<Kind> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("png") => Ok(Kind::Png),
        Some("svg") => Ok(Kind::Svg),
        Some("txt") => Ok(Kind::Text),
        _ => bail!(
            "cannot tell the format of {}, use --format text|png|svg",
            path.display()
        ),
    }
}

/// Prints the code, or writes it to the file of `--out`, and then what it
/// holds.
fn show(join: &Join, args: &QrArgs, kind: Kind, out: &mut impl Write) -> anyhow::Result<()> {
    let modules = Modules::new(join.payload().expose_secret(), args.border.into())?;
    match &args.out {
        None => out.write_all(text(&modules, true).as_bytes())?,
        Some(path) => {
            let bytes = match kind {
                Kind::Text => text(&modules, false).into_bytes(),
                Kind::Svg => svg(&modules, args.scale.into()).into_bytes(),
                Kind::Png => png(&modules, args.scale.into())?,
            };
            save(path, &bytes)?;
            let ssid = status::clean(&redact::value("NewSSID", &join.ssid));
            writeln!(out, "saved the code of {ssid} to {}", path.display())?;
        }
    }
    let pairs = [
        ("Service", Some(join.service.clone())),
        ("SSID", Some(redact::value("NewSSID", &join.ssid))),
        ("Security", Some(join.auth.to_string())),
        ("Hidden", Some(status::yes_no(join.hidden))),
        ("Guest", Some(status::yes_no(join.guest))),
    ];
    status::pairs(&pairs, out)?;
    if let (true, Some(passphrase)) = (args.show_password, &join.passphrase) {
        let passphrase = status::clean(passphrase.expose_secret());
        writeln!(out, "Password  {passphrase}")?;
    }
    Ok(())
}

/// Writes `bytes` to `path`, readable only by the user, also if the file
/// existed with other permissions.
fn save(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let write = || -> io::Result<()> {
        let mut file: File = options.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)
    };
    write().with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use fritz_core::wlan::join::Auth;
    use secrecy::SecretString;

    use super::render::{LIGHT, RESET};
    use super::*;

    /// A fake passphrase with every character that needs escaping.
    const PASSPHRASE: &str = r#"fake\pass;phrase,with:"quotes""#;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        qr: QrArgs,
    }

    fn args(args: &[&str]) -> QrArgs {
        Cli::try_parse_from(std::iter::once("qr").chain(args.iter().copied()))
            .unwrap()
            .qr
    }

    fn network() -> Join {
        Join {
            service: "WLANConfiguration3".to_owned(),
            ssid: "Café;\"Gäste\"\u{7}".to_owned(),
            auth: Auth::Wpa,
            hidden: true,
            enabled: Some(true),
            guest: true,
            passphrase: Some(SecretString::from(PASSPHRASE)),
        }
    }

    /// The payload of [`network`], as a phone reads it.
    fn payload() -> String {
        network().payload().expose_secret().to_owned()
    }

    fn shown(join: &Join, args: &QrArgs, kind: Kind) -> String {
        let mut out = Vec::new();
        show(join, args, kind, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// Decodes an image of a single code, `pixel` is the luminance.
    fn decode(side: usize, pixel: impl FnMut(usize, usize) -> u8) -> String {
        let mut image = rqrr::PreparedImage::prepare_from_greyscale(side, side, pixel);
        let grids = image.detect_grids();
        assert_eq!(grids.len(), 1);
        grids[0].decode().unwrap().1
    }

    /// Decodes modules, as an image of four pixels per module.
    fn decode_modules(size: usize, dark: &[bool]) -> String {
        decode(
            size * 4,
            |x, y| {
                if dark[y / 4 * size + x / 4] { 0 } else { 255 }
            },
        )
    }

    #[test]
    fn tells_the_format() {
        for (given, terminal, expected) in [
            (&[][..], true, Kind::Text),
            (&["--out", "code.png"][..], false, Kind::Png),
            (&["--out", "code.SVG"][..], false, Kind::Svg),
            (&["--out", "code.txt"][..], false, Kind::Text),
            (&["--out", "code", "--format", "png"][..], false, Kind::Png),
            (
                &["--out", "code.png", "--format", "svg"][..],
                false,
                Kind::Svg,
            ),
            (
                &["--format", "text", "--show-password"][..],
                true,
                Kind::Text,
            ),
        ] {
            assert_eq!(
                kind(&args(given), terminal, true).unwrap(),
                expected,
                "{given:?}"
            );
        }
        for (given, terminal, message) in [
            (
                &[][..],
                false,
                "stdout is not a terminal, and the code holds the password: \
                 write it to a file with --out <file>",
            ),
            (
                &["--format", "png"][..],
                true,
                "--format png needs --out <file>",
            ),
            (
                &["--format", "svg"][..],
                true,
                "--format svg needs --out <file>",
            ),
            (
                &["--out", "code.png", "--show-password"][..],
                false,
                "--show-password needs a terminal",
            ),
            (
                &["--out", "code"][..],
                true,
                "cannot tell the format of code, use --format text|png|svg",
            ),
            (
                &["--out", "code.jpg"][..],
                true,
                "cannot tell the format of code.jpg, use --format text|png|svg",
            ),
        ] {
            let error = kind(&args(given), terminal, true).unwrap_err();
            assert_eq!(error.to_string(), message, "{given:?}");
        }
        assert!(Cli::try_parse_from(["qr", "--scale", "0"]).is_err());
        assert!(Cli::try_parse_from(["qr", "--border", "17"]).is_err());
        // Without files only the user can read, only the terminal.
        let error = kind(&args(&["--out", "code.png"]), true, false).unwrap_err();
        assert_eq!(
            error.to_string(),
            "--out is not supported on this system, which cannot make the file \
             readable only by you"
        );
        assert_eq!(kind(&args(&[]), true, false).unwrap(), Kind::Text);
    }

    #[test]
    fn hides_session_ids_in_the_ssid_only_when_shown() {
        let dir = tempfile::tempdir().unwrap();
        let mut join = network();
        join.ssid = "Home?sid=123&sid=456".to_owned();
        let path = dir.path().join("code.png");
        let text = shown(&join, &args(&["--out", path.to_str().unwrap()]), Kind::Png);
        assert!(
            text.starts_with("saved the code of Home?sid=***&sid=*** to "),
            "{text}"
        );
        assert!(
            text.contains("\nSSID      Home?sid=***&sid=***\n"),
            "{text}"
        );
        assert!(!text.contains("123"), "{text}");
        assert!(
            join.payload()
                .expose_secret()
                .contains(r"S:Home?sid=123&sid=456;"),
        );
    }

    #[test]
    fn prints_the_code_to_a_terminal() {
        let join = network();
        let text = shown(&join, &args(&[]), Kind::Text);
        let (code, info) = text.split_at(text.rfind(RESET).unwrap() + RESET.len() + 1);
        assert_eq!(
            info,
            "Service   WLANConfiguration3\n\
             SSID      Café;\"Gäste\"\u{FFFD}\n\
             Security  WPA\n\
             Hidden    yes\n\
             Guest     yes\n"
        );
        // Every line is dark on light; the half blocks decode to the payload.
        let mut rows: Vec<Vec<bool>> = Vec::new();
        for line in code.lines() {
            let line = line
                .strip_prefix(LIGHT)
                .unwrap()
                .strip_suffix(RESET)
                .unwrap();
            let (top, bottom) = line
                .chars()
                .map(|c| (matches!(c, '█' | '▀'), matches!(c, '█' | '▄')))
                .unzip();
            rows.extend([top, bottom]);
        }
        let size = rows[0].len();
        // The last line holds one row of the code; the other is light.
        assert!(rows.pop().unwrap().iter().all(|d| !d));
        assert_eq!(rows.len(), size);
        let dark: Vec<bool> = rows.concat();
        assert_eq!(decode_modules(size, &dark), payload());
        assert!(!text.contains("fake"));
        // Only with --show-password, and with control characters replaced.
        let text = shown(&join, &args(&["--show-password"]), Kind::Text);
        assert!(
            text.ends_with(&format!("\nPassword  {PASSPHRASE}\n")),
            "{text}"
        );
        let mut join = network();
        join.passphrase = Some(SecretString::from("fake\u{1b}[2J"));
        let text = shown(&join, &args(&["--show-password"]), Kind::Text);
        assert!(text.ends_with("\nPassword  fake\u{FFFD}[2J\n"), "{text}");
        // An open network has no password to show.
        join.auth = Auth::Open;
        join.passphrase = None;
        let text = shown(&join, &args(&["--show-password"]), Kind::Text);
        assert!(text.ends_with("Security  nopass\nHidden    yes\nGuest     yes\n"));
    }

    #[test]
    fn writes_codes_that_decode_to_the_payload() {
        let dir = tempfile::tempdir().unwrap();
        let join = network();
        // PNG, decoded by the png crate.
        let path = dir.path().join("code.png");
        let given = [
            "--out",
            path.to_str().unwrap(),
            "--scale",
            "3",
            "--border",
            "2",
        ];
        let text = shown(&join, &args(&given), Kind::Png);
        assert_eq!(
            text.lines().next().unwrap(),
            format!(
                "saved the code of Café;\"Gäste\"\u{FFFD} to {}",
                path.display()
            )
        );
        assert!(!text.contains("fake"));
        let bytes = std::fs::read(&path).unwrap();
        let mut decoder = png::Decoder::new(io::Cursor::new(bytes))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; decoder.output_buffer_size().unwrap()];
        let info = decoder.next_frame(&mut pixels).unwrap();
        assert_eq!(info.color_type, png::ColorType::Grayscale);
        assert_eq!(info.width, info.height);
        let png_side = info.width as usize;
        let modules = Modules::new(&payload(), 2).unwrap();
        assert_eq!(png_side, modules.size * 3);
        assert_eq!(decode(png_side, |x, y| pixels[y * png_side + x]), payload());

        // SVG: its paths, read back into modules. This is no SVG renderer:
        // it reads only the shapes that `svg` writes.
        let path = dir.path().join("code.svg");
        shown(&join, &args(&["--out", path.to_str().unwrap()]), Kind::Svg);
        let svg = std::fs::read_to_string(&path).unwrap();
        let size = Modules::new(&payload(), 4).unwrap().size;
        let pixels = size * 8;
        assert!(svg.contains(&format!(
            "width=\"{pixels}\" height=\"{pixels}\" viewBox=\"0 0 {size} {size}\""
        )));
        assert!(svg.contains(&format!(
            "<rect width=\"{size}\" height=\"{size}\" fill=\"#fff\"/>"
        )));
        let d = svg.split("d=\"").nth(1).unwrap().split('"').next().unwrap();
        let mut dark = vec![false; size * size];
        for shape in d.split('M').filter(|s| !s.is_empty()) {
            let (start, rest) = shape.split_once(' ').unwrap();
            let (y, rest) = rest.split_once('h').unwrap();
            let (run, rest) = rest.split_once('v').unwrap();
            assert_eq!(rest, format!("1h-{run}z"));
            let (start, y, run): (usize, usize, usize) = (
                start.parse().unwrap(),
                y.parse().unwrap(),
                run.parse().unwrap(),
            );
            for x in start..start + run {
                dark[y * size + x] = true;
            }
        }
        assert_eq!(decode_modules(size, &dark), payload());

        // Text, without colors.
        let path = dir.path().join("code.txt");
        shown(&join, &args(&["--out", path.to_str().unwrap()]), Kind::Text);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains('\x1b'));
        assert_eq!(decode_modules(size, &art(&text, size)), payload());
    }

    /// The modules of half blocks without colors, `size` per side. Lines may
    /// lack their trailing light modules.
    fn art(text: &str, size: usize) -> Vec<bool> {
        let mut dark = Vec::new();
        for line in text.lines() {
            let cells: Vec<char> = line.chars().collect();
            for blocks in [['█', '▀'], ['█', '▄']] {
                for x in 0..size {
                    dark.push(cells.get(x).is_some_and(|c| blocks.contains(c)));
                }
            }
        }
        dark.resize(size * size, false);
        dark
    }

    #[test]
    fn shows_a_code_that_works_in_the_readme() {
        let readme = concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md");
        let readme = std::fs::read_to_string(readme).unwrap();
        let block = readme.split("$ fritz wlan qr guest\n").nth(1).unwrap();
        let block = block.split("Service   ").next().unwrap();
        let join = Join {
            service: "WLANConfiguration3".to_owned(),
            ssid: "Home-Guest".to_owned(),
            auth: Auth::Wpa,
            hidden: false,
            enabled: Some(true),
            guest: true,
            passphrase: Some(SecretString::from("example-passphrase")),
        };
        let payload = join.payload();
        let payload = payload.expose_secret();
        assert_eq!(payload, "WIFI:T:WPA;S:Home-Guest;P:example-passphrase;;");
        // What the terminal shows, without colors and trailing spaces, dark
        // on light like a code block.
        let modules = Modules::new(payload, 4).unwrap();
        let mut shown = String::new();
        for line in text(&modules, false).lines() {
            shown.push_str(line.trim_end());
            shown.push('\n');
        }
        assert_eq!(block, shown);
        assert_eq!(
            decode_modules(modules.size, &art(block, modules.size)),
            payload
        );
    }

    #[test]
    fn writes_files_only_for_the_user() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("code.png");
        std::fs::write(&path, "a longer text than the new one").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            save(&path, b"new").unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            std::fs::remove_file(&path).unwrap();
            save(&path, b"new").unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        save(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let missing = dir.path().join("missing/code.png");
        let error = save(&missing, b"new").unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("cannot write {}", missing.display())
        );
    }

    #[test]
    fn lists_the_networks_to_choose_from() {
        let wlan = |service: &str, guest: bool, enabled: Option<bool>| Wlan {
            service: service.to_owned(),
            ssid: Some("Home".to_owned()),
            guest,
            enabled,
            ..Wlan::default()
        };
        let wlans = [
            wlan("WLANConfiguration", false, None),
            wlan("WLANConfiguration3", true, Some(true)),
        ];
        let mut text = Vec::new();
        list(&wlans, &mut text).unwrap();
        assert_eq!(
            String::from_utf8(text).unwrap(),
            "NUMBER  SERVICE             SSID  BAND  GUEST  STATUS\n\
             -       WLANConfiguration   Home\n\
             3       WLANConfiguration3  Home        guest  on\n"
        );
        assert_eq!(hint(&wlans), "like: fritz wlan qr guest");
        assert_eq!(hint(&[]), "but the device has no wireless network");
    }

    #[test]
    fn names_the_formats() {
        assert_eq!(Kind::Text.to_string(), "text");
        assert_eq!(Kind::Png.to_string(), "png");
        assert_eq!(Kind::Svg.to_string(), "svg");
    }
}
