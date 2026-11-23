//! `fritz doctor`: checks the whole setup, from the configuration to the
//! login and the rights, and says how to fix what fails.
//!
//! Runs every check it can, also after a failure, and tries each login at
//! most once, so that it never makes the device block logins longer. Never
//! shows the password, a session ID or a key.

use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::bail;
use fritz_core::capabilities::{self, Capabilities};
use fritz_core::config::Config;
use fritz_core::connection::Target;
use fritz_core::doctor::{self, Check};
use fritz_core::generic;
use fritz_core::session::{self, Session};
use fritz_core::smarthome::{self, Device};

use crate::context::Context;
use crate::output::{Format, show};
use crate::status::{clean, reader};
use crate::tls;

const CONFIGURATION: &str = "Configuration";
const HOST: &str = "Host";
const REACHABLE: &str = "Reachable";
const TLS: &str = "TLS";
const PASSWORD: &str = "Password";
const LOGIN: &str = "Login";
const RIGHTS: &str = "Rights";
const TWO_FACTOR: &str = "Two-factor";
const WEB: &str = "Web interface";
const SMART_HOME: &str = "Smart Home";
#[cfg(feature = "write")]
const CHANGES: &str = "Changes";

/// The checks in their order.
const CHECKS: &[&str] = &[
    CONFIGURATION,
    HOST,
    REACHABLE,
    TLS,
    PASSWORD,
    LOGIN,
    RIGHTS,
    TWO_FACTOR,
    WEB,
    SMART_HOME,
    #[cfg(feature = "write")]
    CHANGES,
];

/// The hint while the device blocks logins.
const WAIT: &str = "wait until then; every failed login makes the device wait longer";

/// How long to wait for the TR-064 port.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs the checks and prints them. Fails if a check fails.
pub(crate) async fn run(
    context: anyhow::Result<Context>,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let checks = report(context.as_ref()).await;
    show(checks.as_slice(), format, out, text)?;
    let failed: Vec<_> = checks
        .iter()
        .filter(|c| c.failed())
        .map(|c| c.check)
        .collect();
    if !failed.is_empty() {
        bail!("checks failed: {}", failed.join(", "));
    }
    Ok(())
}

/// The checks of the setup in `context`, or of the configuration that
/// could not be loaded.
pub(crate) async fn report(context: Result<&Context, &anyhow::Error>) -> Vec<Check> {
    match context {
        Ok(context) => checks(context).await,
        Err(error) => rest(vec![Check::fail(
            CONFIGURATION,
            format!("{error:#}"),
            "fix the configuration file or the environment, or run `fritz config init`",
        )]),
    }
}

/// One line per check: the status, the name, what was found and the hint.
fn text(checks: &[Check], out: &mut impl Write) -> anyhow::Result<()> {
    let width = CHECKS.iter().map(|c| c.len()).max().unwrap_or_default();
    for check in checks {
        let status = check.status.name();
        let detail = clean(&check.detail);
        writeln!(out, "{status:<4}  {:<width$}  {detail}", check.check)?;
        if let Some(hint) = &check.hint {
            writeln!(out, "      {:<width$}  hint: {}", "", clean(hint))?;
        }
    }
    Ok(())
}

/// Skips the checks after `checks`, naming the first that failed.
fn rest(mut checks: Vec<Check>) -> Vec<Check> {
    let failed = checks
        .iter()
        .find(|c| c.failed())
        .map_or("an earlier check", |c| c.check);
    let detail = format!("skipped: {failed} failed");
    for name in &CHECKS[checks.len().min(CHECKS.len())..] {
        checks.push(Check::skip(name, detail.clone()));
    }
    checks
}

/// Runs the checks in order, and skips those that need one that failed.
async fn checks(context: &Context) -> Vec<Check> {
    let target = match context.target() {
        Ok(target) => target,
        Err(error) => {
            let hint = "check --profile, --host and --fingerprint, and the profiles in \
                        `fritz config list`";
            return rest(vec![Check::fail(CONFIGURATION, format!("{error:#}"), hint)]);
        }
    };
    let mut checks = vec![configuration(context)];
    let host = target.url.host_str().unwrap_or_default().to_owned();
    let addresses = resolve(&target).await;
    checks.push(doctor::host(HOST, &host, &addresses));
    let reachable = reachable(&target, &addresses).await;
    let tls = if reachable.failed() {
        Check::skip(TLS, "skipped: Reachable failed")
    } else {
        tls(&target).await
    };
    checks.extend([reachable, tls, password(context, &target).await]);
    if checks.iter().any(Check::failed) {
        return rest(checks);
    }
    let capabilities = match login(context, &target).await {
        Ok(capabilities) => capabilities,
        Err(check) => {
            checks.push(check);
            return rest(checks);
        }
    };
    checks.push(Check::ok(LOGIN, logged_in(&target)));
    checks.push(doctor::rights(RIGHTS, &capabilities.rights));
    let changes = cfg!(feature = "write");
    checks.push(doctor::two_factor(
        TWO_FACTOR,
        capabilities.two_factor,
        changes,
    ));
    checks.extend(web(context).await);
    #[cfg(feature = "write")]
    checks.push(self::changes(context).await);
    checks
}

/// The check of the configuration, after it loaded and names a device.
fn configuration(context: &Context) -> Check {
    let path = context.path();
    let shown = path.display();
    let Some(profile) = context.profile() else {
        if context.overrides().host.is_some() {
            return Check::ok(
                CONFIGURATION,
                format!("no profile in {shown}, --host is set"),
            );
        }
        let detail = format!("no profile in {shown}, so fritz looks for fritz.box");
        return Check::warn(CONFIGURATION, detail, "run `fritz config init`");
    };
    let name = context.profile_name();
    if profile.password_command.is_some() && Config::writable_by_others(path) {
        let detail = format!("others may change {shown}, which has the password command");
        return Check::warn(CONFIGURATION, detail, format!("run `chmod go-w {shown}`"));
    }
    Check::ok(CONFIGURATION, format!("profile {name} in {shown}"))
}

/// The addresses of the host of `target`, without repeats.
async fn resolve(target: &Target) -> Vec<IpAddr> {
    let host = target.url.host_str().unwrap_or_default();
    let port = target.url.port_or_known_default().unwrap_or_default();
    let mut addresses: Vec<IpAddr> = tokio::net::lookup_host(format!("{host}:{port}"))
        .await
        .map_or_default(|found| found.map(|a| a.ip()).collect());
    addresses.sort_unstable();
    addresses.dedup();
    addresses
}

/// Whether the TR-064 port of `target` takes connections at one of the
/// `addresses`.
async fn reachable(target: &Target, addresses: &[IpAddr]) -> Check {
    if addresses.is_empty() {
        return Check::skip(REACHABLE, "skipped: Host failed");
    }
    let port = target.url.port_or_known_default().unwrap_or_default();
    let sockets: Vec<SocketAddr> = addresses.iter().map(|ip| (*ip, port).into()).collect();
    let connect = tokio::net::TcpStream::connect(sockets.as_slice());
    match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Ok(Ok(_)) => Check::ok(REACHABLE, format!("TR-064 answers on port {port}")),
        Ok(Err(_)) | Err(_) => Check::fail(
            REACHABLE,
            format!("nothing answers on port {port}"),
            "turn on \"Allow access for applications\" in Home Network > Network > Network \
             Settings; TR-064 uses port 49443 for https and 49000 for http",
        ),
    }
}

/// Whether the device presents the trusted certificate. Never shows a
/// fingerprint.
async fn tls(target: &Target) -> Check {
    if target.url.scheme() != "https" {
        return Check::warn(
            TLS,
            "http: the password and all values cross the network unencrypted",
            "use https, like --host fritz.box, and run `fritz tls trust`",
        );
    }
    let Some(trusted) = target.fingerprint else {
        let detail = "no certificate is trusted for this device";
        return Check::fail(TLS, detail, "run `fritz tls trust`");
    };
    match tls::fetch(&target.url).await {
        Ok(found) if found == trusted => {
            Check::ok(TLS, "the device presents the trusted certificate")
        }
        Ok(_) => Check::fail(
            TLS,
            "the device presents another certificate than the trusted one",
            "if the device got a new certificate, check it and run `fritz tls trust`; \
             else another device answers",
        ),
        Err(error) => Check::fail(TLS, format!("{error:#}"), "check the host and the port"),
    }
}

/// Where the password comes from.
async fn password(context: &Context, target: &Target) -> Check {
    match context.password(target).await {
        Ok((_, source)) => Check::ok(PASSWORD, format!("from {source}")),
        Err(error) => Check::fail(
            PASSWORD,
            format!("{error:#}"),
            "run `fritz auth login`, or set FRITZ_PASSWORD or a password command",
        ),
    }
}

/// Logs in once over TR-064 and reads the capabilities, unless the device
/// blocks logins now, since a login then only makes it block longer.
async fn login(context: &Context, target: &Target) -> Result<Capabilities, Check> {
    let block = block_time(target).await.unwrap_or_default();
    if block > 0 {
        let detail = format!("the device blocks logins for {block} more seconds");
        return Err(Check::fail(LOGIN, detail, WAIT));
    }
    let result = async {
        let mut reader = reader(context).await?;
        // Needs the login on every device, also where the capabilities do
        // not. A missing right still shows that the login worked.
        reader.try_read("DeviceInfo1", "GetInfo", &[]).await?;
        Ok(capabilities::capabilities(&mut reader).await?)
    };
    result.await.map_err(|error: anyhow::Error| {
        if rejected(&error) {
            Check::fail(
                LOGIN,
                "the device rejected the user or the password",
                "check the user with --user and the password with `fritz auth status`; \
                 not retried, so that the device does not block logins",
            )
        } else {
            Check::fail(LOGIN, format!("{error:#}"), "run with -v for details")
        }
    })
}

/// The seconds until the web interface of `target` accepts a login, read
/// without logging in.
async fn block_time(target: &Target) -> anyhow::Result<u64> {
    let client = target.web().client()?.build()?;
    Ok(session::block_time(&client).await?)
}

/// Whether the device rejected the credentials.
fn rejected(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<generic::Error>(),
            Some(generic::Error::Tr064(tr064::Error::AuthenticationFailed))
        ) || matches!(
            cause.downcast_ref::<tr064::Error>(),
            Some(tr064::Error::AuthenticationFailed)
        )
    })
}

/// Who logged in.
fn logged_in(target: &Target) -> String {
    match &target.username {
        Some(user) => format!("logged in as {user}"),
        None => "logged in with the password only".to_owned(),
    }
}

/// The checks of the web interface and the Smart Home API, which it serves.
async fn web(context: &Context) -> [Check; 2] {
    match context.session().await {
        Ok(session) => {
            let smart_home = smart_home(&session).await;
            // The check is done; a failed logout only lets the session expire.
            let _ = session.logout().await;
            [Check::ok(WEB, "logged in to the web interface"), smart_home]
        }
        Err(error) => [
            web_failure(&error),
            Check::skip(SMART_HOME, "skipped: Web interface failed"),
        ],
    }
}

/// Why the login to the web interface failed.
fn web_failure(error: &anyhow::Error) -> Check {
    match error.downcast_ref::<session::Error>() {
        Some(session::Error::Blocked(seconds)) => Check::fail(
            WEB,
            format!("the device blocks logins for {seconds} more seconds"),
            WAIT,
        ),
        Some(session::Error::Rejected(_)) => Check::fail(
            WEB,
            "the web interface rejected the user or the password",
            "the web interface needs the same user and password as TR-064",
        ),
        _ => Check::fail(WEB, format!("{error:#}"), "run with -v for details"),
    }
}

/// Whether the Smart Home API answers. Its absence is no fault: older
/// versions of FRITZ!OS do not have it.
async fn smart_home(session: &Session) -> Check {
    smart_home_check(smarthome::devices(session).await)
}

/// The check of what the Smart Home API answered.
fn smart_home_check(devices: Result<Vec<Device>, session::Error>) -> Check {
    match devices {
        Ok(devices) => Check::ok(SMART_HOME, format!("{} devices", devices.len())),
        Err(session::Error::Tr064(tr064::Error::Status { status: 404, .. })) => Check::skip(
            SMART_HOME,
            "the device has no Smart Home API (FRITZ!OS 8.20 and later)",
        ),
        Err(session::Error::Tr064(tr064::Error::Status { status: 403, .. })) => Check::warn(
            SMART_HOME,
            "the user may not read the Smart Home devices",
            "give the user the right for Smart Home in System > FRITZ!Box Users",
        ),
        Err(error) => Check::fail(SMART_HOME, error.to_string(), "run with -v for details"),
    }
}

/// Whether changes to the device are allowed: the environment, and the
/// allow and deny lists of the configuration.
#[cfg(feature = "write")]
async fn changes(context: &Context) -> Check {
    use fritz_core::change::{self, ENV_ALLOW_WRITE, Permit};

    if !Permit::from_env().write {
        return Check::skip(CHANGES, format!("{ENV_ALLOW_WRITE} is not set"));
    }
    let verdict = async {
        let (identity, _) = change::identity(&context.client().await?).await?;
        Ok(context.write_targets().verdict(&identity))
    };
    changes_check(verdict.await)
}

/// The check of whether the device may be changed.
#[cfg(feature = "write")]
fn changes_check(verdict: anyhow::Result<fritz_core::identity::Verdict>) -> Check {
    use fritz_core::identity::Verdict;

    let hint = "run `fritz change identity` and add the identity to `allow` in [write] of the \
                configuration";
    match verdict {
        Ok(Verdict::Allowed) => Check::ok(CHANGES, "the device is on the allow list"),
        Ok(Verdict::Denied) => Check::fail(
            CHANGES,
            "the device is on the deny list",
            "remove it from `deny` in [write] of the configuration to change it",
        ),
        Ok(Verdict::Unknown) => Check::fail(CHANGES, "the device is not on the allow list", hint),
        Err(error) => Check::fail(CHANGES, format!("{error:#}"), "run with -v for details"),
    }
}

#[cfg(test)]
mod tests {
    use fritz_core::connection::Overrides;
    use fritz_core::doctor::Status;

    use super::*;
    use crate::context::tests::{agent, context};

    #[test]
    fn checks_the_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let check = configuration(&context(dir.path(), Overrides::default()));
        assert_eq!(check.status, Status::Warn);
        assert!(check.detail.contains("fritz.box"), "{}", check.detail);
        let check = configuration(&context(dir.path(), agent()));
        assert_eq!(check.status, Status::Ok);
        assert!(check.detail.ends_with("--host is set"), "{}", check.detail);

        let path = dir.path().join("config.toml");
        let profile = "[profile.default]\nhost = \"192.168.178.1\"\n";
        std::fs::write(&path, profile).unwrap();
        let check = configuration(&context(dir.path(), Overrides::default()));
        assert!(
            check.detail.starts_with("profile default in"),
            "{}",
            check.detail
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let text = format!("{profile}password_command = [\"true\"]\n");
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
            let check = configuration(&context(dir.path(), Overrides::default()));
            assert_eq!(check.status, Status::Warn);
            assert!(check.hint.unwrap().starts_with("run `chmod go-w"));
        }
    }

    #[tokio::test]
    async fn skips_an_unresolved_host() {
        let dir = tempfile::tempdir().unwrap();
        let target = context(dir.path(), agent()).target().unwrap();
        assert_eq!(reachable(&target, &[]).await.status, Status::Skip);
        assert_eq!(logged_in(&target), "logged in as agent");
        let target = Target {
            username: None,
            ..target
        };
        assert_eq!(logged_in(&target), "logged in with the password only");
    }

    #[test]
    fn explains_failed_web_logins() {
        let check = web_failure(&session::Error::Blocked(30).into());
        assert_eq!(check.detail, "the device blocks logins for 30 more seconds");
        let check = web_failure(&session::Error::Rejected(8).into());
        assert!(check.detail.contains("rejected"), "{}", check.detail);
        let check = web_failure(&anyhow::anyhow!("no answer"));
        assert_eq!(
            (check.status, check.detail.as_str()),
            (Status::Fail, "no answer")
        );
    }

    #[test]
    fn explains_the_smart_home_api() {
        let status = |status| {
            let url = "https://192.168.178.1/api".to_owned();
            Err(session::Error::Tr064(tr064::Error::Status { status, url }))
        };
        assert_eq!(smart_home_check(Ok(Vec::new())).detail, "0 devices");
        assert_eq!(smart_home_check(status(404)).status, Status::Skip);
        assert_eq!(smart_home_check(status(403)).status, Status::Warn);
        assert_eq!(smart_home_check(status(500)).status, Status::Fail);
    }

    #[cfg(feature = "write")]
    #[test]
    fn checks_whether_changes_are_allowed() {
        use fritz_core::identity::Verdict;

        assert_eq!(changes_check(Ok(Verdict::Allowed)).status, Status::Ok);
        let check = changes_check(Ok(Verdict::Denied));
        assert_eq!(check.detail, "the device is on the deny list");
        let check = changes_check(Ok(Verdict::Unknown));
        assert!(check.hint.unwrap().contains("fritz change identity"));
        let check = changes_check(Err(anyhow::anyhow!("no serial number")));
        assert_eq!(
            (check.status, check.detail.as_str()),
            (Status::Fail, "no serial number")
        );
    }
}
