//! Finds FRITZ! routers and repeaters on the local network: at the
//! addresses of the configured host and the default gateway, through an
//! SSDP search, and at well-known default addresses.
//!
//! Every candidate is verified by its device description, which needs no
//! login: only FRITZ! devices with TR-064 are listed, and a device found in
//! several ways is listed once. Nothing goes to addresses outside the local
//! network, and every step ends after a fixed time.
//!
//! The name resolution, the default gateway and SSDP are behind the traits
//! [`Resolver`], [`Gateway`] and [`Ssdp`], so that tests can give the
//! answers, see [`Fixed`].

use std::fmt;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use serde::Serialize;
use tokio::net::UdpSocket;
use tokio::task::JoinSet;
use tr064::{Client, DeviceDescription, Url};

use crate::connection::HTTP_PORT;

/// The device type of the root device of TR-064 devices, which FRITZ!
/// routers and repeaters announce over SSDP.
pub const ROOT_DEVICE: &str = "urn:dslforum-org:device:InternetGatewayDevice:1";

/// The device types an SSDP search asks for: the root device, and the LAN
/// device that routers and repeaters embed.
pub const SEARCH_TARGETS: [&str; 2] = [ROOT_DEVICE, "urn:dslforum-org:device:LANDevice:1"];

/// The address and port of SSDP.
pub const SSDP_ADDRESS: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(239, 255, 255, 250)), 1900);

/// The addresses FRITZ!Boxes have by default: the usual one of the home
/// network and the fallback address that is always reachable over LAN.
pub const DEFAULT_ADDRESSES: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::new(192, 168, 178, 1)),
    IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)),
];

/// How the devices were found, in the order of preference of their
/// addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// An address of the configured host.
    Config,
    /// The default gateway of this machine.
    Gateway,
    /// An answer to the SSDP search.
    Ssdp,
    /// A well-known default address.
    Default,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Config => "config",
            Self::Gateway => "gateway",
            Self::Ssdp => "ssdp",
            Self::Default => "default",
        })
    }
}

/// What a device does in the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// It has a WAN device: a FRITZ!Box.
    Router,
    /// It has none: a repeater or another device of the mesh.
    Repeater,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Router => "router",
            Self::Repeater => "repeater",
        })
    }
}

/// A verified device. The texts come from the network and are untrusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Device {
    /// The IP address, with the port if it is not the TR-064 port 49000.
    pub address: String,
    /// The host for `--host` and a profile: the IP address, which means
    /// HTTPS on port 49443, or an `http` address for another port.
    pub host: String,
    /// The manufacturer.
    pub manufacturer: Option<String>,
    /// The model, like `FRITZ!Box 7690`.
    pub model: Option<String>,
    /// The FRITZ!OS version, like `8.25`, if the description has it.
    pub firmware: Option<String>,
    /// Router or repeater.
    pub role: Role,
    /// How it was found, in the order of [`Source`].
    pub found_by: Vec<Source>,
    /// The unique device name, to merge the same device at several
    /// addresses.
    #[serde(skip)]
    udn: Option<String>,
}

/// The result of a discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// The configured host.
    pub host: String,
    /// The addresses it resolves to.
    pub addresses: Vec<IpAddr>,
    /// Whether it resolves only to addresses of the local network.
    pub local: bool,
    /// The default gateway of this machine.
    pub gateway: Option<IpAddr>,
    /// The verified devices, routers first.
    pub devices: Vec<Device>,
}

/// Resolves host names.
pub trait Resolver {
    /// The addresses of `host`, empty if it does not resolve.
    fn resolve(&self, host: &str) -> impl Future<Output = Vec<IpAddr>> + Send;
}

/// Finds the default gateway.
pub trait Gateway {
    /// The default gateway of this machine, if it has one.
    fn gateway(&self) -> impl Future<Output = Option<IpAddr>> + Send;
}

/// Sends an SSDP search and collects the answers.
pub trait Ssdp {
    /// The `LOCATION` URLs of the answers to a search for `targets` that
    /// arrive within `time`.
    fn search(&self, targets: &[&str], time: Duration) -> impl Future<Output = Vec<String>> + Send;
}

/// The name resolution and the routing table of the system.
#[derive(Debug, Clone, Copy, Default)]
pub struct System;

impl Resolver for System {
    async fn resolve(&self, host: &str) -> Vec<IpAddr> {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let found = tokio::net::lookup_host((host, 0)).await;
        let found = found.inspect_err(|error| tracing::debug!(host, %error, "cannot resolve"));
        let mut addresses: Vec<IpAddr> = found.into_iter().flatten().map(|a| a.ip()).collect();
        addresses.sort();
        addresses.dedup();
        addresses
    }
}

impl Gateway for System {
    async fn gateway(&self) -> Option<IpAddr> {
        tokio::task::spawn_blocking(default_gateway)
            .await
            .ok()
            .flatten()
    }
}

/// The default gateway from `/proc/net/route`.
#[cfg(target_os = "linux")]
fn default_gateway() -> Option<IpAddr> {
    linux_gateway(&std::fs::read_to_string("/proc/net/route").ok()?)
}

/// The default gateway from `route print`.
#[cfg(windows)]
fn default_gateway() -> Option<IpAddr> {
    let output = route(&["print", "-4", "0.0.0.0"])?;
    windows_gateway(&output)
}

/// The default gateway from `route get`, on macOS and the BSDs.
#[cfg(not(any(target_os = "linux", windows)))]
fn default_gateway() -> Option<IpAddr> {
    bsd_gateway(&route(&["-n", "get", "default"])?)
}

/// The output of the system's `route` command.
#[cfg(not(target_os = "linux"))]
fn route(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("route")
        .args(args)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The gateway of the default route in `/proc/net/route`, where addresses
/// are hex numbers in the byte order of the machine.
#[cfg(any(target_os = "linux", test))]
fn linux_gateway(table: &str) -> Option<IpAddr> {
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (destination, gateway) = (fields.get(1)?, fields.get(2)?);
        let gateway = u32::from_str_radix(gateway, 16).ok()?;
        (*destination == "00000000" && gateway != 0)
            .then(|| IpAddr::V4(Ipv4Addr::from(gateway.to_le_bytes())))
    })
}

/// The gateway in the output of `route -n get default`.
#[cfg(any(not(any(target_os = "linux", windows)), test))]
fn bsd_gateway(output: &str) -> Option<IpAddr> {
    output.lines().find_map(|line| {
        let value = line.trim().strip_prefix("gateway:")?;
        value.trim().parse().ok()
    })
}

/// The gateway of the default route in the output of `route print`.
#[cfg(any(windows, test))]
fn windows_gateway(output: &str) -> Option<IpAddr> {
    output.lines().find_map(
        |line| match line.split_whitespace().collect::<Vec<_>>()[..] {
            ["0.0.0.0", "0.0.0.0", gateway, ..] => gateway.parse().ok(),
            _ => None,
        },
    )
}

/// SSDP over UDP: a search sent to [`SSDP_ADDRESS`], or to another
/// address for tests.
#[derive(Debug, Clone, Copy)]
pub struct Multicast {
    /// Where the search goes.
    pub target: SocketAddr,
}

impl Default for Multicast {
    fn default() -> Self {
        Self {
            target: SSDP_ADDRESS,
        }
    }
}

impl Ssdp for Multicast {
    async fn search(&self, targets: &[&str], time: Duration) -> Vec<String> {
        let found = self.try_search(targets, time).await;
        found
            .inspect_err(|error| tracing::debug!(%error, "SSDP search failed"))
            .unwrap_or_default()
    }
}

impl Multicast {
    async fn try_search(&self, targets: &[&str], time: Duration) -> io::Result<Vec<String>> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
        // Only to the local network.
        socket.set_multicast_ttl_v4(1)?;
        let deadline = tokio::time::Instant::now() + time;
        for target in targets {
            let request = format!(
                "M-SEARCH * HTTP/1.1\r\nHOST: {}\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: {target}\r\n\r\n",
                self.target
            );
            socket.send_to(request.as_bytes(), self.target).await?;
        }
        let mut buffer = [0; 2048];
        let mut locations = Vec::new();
        while let Ok(answer) =
            tokio::time::timeout_at(deadline, socket.recv_from(&mut buffer)).await
        {
            let (length, _) = answer?;
            if let Some(location) = location(&buffer[..length])
                && !locations.contains(&location)
            {
                locations.push(location);
            }
        }
        Ok(locations)
    }
}

/// The `LOCATION` header of an SSDP answer.
fn location(answer: &[u8]) -> Option<String> {
    String::from_utf8_lossy(answer).lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("location")
            .then(|| value.trim().to_owned())
    })
}

/// Answers given in advance, for tests.
#[derive(Debug, Clone, Default)]
pub struct Fixed {
    /// What every host resolves to.
    pub addresses: Vec<IpAddr>,
    /// The default gateway.
    pub gateway: Option<IpAddr>,
    /// The `LOCATION` URLs of the SSDP answers.
    pub locations: Vec<String>,
}

impl Resolver for Fixed {
    fn resolve(&self, _host: &str) -> impl Future<Output = Vec<IpAddr>> + Send {
        std::future::ready(self.addresses.clone())
    }
}

impl Gateway for Fixed {
    fn gateway(&self) -> impl Future<Output = Option<IpAddr>> + Send {
        std::future::ready(self.gateway)
    }
}

impl Ssdp for Fixed {
    fn search(
        &self,
        _targets: &[&str],
        _time: Duration,
    ) -> impl Future<Output = Vec<String>> + Send {
        std::future::ready(self.locations.clone())
    }
}

/// Where and how long to look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The port of the device descriptions at the addresses of the
    /// configured host, the gateway and the defaults.
    pub port: u16,
    /// The well-known addresses to try.
    pub defaults: Vec<IpAddr>,
    /// How long to collect SSDP answers.
    pub search_time: Duration,
    /// How long to wait for a device description.
    pub timeout: Duration,
}

impl Default for Options {
    /// Port 49000, [`DEFAULT_ADDRESSES`], and two seconds each.
    fn default() -> Self {
        Self {
            port: HTTP_PORT,
            defaults: DEFAULT_ADDRESSES.to_vec(),
            search_time: Duration::from_secs(2),
            timeout: Duration::from_secs(2),
        }
    }
}

/// A discovery with its sources: `network` resolves names and finds the
/// gateway, `ssdp` searches.
#[derive(Debug, Clone, Default)]
pub struct Discovery<N, S> {
    /// The name resolution and the default gateway.
    pub network: N,
    /// The SSDP search.
    pub ssdp: S,
    /// Where and how long to look.
    pub options: Options,
}

impl Discovery<System, Multicast> {
    /// A discovery on the network of this machine.
    pub fn system() -> Self {
        Self::default()
    }
}

impl<N: Resolver + Gateway, S: Ssdp> Discovery<N, S> {
    /// Looks for devices, with the configured `host` as the first
    /// candidate if it is on the local network. Its description is asked
    /// for at `port`, else at the port of the options. The name resolution
    /// gets the timeout of the options, and gives no address after it.
    pub async fn run(&self, host: &str, port: Option<u16>) -> Report {
        let options = &self.options;
        let resolve = tokio::time::timeout(options.timeout, self.network.resolve(host));
        let (addresses, gateway, locations) = tokio::join!(
            async { resolve.await.unwrap_or_default() },
            self.network.gateway(),
            self.ssdp.search(&SEARCH_TARGETS, options.search_time),
        );
        let mut candidates: Vec<(Url, Vec<Source>)> = Vec::new();
        let at = |ip: IpAddr| probe(ip).then(|| base(SocketAddr::new(ip, options.port)));
        let port = port.unwrap_or(options.port);
        let found = (addresses
            .iter()
            .filter(|ip| probe(**ip))
            .map(|ip| (base(SocketAddr::new(*ip, port)), Source::Config)))
        .chain(gateway.and_then(at).map(|url| (url, Source::Gateway)))
        .chain(
            locations
                .iter()
                .filter_map(|l| location_base(l))
                .map(|url| (url, Source::Ssdp)),
        )
        .chain(
            options
                .defaults
                .iter()
                .filter_map(|ip| at(*ip))
                .map(|url| (url, Source::Default)),
        );
        for (url, source) in found {
            match candidates.iter_mut().find(|(known, _)| *known == url) {
                Some((_, sources)) if sources.contains(&source) => {}
                Some((_, sources)) => sources.push(source),
                None => candidates.push((url, vec![source])),
            }
        }
        let mut tasks = JoinSet::new();
        for (url, found_by) in candidates {
            let timeout = options.timeout;
            tasks.spawn(async move {
                let device = verify(url, timeout).await?;
                Some(Device { found_by, ..device })
            });
        }
        let mut devices = Vec::new();
        while let Some(result) = tasks.join_next().await {
            if let Ok(Some(device)) = result {
                merge(&mut devices, device);
            }
        }
        devices.sort_by(|a, b| {
            (a.role, &a.found_by, &a.address).cmp(&(b.role, &b.found_by, &b.address))
        });
        Report {
            host: host.to_owned(),
            local: !addresses.is_empty() && addresses.iter().all(|ip| is_local(*ip)),
            addresses,
            gateway,
            devices,
        }
    }
}

/// Whether the address belongs to the local network: private, link-local
/// or loopback.
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local() || ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local() || ip.is_loopback(),
    }
}

/// Whether to ask the address for a description: a local address, but no
/// IPv6 link-local one, which needs the interface in the URL.
fn probe(ip: IpAddr) -> bool {
    is_local(ip) && !matches!(ip, IpAddr::V6(ip) if ip.is_unicast_link_local())
}

/// The base URL of a device description.
fn base(address: SocketAddr) -> Url {
    Url::parse(&format!("http://{address}/")).expect("an IP address and a port form a URL")
}

/// The base URL of the device of an SSDP `LOCATION`, if it is an `http`
/// URL with a local IP address.
fn location_base(location: &str) -> Option<Url> {
    let url = Url::parse(location)
        .ok()
        .filter(|url| url.scheme() == "http")?;
    let host = url
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let ip: IpAddr = host.parse().ok()?;
    let port = url.port_or_known_default()?;
    probe(ip).then(|| base(SocketAddr::new(ip, port)))
}

/// Reads the description at `base`, without login, and keeps FRITZ!
/// devices.
async fn verify(base: Url, timeout: Duration) -> Option<Device> {
    let client = Client::builder(base.clone())
        .timeout(timeout)
        .build()
        .ok()?;
    let xml = client.get("/tr64desc.xml").await;
    let xml = xml
        .inspect_err(|error| tracing::debug!(%base, %error, "no description"))
        .ok()?;
    describe(&base, &xml)
}

/// The device of a description, if it is a FRITZ! device with TR-064.
fn describe(base: &Url, xml: &str) -> Option<Device> {
    let description = DeviceDescription::parse(xml).ok()?;
    let root = &description.device;
    let manufacturer = root.manufacturer.as_deref().unwrap_or_default();
    let model = root.model_name.as_deref().unwrap_or_default();
    let fritz = manufacturer.contains("AVM")
        || manufacturer.contains("FRITZ!")
        || model.starts_with("FRITZ!");
    if root.device_type != ROOT_DEVICE || !fritz {
        return None;
    }
    let router = description
        .devices()
        .any(|device| device.device_type.contains(":WANDevice:"));
    let host = base.host_str().unwrap_or_default();
    let (address, host) = if base.port() == Some(HTTP_PORT) {
        (host.to_owned(), host.to_owned())
    } else {
        let address = base
            .as_str()
            .trim_start_matches("http://")
            .trim_end_matches('/');
        (address.to_owned(), format!("http://{address}"))
    };
    Some(Device {
        address,
        host,
        manufacturer: root.manufacturer.clone(),
        model: root.model_name.clone(),
        firmware: firmware(xml),
        role: if router { Role::Router } else { Role::Repeater },
        found_by: Vec::new(),
        udn: root.udn.clone(),
    })
}

/// The FRITZ!OS version of `<systemVersion><Display>`, like `8.25` of
/// `285.08.25`, without the hardware number in front.
fn firmware(xml: &str) -> Option<String> {
    let document = roxmltree::Document::parse(xml).ok()?;
    let display = child(child(document.root_element(), "systemVersion")?, "Display")?.text()?;
    let (_, version) = display.trim().split_once('.')?;
    Some(version.strip_prefix('0').unwrap_or(version).to_owned())
}

/// The first child element of `node` named `name`.
fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children().find(|child| child.has_tag_name(name))
}

/// Adds the device, or merges it into the same device found at another
/// address, which keeps the address that was found the preferred way.
fn merge(devices: &mut Vec<Device>, device: Device) {
    let same = |known: &&mut Device| match (&known.udn, &device.udn) {
        (Some(a), Some(b)) => a == b && known.model == device.model,
        _ => known.address == device.address,
    };
    let Some(known) = devices.iter_mut().find(same) else {
        devices.push(device);
        return;
    };
    if device.found_by < known.found_by {
        known.address.clone_from(&device.address);
        known.host.clone_from(&device.host);
    }
    known.found_by.extend(device.found_by);
    known.found_by.sort();
    known.found_by.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_default_gateway_in_the_routing_tables() {
        let linux = "Iface\tDestination\tGateway \tFlags\n\
                     eth0\t0000A8C0\t00000000\t0001\n\
                     eth0\t00000000\t01B2A8C0\t0003\n";
        assert_eq!(linux_gateway(linux), Some("192.168.178.1".parse().unwrap()));
        assert_eq!(linux_gateway("Iface\tDestination\tGateway\n"), None);
        assert_eq!(linux_gateway("h\neth0\t00000000\t00000000\n"), None);

        let bsd = "   route to: default\ndestination: default\n    gateway: 192.168.178.1\n";
        assert_eq!(bsd_gateway(bsd), Some("192.168.178.1".parse().unwrap()));
        assert_eq!(
            bsd_gateway("route: writing to routing socket: not in table"),
            None
        );

        let windows = "Active Routes:\nNetwork Destination        Netmask          Gateway       Interface  Metric\n          0.0.0.0          0.0.0.0    192.168.178.1  192.168.178.20     25\n";
        assert_eq!(
            windows_gateway(windows),
            Some("192.168.178.1".parse().unwrap())
        );
        assert_eq!(windows_gateway("none"), None);
    }

    #[test]
    fn reads_the_location_of_ssdp_answers() {
        let answer = b"HTTP/1.1 200 OK\r\nST: x\r\nLocation: http://192.168.178.1:49000/tr64desc.xml\r\n\r\n";
        assert_eq!(
            location(answer).as_deref(),
            Some("http://192.168.178.1:49000/tr64desc.xml")
        );
        assert_eq!(location(b"HTTP/1.1 200 OK\r\n\r\n"), None);
    }

    #[test]
    fn takes_only_local_addresses() {
        for local in [
            "192.168.1.1",
            "10.0.0.1",
            "169.254.1.1",
            "127.0.0.1",
            "fd00::1",
            "::1",
            "fe80::1",
        ] {
            assert!(is_local(local.parse().unwrap()), "{local}");
        }
        for public in ["203.0.113.1", "2001:db8::1", "100.64.0.1"] {
            assert!(!is_local(public.parse().unwrap()), "{public}");
        }
        assert!(!probe("fe80::1".parse().unwrap()));
        assert_eq!(
            location_base("http://192.168.178.30:49000/tr64desc.xml").map(String::from),
            Some("http://192.168.178.30:49000/".to_owned())
        );
        assert_eq!(
            location_base("http://[fd00::1]/igd.xml").map(String::from),
            Some("http://[fd00::1]/".to_owned())
        );
        for rejected in [
            "https://192.168.178.1:49443/tr64desc.xml",
            "http://203.0.113.1:49000/tr64desc.xml",
            "http://fritz.box:49000/tr64desc.xml",
            "not a URL",
        ] {
            assert_eq!(location_base(rejected), None, "{rejected}");
        }
    }

    #[test]
    fn reads_the_firmware_without_the_hardware_number() {
        let xml = "<root><systemVersion><Display>285.08.25</Display></systemVersion></root>";
        assert_eq!(firmware(xml).as_deref(), Some("8.25"));
        let xml = "<root><systemVersion><Display>285.10.01</Display></systemVersion></root>";
        assert_eq!(firmware(xml).as_deref(), Some("10.01"));
        assert_eq!(firmware("<root/>"), None);
        assert_eq!(firmware("<root>"), None);
    }

    #[test]
    fn names_sources_and_roles() {
        let names: Vec<String> = [
            Source::Config,
            Source::Gateway,
            Source::Ssdp,
            Source::Default,
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(names, ["config", "gateway", "ssdp", "default"]);
        assert_eq!(Role::Router.to_string(), "router");
        assert_eq!(Role::Repeater.to_string(), "repeater");
    }

    const DESCRIPTION: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fritz-sim/fixtures/fritzbox-7690/8.25/tr64desc.xml"
    ));

    fn found(base: &str, source: Source, udn: Option<&str>) -> Device {
        let mut device = describe(&base.parse().unwrap(), DESCRIPTION).unwrap();
        device.found_by = vec![source];
        device.udn = udn.map(str::to_owned);
        device
    }

    #[test]
    fn merges_the_same_device_found_twice() {
        let ssdp = found("http://192.168.178.1:49000/", Source::Ssdp, Some("uuid:1"));
        assert_eq!(
            (ssdp.address.as_str(), ssdp.host.as_str()),
            ("192.168.178.1", "192.168.178.1")
        );
        let gateway = found("http://10.0.0.1:49000/", Source::Gateway, Some("uuid:1"));
        let other = found("http://10.0.0.2:49000/", Source::Default, Some("uuid:2"));
        let unnamed = found("http://10.0.0.3:49000/", Source::Default, None);
        let again = found("http://10.0.0.3:49000/", Source::Ssdp, None);
        let mut devices = Vec::new();
        for device in [ssdp, gateway, other, unnamed, again] {
            merge(&mut devices, device);
        }
        let merged: Vec<(&str, &[Source])> = devices
            .iter()
            .map(|device| (device.address.as_str(), device.found_by.as_slice()))
            .collect();
        assert_eq!(
            merged,
            [
                ("10.0.0.1", &[Source::Gateway, Source::Ssdp][..]),
                ("10.0.0.2", &[Source::Default][..]),
                ("10.0.0.3", &[Source::Ssdp, Source::Default][..]),
            ]
        );
        assert_eq!(devices[0].host, "10.0.0.1");
    }

    #[test]
    fn looks_on_the_local_network_by_default() {
        let discovery = Discovery::system();
        assert_eq!(discovery.ssdp.target, SSDP_ADDRESS);
        assert_eq!(discovery.options.port, 49000);
        assert_eq!(discovery.options.defaults, DEFAULT_ADDRESSES);
    }
}
