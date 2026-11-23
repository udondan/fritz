//! Stateful fake TR-064 server, built from recorded fixtures. Used by tests
//! only: every write test runs against it, never against a real device.
//!
//! [`Sim::start`] serves a [`Fixture`] on the loopback interface: the device
//! description and the service descriptions without authentication, and
//! the action calls with HTTP Digest authentication.
//!
//! - Calls are checked against the service description: an unknown action
//!   fails with UPnP error 401, missing or unknown arguments with 402, and
//!   values that don't fit the data type or the allowed values with 600.
//!   Like the devices, it doesn't check the allowed range.
//! - Getters answer as recorded. Getters with an index answer the recorded
//!   entry, or 713 if there is none.
//! - Every argument refers to a state variable. Calls of other actions, like
//!   `SetEnable`, write their input arguments into these variables, and the
//!   getters that read the same variables answer with the new values.
//!   Nothing else changes: the simulator does not know which settings a
//!   device changes along with them, like the options of a feature it
//!   turns off. Tests inject such side effects themselves.
//! - Paths to list documents, like the one `X_AVM-DE_GetHostListPath`
//!   returns, hold a session ID, and the documents are served for it.
//! - The web interface has a session login, `/login_sid.lua`, with a
//!   PBKDF2 challenge, and serves the documents of the REST API, like
//!   `/api/v0/smarthome/overview`, the pages of `/data.lua`, like
//!   `ecoStat`, and the statistics of Smart Home devices of
//!   `/webservices/homeautoswitch.lua`, for the open session. Like the
//!   device, the page `log` remembers the category of its last field
//!   `filter` and lists only its entries, see [`Sim::log_filter`].
//! - Tests can [inject](Sim::inject) UPnP errors, delays, discarded writes,
//!   side effects of writes, counters that grow, a locked login, calls that
//!   break off, restarts and pages that fail, and read the [requests](Sim::requests) the
//!   server got.
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use fritz_sim::{Fixture, Injection, Sim};
//!
//! let sim = Sim::start(Fixture::bundled("fritzbox-7690/8.25")?).await?;
//! let client = sim.client().build()?;
//! let description = client.description().await?;
//! let wlan = description.service("WLANConfiguration1")?;
//! client.call(wlan, "SetEnable", &[("NewEnable", "0")]).await?;
//! assert_eq!(sim.state("WLANConfiguration1", "Enable").as_deref(), Some("0"));
//!
//! sim.inject(Injection::fault("WLANConfiguration1", "SetEnable", 866));
//! assert!(client.call(wlan, "SetEnable", &[("NewEnable", "1")]).await.is_err());
//! # Ok(())
//! # }
//! ```

use std::future::IntoFuture as _;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes, HttpBody};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tokio::task::JoinHandle;
use tr064::{ClientBuilder, Fingerprint, SecretString, Service, Url};

mod auth;
mod device;
mod fixture;
mod soap;
mod tls;
mod web;

use device::{Device, Fault, Hooks, short_id};
pub use fixture::{Answer, Error, Fixture};
use tls::TlsListener;
use web::Web;

/// The user name the simulator accepts by default.
pub const USERNAME: &str = "fritz-sim";
/// The password the simulator accepts by default.
pub const PASSWORD: &str = "fritz-sim-password";

const XML: &str = r#"text/xml; charset="utf-8""#;

/// How the simulator listens and whom it lets in.
#[derive(Debug, Clone)]
pub struct Options {
    /// Port 0 picks a free port.
    pub address: SocketAddr,
    pub username: String,
    pub password: String,
    /// Serve HTTPS with a new self-signed certificate, see
    /// [`Sim::fingerprint`].
    pub https: bool,
}

impl Default for Options {
    /// A free port on the loopback interface, plain HTTP, with [`USERNAME`]
    /// and [`PASSWORD`].
    fn default() -> Self {
        Self {
            address: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            username: USERNAME.to_owned(),
            password: PASSWORD.to_owned(),
            https: false,
        }
    }
}

/// A change to how the simulator behaves, until
/// [cleared](Sim::clear_injections).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Injection {
    /// Calls of an action fail with a UPnP error once their arguments are
    /// valid. `service` is any reference [`Service::matches`] accepts, like
    /// `WLANConfiguration1`.
    Fault {
        service: String,
        action: String,
        code: u16,
    },
    /// Every response is sent after a delay, to make clients time out.
    Delay(Duration),
    /// Writes succeed, but change nothing.
    DiscardWrites,
    /// A write of an action also sets a state variable of the same service,
    /// like a device that resets the settings of a feature when it is
    /// turned off. Discarded writes set nothing.
    SideEffect {
        service: String,
        action: String,
        variable: String,
        value: String,
    },
    /// The account is locked: every login fails, even with the right
    /// password.
    LockLogin,
    /// Every call of a getter that answers a state variable of a service
    /// first adds `step` to it, like a counter of bytes, and wraps around
    /// at `wrap`, or at 2^64 without it.
    Counter {
        service: String,
        variable: String,
        step: u64,
        wrap: Option<u64>,
    },
    /// Calls of an action break off without an answer, like a connection
    /// that fails while the call is sent. The call changes nothing.
    Disconnect { service: String, action: String },
    /// Calls of an action answer with a body that ends early, like a
    /// connection that closes while the answer is sent. The call changes
    /// nothing.
    Truncate { service: String, action: String },
    /// A call of `Reboot` of `DeviceConfig1` restarts the device: it
    /// answers the call, breaks off the next `down` requests without an
    /// answer, and then answers again, with the uptime of `DeviceInfo1`
    /// `GetInfo` reset to 0. With `serial`, another device comes back: it
    /// answers this serial number.
    Reboot { down: u32, serial: Option<String> },
    /// Posts of the page `page` of `/data.lua` whose form field `field` is
    /// `value` fail with 500, after the page took the form, like a page
    /// that remembers its filter but breaks off its answer.
    FailPage {
        page: String,
        field: String,
        value: String,
    },
}

impl Injection {
    /// Calls of `action` of `service` break off without an answer.
    pub fn disconnect(service: impl Into<String>, action: impl Into<String>) -> Self {
        Self::Disconnect {
            service: service.into(),
            action: action.into(),
        }
    }

    /// Calls of `action` of `service` answer with a body that ends early.
    pub fn truncate(service: impl Into<String>, action: impl Into<String>) -> Self {
        Self::Truncate {
            service: service.into(),
            action: action.into(),
        }
    }

    /// A call of `Reboot` restarts the device, which breaks off the next
    /// `down` requests.
    pub fn reboot(down: u32) -> Self {
        Self::Reboot { down, serial: None }
    }

    /// Posts of `page` with `value` in the form field `field` fail.
    pub fn fail_page(
        page: impl Into<String>,
        field: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::FailPage {
            page: page.into(),
            field: field.into(),
            value: value.into(),
        }
    }

    /// Calls of `action` of `service` fail with the UPnP error `code`.
    pub fn fault(service: impl Into<String>, action: impl Into<String>, code: u16) -> Self {
        Self::Fault {
            service: service.into(),
            action: action.into(),
            code,
        }
    }

    /// Writes of `action` of `service` also set the state `variable` of
    /// that service to `value`.
    pub fn side_effect(
        service: impl Into<String>,
        action: impl Into<String>,
        variable: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::SideEffect {
            service: service.into(),
            action: action.into(),
            variable: variable.into(),
            value: value.into(),
        }
    }

    /// Every read of the state `variable` of `service` adds `step` to it
    /// first, wrapping around at `wrap`.
    pub fn counter(
        service: impl Into<String>,
        variable: impl Into<String>,
        step: u64,
        wrap: Option<u64>,
    ) -> Self {
        Self::Counter {
            service: service.into(),
            variable: variable.into(),
            step,
            wrap,
        }
    }
}

/// A request the simulator got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    /// The action, for action calls.
    pub action: Option<String>,
    /// The input arguments, for action calls.
    pub arguments: Vec<(String, String)>,
    /// Whether the request carried valid credentials.
    pub authorized: bool,
}

/// A running simulator. It stops when dropped.
#[derive(Debug)]
pub struct Sim {
    url: Url,
    fingerprint: Option<Fingerprint>,
    shared: Arc<Shared>,
    server: JoinHandle<io::Result<()>>,
}

impl Sim {
    /// Serves `fixture` on a free port of the loopback interface, with
    /// [`USERNAME`] and [`PASSWORD`].
    ///
    /// # Errors
    ///
    /// The errors of binding the port.
    pub async fn start(fixture: Fixture) -> io::Result<Self> {
        Self::start_with(fixture, Options::default()).await
    }

    /// Serves `fixture` as configured.
    ///
    /// # Errors
    ///
    /// The errors of binding the address.
    pub async fn start_with(fixture: Fixture, options: Options) -> io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(options.address).await?;
        let scheme = if options.https { "https" } else { "http" };
        let url = Url::parse(&format!("{scheme}://{}", listener.local_addr()?))
            .map_err(io::Error::other)?;
        let https = options.https;
        let shared = Arc::new(Shared {
            options,
            inner: Mutex::new(Inner {
                web: Web::new(&fixture.pages),
                device: Device::new(fixture),
                injections: Vec::new(),
                requests: Vec::new(),
                down: 0,
            }),
        });
        let app = Router::new()
            .fallback(handle)
            .with_state(Arc::clone(&shared));
        let (server, fingerprint) = if https {
            let (listener, fingerprint) = TlsListener::new(listener);
            let server = tokio::spawn(axum::serve(listener, app).into_future());
            (server, Some(fingerprint))
        } else {
            (tokio::spawn(axum::serve(listener, app).into_future()), None)
        };
        Ok(Self {
            url,
            fingerprint,
            shared,
            server,
        })
    }

    /// The base URL, like `http://127.0.0.1:49000`.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The fingerprint of the certificate, if the simulator serves HTTPS.
    pub fn fingerprint(&self) -> Option<Fingerprint> {
        self.fingerprint
    }

    /// A client builder for the simulator, with its credentials and, for
    /// HTTPS, its certificate pinned.
    pub fn client(&self) -> ClientBuilder {
        let options = &self.shared.options;
        let builder = tr064::Client::builder(self.url.clone()).credentials(
            options.username.clone(),
            SecretString::from(options.password.clone()),
        );
        match self.fingerprint {
            Some(fingerprint) => builder.pin(fingerprint),
            None => builder,
        }
    }

    /// Changes how the simulator behaves from the next request on.
    pub fn inject(&self, injection: Injection) {
        self.shared.lock().injections.push(injection);
    }

    /// Removes all injections.
    pub fn clear_injections(&self) {
        self.shared.lock().injections.clear();
    }

    /// The requests the simulator got so far, oldest first.
    pub fn requests(&self) -> Vec<Request> {
        self.shared.lock().requests.clone()
    }

    /// The category the page `log` of the web interface remembers, like
    /// `net`: the value of its last field `filter`, or else of `filter` in
    /// the page of the fixture. `None` if neither is there.
    pub fn log_filter(&self) -> Option<String> {
        self.shared.lock().web.log_filter().map(str::to_owned)
    }

    /// The value of a state variable of a service, like `Enable` of
    /// `WLANConfiguration1`. `service` is any reference
    /// [`Service::matches`] accepts.
    pub fn state(&self, service: &str, variable: &str) -> Option<String> {
        let inner = self.shared.lock();
        let service = inner.device.fixture.description.service(service).ok()?;
        inner
            .device
            .state(short_id(service), variable)
            .map(str::to_owned)
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[derive(Debug)]
struct Shared {
    options: Options,
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    device: Device,
    web: Web,
    injections: Vec<Injection>,
    requests: Vec<Request>,
    /// The number of requests to break off while the device restarts.
    down: u32,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Answers a request, and the delay before the answer.
    fn respond(
        &self,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        body: &str,
    ) -> (Response, Duration) {
        let mut inner = self.lock();
        let path = uri.path();
        let call = (method == Method::POST)
            .then(|| soap::parse(body))
            .flatten();
        let locked = inner.injections.contains(&Injection::LockLogin);
        let authorized = !locked
            && auth::verify(
                headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()),
                &auth::Expected {
                    username: &self.options.username,
                    password: &self.options.password,
                    nonce: NONCE,
                    method: method.as_str(),
                    uri: uri.path_and_query().map_or(path, |p| p.as_str()),
                },
            );
        inner.requests.push(Request {
            method: method.to_string(),
            path: path.to_owned(),
            action: call.as_ref().map(|c| c.action.clone()),
            arguments: call.as_ref().map_or_default(|c| c.arguments.clone()),
            authorized,
        });
        let delay = inner
            .injections
            .iter()
            .map(|i| match i {
                Injection::Delay(delay) => *delay,
                _ => Duration::ZERO,
            })
            .sum();
        let response = match *method {
            _ if inner.down > 0 => {
                inner.down -= 1;
                broken()
            }
            _ if path == web::LOGIN => inner.web.login(method, body, &self.options, locked),
            _ if path.starts_with(web::API) => {
                let Inner { web, device, .. } = &*inner;
                web.api(&device.fixture.api, method, path, headers)
            }
            _ if path == web::DATA => {
                let Inner {
                    web,
                    device,
                    injections,
                    ..
                } = &mut *inner;
                web.page(&device.fixture.pages, injections, method, body)
            }
            _ if path == web::HOMEAUTO => {
                inner.web.stats(&inner.device.fixture.stats, method, body)
            }
            Method::GET => inner.document(path, uri.query()),
            Method::POST if !authorized => (
                StatusCode::UNAUTHORIZED,
                [(WWW_AUTHENTICATE, auth::challenge(NONCE))],
            )
                .into_response(),
            Method::POST => inner.control(path, headers, call),
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        };
        (response, delay)
    }
}

/// The nonce of every challenge. Clients may reuse it as long as they
/// like.
const NONCE: &str = "F1A2B3C4D5E6F708";

/// The session ID in the paths of list documents, in place of the `***` the
/// fixture recorded.
const SESSION: &str = "5e55104d0c0ffee5";

impl Inner {
    /// The device description, a service description, or a list document
    /// for the [session](SESSION).
    fn document(&self, path: &str, query: Option<&str>) -> Response {
        let fixture = &self.device.fixture;
        let name = path.trim_start_matches('/');
        if let Some(list) = name
            .strip_suffix(".lua")
            .and_then(|stem| fixture.lists.get(stem))
        {
            let session = query
                .into_iter()
                .flat_map(|q| q.split('&'))
                .any(|p| p == format!("sid={SESSION}"));
            return if session {
                list.clone().into_response()
            } else {
                StatusCode::FORBIDDEN.into_response()
            };
        }
        let xml = match name {
            "tr64desc.xml" => Some(&fixture.description_xml),
            name => fixture.scpds.get(name).map(|(xml, _)| xml),
        };
        match xml {
            Some(xml) => ([(CONTENT_TYPE, XML)], xml.clone()).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    /// Answers an authorized action call.
    fn control(&mut self, path: &str, headers: &HeaderMap, call: Option<soap::Call>) -> Response {
        let service = self
            .device
            .fixture
            .description
            .services()
            .find(|s| s.control_url == path)
            .cloned();
        let Some(service) = service else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some(call) = call else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let soap_action = headers
            .get("SOAPAction")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .trim_matches('"');
        let expected = format!("{}#{}", service.service_type, call.action);
        let result = if soap_action == expected && call.service_type == service.service_type {
            if let Some(response) = self.breaks(&service, &call.action) {
                return response;
            }
            let hooks = self.hooks(&service, &call.action);
            let result = self
                .device
                .call(&service, &call.action, &call.arguments, &hooks);
            if result.is_ok() {
                self.reboot(&service, &call.action);
            }
            result
        } else {
            Err(Fault::new(401))
        };
        match result {
            Ok(mut out) => {
                for (_, value) in &mut out {
                    *value = value.replace("sid=***", &format!("sid={SESSION}"));
                }
                let xml = soap::response(&service.service_type, &call.action, &out);
                ([(CONTENT_TYPE, XML)], xml).into_response()
            }
            Err(fault) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(CONTENT_TYPE, XML)],
                soap::fault(&fault),
            )
                .into_response(),
        }
    }

    /// The broken answer to calls of `action`, if they break off.
    fn breaks(&self, service: &Service, action: &str) -> Option<Response> {
        self.injections.iter().find_map(|i| match i {
            Injection::Disconnect {
                service: reference,
                action: name,
            } if name == action && service.matches(reference) => Some(broken()),
            Injection::Truncate {
                service: reference,
                action: name,
            } if name == action && service.matches(reference) => Some(truncated()),
            _ => None,
        })
    }

    /// Restarts the device after a call of `Reboot`, if a test
    /// [injected](Injection::Reboot) it. `None` if not.
    fn reboot(&mut self, service: &Service, action: &str) -> Option<()> {
        (action == "Reboot" && service.matches("DeviceConfig1")).then_some(())?;
        let reboot = self
            .injections
            .iter()
            .find(|i| matches!(i, Injection::Reboot { .. }));
        let Some(Injection::Reboot { down, serial }) = reboot.cloned() else {
            return None;
        };
        self.down = down;
        let device = &mut self.device;
        device.set_output("DeviceInfo1", "GetInfo", "NewUpTime", "0")?;
        device.set_output("DeviceInfo1", "GetInfo", "NewSerialNumber", &serial?)
    }

    fn hooks(&self, service: &Service, action: &str) -> Hooks {
        let fault = self.injections.iter().find_map(|i| match i {
            Injection::Fault {
                service: reference,
                action: name,
                code,
            } if name == action && service.matches(reference) => Some(Fault::new(*code)),
            _ => None,
        });
        let side_effects = self
            .injections
            .iter()
            .filter_map(|i| match i {
                Injection::SideEffect {
                    service: reference,
                    action: name,
                    variable,
                    value,
                } if name == action && service.matches(reference) => {
                    Some((variable.clone(), value.clone()))
                }
                _ => None,
            })
            .collect();
        let counters = self
            .injections
            .iter()
            .filter_map(|i| match i {
                Injection::Counter {
                    service: reference,
                    variable,
                    step,
                    wrap,
                } if service.matches(reference) => Some((variable.clone(), *step, *wrap)),
                _ => None,
            })
            .collect();
        Hooks {
            fault,
            discard_writes: self.injections.contains(&Injection::DiscardWrites),
            side_effects,
            counters,
        }
    }
}

/// A response that breaks off: its body fails before it ends, so the
/// client gets no answer.
fn broken() -> Response {
    (StatusCode::OK, [(CONTENT_LENGTH, "1")], Body::new(Broken)).into_response()
}

/// A response whose SOAP envelope ends early.
fn truncated() -> Response {
    (StatusCode::OK, "<?xml version=\"1.0\"?><s:Envelope").into_response()
}

/// A body that fails.
struct Broken;

impl HttpBody for Broken {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, io::Error>>> {
        Poll::Ready(Some(Err(io::ErrorKind::ConnectionReset.into())))
    }
}

async fn handle(
    State(shared): State<Arc<Shared>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (response, delay) = shared.respond(&method, &uri, &headers, &body);
    tokio::time::sleep(delay).await;
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/fritzbox-7690/8.25");
    const WLAN: &str = "urn:dslforum-org:service:WLANConfiguration:1";
    const CONTROL: &str = "/upnp/control/wlanconfig1";

    fn inner() -> Inner {
        Inner {
            device: Device::new(Fixture::load(FIXTURE).unwrap()),
            web: Web::default(),
            injections: Vec::new(),
            requests: Vec::new(),
            down: 0,
        }
    }

    /// A call of `GetInfo`.
    fn call(service_type: &str) -> soap::Call {
        soap::Call {
            service_type: service_type.to_owned(),
            action: "GetInfo".to_owned(),
            arguments: Vec::new(),
        }
    }

    fn soap_action(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("SOAPAction", value.parse().unwrap());
        headers
    }

    #[test]
    fn answers_calls_that_match_the_service() {
        let headers = soap_action(&format!("\"{WLAN}#GetInfo\""));
        let response = inner().control(CONTROL, &headers, Some(call(WLAN)));
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn rejects_calls_that_do_not_match_the_service() {
        let mut inner = inner();
        let right = soap_action(&format!("{WLAN}#GetInfo"));
        let wrong = soap_action(&format!("{WLAN}#SetEnable"));
        for (headers, service_type) in [
            (&wrong, WLAN),
            (&HeaderMap::new(), WLAN),
            (&right, "urn:other"),
        ] {
            let response = inner.control(CONTROL, headers, Some(call(service_type)));
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }
        let response = inner.control("/upnp/control/missing", &right, Some(call(WLAN)));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = inner.control(CONTROL, &right, None);
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn allows_only_get_and_post() {
        let shared = Shared {
            options: Options::default(),
            inner: Mutex::new(inner()),
        };
        let uri = Uri::from_static("/tr64desc.xml");
        let (response, delay) = shared.respond(&Method::PUT, &uri, &HeaderMap::new(), "");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(delay, Duration::ZERO);
        assert_eq!(shared.lock().requests[0].method, "PUT");
    }
}
