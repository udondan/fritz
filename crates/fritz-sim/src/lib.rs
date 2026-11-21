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
//! - Paths to list documents, like the one `X_AVM-DE_GetHostListPath`
//!   returns, hold a session ID, and the documents are served for it.
//! - The web interface has a session login, `/login_sid.lua`, with a
//!   PBKDF2 challenge, and serves the documents of the REST API, like
//!   `/api/v0/smarthome/overview`, for the open session.
//! - Tests can [inject](Sim::inject) UPnP errors, delays, discarded writes
//!   and a locked login, and read the [requests](Sim::requests) the server
//!   got.
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
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
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
pub use fixture::{Answer, Error, FIXTURES, Fixture};
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
    /// The account is locked: every login fails, even with the right
    /// password.
    LockLogin,
}

impl Injection {
    /// Calls of `action` of `service` fail with the UPnP error `code`.
    pub fn fault(service: impl Into<String>, action: impl Into<String>, code: u16) -> Self {
        Self::Fault {
            service: service.into(),
            action: action.into(),
            code,
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
                device: Device::new(fixture),
                web: Web::default(),
                injections: Vec::new(),
                requests: Vec::new(),
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
            _ if path == web::LOGIN => inner.web.login(method, body, &self.options, locked),
            _ if path.starts_with(web::API) => {
                let Inner { web, device, .. } = &*inner;
                web.api(&device.fixture.api, method, path, headers)
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
            let hooks = self.hooks(&service, &call.action);
            self.device
                .call(&service, &call.action, &call.arguments, &hooks)
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

    fn hooks(&self, service: &Service, action: &str) -> Hooks {
        let fault = self.injections.iter().find_map(|i| match i {
            Injection::Fault {
                service: reference,
                action: name,
                code,
            } if name == action && service.matches(reference) => Some(Fault::new(*code)),
            _ => None,
        });
        Hooks {
            fault,
            discard_writes: self.injections.contains(&Injection::DiscardWrites),
        }
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
