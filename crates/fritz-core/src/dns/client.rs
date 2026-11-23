//! The resolver configuration of this machine: on macOS from `scutil --dns`,
//! elsewhere from `/etc/resolv.conf`. Both are read with a size and a time
//! limit, and only read.

use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncReadExt as _;

/// The most bytes of the configuration read.
const MAX_CONFIG: u64 = 256 * 1024;

/// How long reading the configuration may take.
pub(crate) const CONFIG_TIMEOUT: Duration = Duration::from_secs(3);

/// How this machine resolves names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ClientConfig {
    /// Where it was read from, like `scutil --dns`.
    pub source: String,
    /// The resolvers, the default ones first. Resolvers for reverse
    /// lookups are left out.
    pub resolvers: Vec<Resolver>,
}

/// A resolver of this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Resolver {
    /// The domain it answers for; `None` for the default resolver.
    pub domain: Option<String>,
    /// The DNS servers it asks.
    pub nameservers: Vec<String>,
    /// The domains added to names without a dot.
    pub search: Vec<String>,
    /// Like `mdns`.
    pub options: Option<String>,
}

impl ClientConfig {
    /// The resolver for `name`: the one for the longest domain that `name`
    /// ends in, else the first default one.
    pub fn resolver_for(&self, name: &str) -> Option<&Resolver> {
        let name = name.trim_end_matches('.').to_ascii_lowercase();
        self.resolvers
            .iter()
            .filter(|r| {
                r.domain.as_deref().is_some_and(|domain| {
                    let domain = domain.to_ascii_lowercase();
                    name == domain || name.ends_with(&format!(".{domain}"))
                })
            })
            .max_by_key(|r| r.domain.as_ref().map_or(0, String::len))
            .or_else(|| self.resolvers.iter().find(|r| r.domain.is_none()))
    }

    /// The search domains of the default resolvers.
    pub fn search(&self) -> Vec<&str> {
        let mut search: Vec<&str> = Vec::new();
        for domain in self
            .resolvers
            .iter()
            .filter(|r| r.domain.is_none())
            .flat_map(|r| &r.search)
        {
            if !search.contains(&domain.as_str()) {
                search.push(domain);
            }
        }
        search
    }
}

/// Parses the output of `scutil --dns`, only its first part: the part for
/// scoped queries repeats the resolvers per interface.
pub fn parse_scutil(text: &str) -> ClientConfig {
    let mut resolvers = Vec::new();
    let mut current: Option<Resolver> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("DNS configuration (") {
            break;
        }
        if line.starts_with("resolver #") {
            resolvers.extend(current.replace(Resolver::default()));
            continue;
        }
        let (Some(resolver), Some((key, value))) = (current.as_mut(), line.split_once(':')) else {
            continue;
        };
        let value = value.trim().to_owned();
        match key.trim() {
            "domain" => resolver.domain = Some(value),
            "options" => resolver.options = Some(value),
            key if key.starts_with("nameserver[") => resolver.nameservers.push(value),
            key if key.starts_with("search domain[") => resolver.search.push(value),
            _ => {}
        }
    }
    resolvers.extend(current);
    ClientConfig {
        source: "scutil --dns".to_owned(),
        resolvers: forward(resolvers),
    }
}

/// Parses a `resolv.conf`: its name servers and search domains, as one
/// default resolver.
pub fn parse_resolv_conf(text: &str) -> ClientConfig {
    let mut resolver = Resolver::default();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("nameserver") => resolver.nameservers.extend(words.next().map(str::to_owned)),
            // The last of `search` and `domain` wins.
            Some("search" | "domain") => {
                resolver.search = words
                    .take_while(|w| !w.starts_with(['#', ';']))
                    .map(str::to_owned)
                    .collect();
            }
            _ => {}
        }
    }
    ClientConfig {
        source: "/etc/resolv.conf".to_owned(),
        resolvers: vec![resolver],
    }
}

/// The resolvers for forward lookups: without those for `.arpa`.
fn forward(resolvers: Vec<Resolver>) -> Vec<Resolver> {
    resolvers
        .into_iter()
        .filter(|r| {
            !r.domain
                .as_deref()
                .is_some_and(|d| d.trim_end_matches('.').ends_with("arpa"))
        })
        .collect()
}

/// Whether `server` is an address on this machine, like a local resolver
/// such as `127.0.0.53`, whose upstream servers are unknown.
pub(crate) fn is_local(server: &str) -> bool {
    server
        .split('%')
        .next()
        .and_then(|s| s.parse::<IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback())
}

/// Where the configuration of a system is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The output of `scutil --dns`, on macOS.
    Scutil,
    /// `/etc/resolv.conf`, on other Unix systems.
    ResolvConf,
}

/// Where the configuration of the system `os` is, like
/// [`std::env::consts::OS`]. `None` where it is unknown.
fn source(os: &str) -> Option<Source> {
    match os {
        "macos" => Some(Source::Scutil),
        "windows" => None,
        _ => Some(Source::ResolvConf),
    }
}

/// Reads the configuration of this machine, `None` where it is unknown.
pub(crate) async fn read() -> Option<ClientConfig> {
    read_from(source(std::env::consts::OS)?).await
}

/// Reads the configuration from `source`.
async fn read_from(source: Source) -> Option<ClientConfig> {
    match source {
        Source::Scutil => run("scutil", &["--dns"], CONFIG_TIMEOUT)
            .await
            .map(|text| parse_scutil(&text)),
        Source::ResolvConf => read_file(Path::new("/etc/resolv.conf"), CONFIG_TIMEOUT)
            .await
            .map(|text| parse_resolv_conf(&text)),
    }
}

/// The start of the output of a command, `None` if it fails or takes longer
/// than `timeout`. The command is killed then.
pub(crate) async fn run(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let output = async {
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let mut text = String::new();
        child
            .stdout
            .take()?
            .take(MAX_CONFIG)
            .read_to_string(&mut text)
            .await
            .ok()?;
        child.wait().await.ok()?.success().then_some(text)
    };
    tokio::time::timeout(timeout, output).await.ok().flatten()
}

/// The start of a file, `None` if it cannot be read in `timeout`.
pub(crate) async fn read_file(path: &Path, timeout: Duration) -> Option<String> {
    let read = async {
        let mut text = String::new();
        tokio::fs::File::open(path)
            .await
            .ok()?
            .take(MAX_CONFIG)
            .read_to_string(&mut text)
            .await
            .ok()?;
        Some(text)
    };
    tokio::time::timeout(timeout, read).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCUTIL: &str = "\
DNS configuration

resolver #1
  search domain[0] : fritz.box
  search domain[1] : example.org
  nameserver[0] : 192.168.178.1
  nameserver[1] : fd00::1
  if_index : 14 (en0)
  flags    : Request A records, Request AAAA records

resolver #2
  domain   : local
  options  : mdns
  timeout  : 5

resolver #3
  domain   : 254.169.in-addr.arpa
  options  : mdns

resolver #4
  domain   : corp.example
  nameserver[0] : 10.8.0.1

DNS configuration (for scoped queries)

resolver #1
  search domain[0] : other.example
  nameserver[0] : 192.168.178.1
";

    const RESOLV_CONF: &str = "\
# Generated
nameserver 127.0.0.53
nameserver
options edns0 trust-ad
domain old.example
search fritz.box lan # comment
";

    #[test]
    fn parses_scutil() {
        let config = parse_scutil(SCUTIL);
        assert_eq!(config.source, "scutil --dns");
        assert_eq!(
            config.resolvers,
            [
                Resolver {
                    domain: None,
                    nameservers: vec!["192.168.178.1".to_owned(), "fd00::1".to_owned()],
                    search: vec!["fritz.box".to_owned(), "example.org".to_owned()],
                    options: None,
                },
                Resolver {
                    domain: Some("local".to_owned()),
                    options: Some("mdns".to_owned()),
                    ..Resolver::default()
                },
                Resolver {
                    domain: Some("corp.example".to_owned()),
                    nameservers: vec!["10.8.0.1".to_owned()],
                    ..Resolver::default()
                },
            ]
        );
        assert_eq!(config.search(), ["fritz.box", "example.org"]);
        let twice = ClientConfig {
            source: String::new(),
            resolvers: vec![config.resolvers[0].clone(), config.resolvers[0].clone()],
        };
        assert_eq!(twice.search(), ["fritz.box", "example.org"]);
        assert_eq!(parse_scutil("").resolvers, []);
    }

    #[test]
    fn parses_resolv_conf() {
        let config = parse_resolv_conf(RESOLV_CONF);
        assert_eq!(config.source, "/etc/resolv.conf");
        assert_eq!(
            config.resolvers,
            [Resolver {
                nameservers: vec!["127.0.0.53".to_owned()],
                search: vec!["fritz.box".to_owned(), "lan".to_owned()],
                ..Resolver::default()
            }]
        );
    }

    #[test]
    fn finds_the_resolver_for_a_name() {
        let config = parse_scutil(SCUTIL);
        let domain = |name| config.resolver_for(name).and_then(|r| r.domain.clone());
        assert_eq!(domain("printer.local"), Some("local".to_owned()));
        assert_eq!(domain("A.Corp.Example."), Some("corp.example".to_owned()));
        assert_eq!(domain("corp.example"), Some("corp.example".to_owned()));
        assert_eq!(domain("nas.fritz.box"), None);
        assert!(config.resolver_for("nas").is_some());
        assert!(ClientConfig::default().resolver_for("nas").is_none());
    }

    #[test]
    fn knows_local_servers() {
        assert!(is_local("127.0.0.53"));
        assert!(is_local("::1"));
        assert!(!is_local("192.168.178.1"));
        assert!(!is_local("fe80::1%en0"));
        assert!(!is_local("name"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_commands_with_limits() {
        assert_eq!(
            run("echo", &["resolver"], CONFIG_TIMEOUT).await.as_deref(),
            Some("resolver\n")
        );
        assert_eq!(run("false", &[], CONFIG_TIMEOUT).await, None);
        assert_eq!(
            run("fritz-no-such-command", &[], CONFIG_TIMEOUT).await,
            None
        );
        // Not text.
        assert_eq!(run("printf", &["\\377"], CONFIG_TIMEOUT).await, None);
        let slow = run("sleep", &["5"], Duration::from_millis(50));
        assert_eq!(slow.await, None);
    }

    #[tokio::test]
    async fn reads_files_with_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resolv.conf");
        std::fs::write(&path, "x".repeat(300 * 1024)).unwrap();
        let text = read_file(&path, CONFIG_TIMEOUT).await.unwrap();
        assert_eq!(text.len(), 256 * 1024);
        assert_eq!(
            read_file(&dir.path().join("missing"), CONFIG_TIMEOUT).await,
            None
        );
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert_eq!(read_file(&path, CONFIG_TIMEOUT).await, None);
    }

    #[tokio::test]
    async fn reads_the_configuration_of_this_machine() {
        assert_eq!(source("macos"), Some(Source::Scutil));
        assert_eq!(source("linux"), Some(Source::ResolvConf));
        assert_eq!(source("windows"), None);
        // Only reads; what it finds depends on the machine.
        for source in [Source::Scutil, Source::ResolvConf] {
            if let Some(config) = read_from(source).await {
                assert_ne!(config.source, "");
            }
        }
        let _ = read().await;
    }
}
