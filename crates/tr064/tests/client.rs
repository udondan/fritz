//! The client against a fake device on the loopback interface.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use md5::{Digest as _, Md5};
use tr064::{Client, Error, SecretString, Service, Url};

const DESCRIPTION: &str = include_str!("data/tr64desc.xml");
const SCPD: &str = include_str!("data/deviceinfoSCPD.xml");
const SERVICE_TYPE: &str = "urn:dslforum-org:service:DeviceInfo:1";

#[derive(Default)]
struct Device {
    requests: AtomicUsize,
    nonce: AtomicU32,
}

impl Device {
    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    fn nonce(&self) -> String {
        format!("nonce-{}", self.nonce.load(Ordering::SeqCst))
    }

    /// Whether the request carries a valid answer to the current challenge.
    fn authorized(&self, headers: &HeaderMap, method: &Method, uri: &str) -> bool {
        let Some(params) = headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Digest "))
            .map(|v| {
                v.split(", ")
                    .filter_map(|p| p.split_once('='))
                    .map(|(k, v)| (k.to_owned(), v.trim_matches('"').to_owned()))
                    .collect::<HashMap<_, _>>()
            })
        else {
            return false;
        };
        let ha1 = md5_hex("user:test:secret");
        let ha2 = md5_hex(&format!("{method}:{uri}"));
        let expected = md5_hex(&format!(
            "{ha1}:{}:{}:{}:auth:{ha2}",
            self.nonce(),
            params["nc"],
            params["cnonce"]
        ));
        params["username"] == "user"
            && params["uri"] == uri
            && params["nonce"] == self.nonce()
            && params["response"] == expected
    }

    fn challenge(&self) -> Response {
        let mut headers = HeaderMap::new();
        headers.append(
            WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"test\""),
        );
        headers.append(
            WWW_AUTHENTICATE,
            HeaderValue::from_str(&format!(
                r#"Digest realm="test", nonce="{}", qop="auth", algorithm=MD5"#,
                self.nonce()
            ))
            .unwrap(),
        );
        (StatusCode::UNAUTHORIZED, headers).into_response()
    }
}

fn md5_hex(data: &str) -> String {
    Md5::digest(data.as_bytes())
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn envelope(body: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body>{body}</s:Body></s:Envelope>"#
    )
}

async fn control(State(device): State<Arc<Device>>, headers: HeaderMap, body: String) -> Response {
    device.requests.fetch_add(1, Ordering::SeqCst);
    if !device.authorized(&headers, &Method::POST, "/upnp/control/deviceinfo") {
        return device.challenge();
    }
    let action = headers["SOAPAction"]
        .to_str()
        .unwrap()
        .strip_prefix(&format!("\"{SERVICE_TYPE}#"))
        .and_then(|a| a.strip_suffix('"'))
        .unwrap()
        .to_owned();
    assert_eq!(headers[CONTENT_TYPE], r#"text/xml; charset="utf-8""#);
    let request = roxmltree::Document::parse(&body).unwrap();
    let call = request
        .descendants()
        .find(|n| n.tag_name().name() == action)
        .unwrap();
    assert_eq!(call.tag_name().namespace(), Some(SERVICE_TYPE));
    let response = |inner: &str| {
        envelope(&format!(
            r#"<u:{action}Response xmlns:u="{SERVICE_TYPE}">{inner}</u:{action}Response>"#
        ))
    };
    match action.as_str() {
        "GetInfo" => response(
            "<NewManufacturerName>Example</NewManufacturerName><NewUpTime>42</NewUpTime><NewEmpty/>",
        )
        .into_response(),
        "Echo" => {
            let value = call
                .children()
                .find(|n| n.tag_name().name() == "NewValue")
                .and_then(|n| n.text())
                .unwrap_or_default()
                .replace('&', "&amp;")
                .replace('<', "&lt;");
            response(&format!("<NewValue>{value}</NewValue>")).into_response()
        }
        "Fail" => (
            StatusCode::INTERNAL_SERVER_ERROR,
            envelope(
                "<s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring>\
                 <detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\">\
                 <errorCode>606</errorCode><errorDescription>Action not authorized</errorDescription>\
                 </UPnPError></detail></s:Fault>",
            ),
        )
            .into_response(),
        "Broken" => (StatusCode::INTERNAL_SERVER_ERROR, "oops").into_response(),
        "Teapot" => StatusCode::IM_A_TEAPOT.into_response(),
        "Wrong" => envelope("<OtherResponse/>").into_response(),
        "NoBody" => "<s:Envelope xmlns:s=\"x\"/>".into_response(),
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
}

async fn protected(State(device): State<Arc<Device>>, headers: HeaderMap, uri: Uri) -> Response {
    device.requests.fetch_add(1, Ordering::SeqCst);
    let uri = uri.path_and_query().unwrap().as_str();
    if device.authorized(&headers, &Method::GET, uri) {
        "welcome".into_response()
    } else {
        device.challenge()
    }
}

async fn start() -> (Url, Arc<Device>) {
    let device = Arc::new(Device::default());
    let app = Router::new()
        .route("/tr64desc.xml", get(|| async { DESCRIPTION }))
        .route("/deviceinfoSCPD.xml", get(|| async { SCPD }))
        .route("/upnp/control/deviceinfo", post(control))
        .route("/protected", get(protected))
        .route(
            "/expire",
            get(|State(device): State<Arc<Device>>| async move {
                device.nonce.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .route(
            "/basic",
            get(|| async {
                (
                    StatusCode::UNAUTHORIZED,
                    [(WWW_AUTHENTICATE, "Basic realm=\"test\"")],
                )
            }),
        )
        .route("/no-challenge", get(|| async { StatusCode::UNAUTHORIZED }))
        .route(
            "/session",
            get(|headers: HeaderMap| async move {
                if headers.get("Authorization").and_then(|v| v.to_str().ok()) == Some("Token abc") {
                    "session".into_response()
                } else {
                    StatusCode::FORBIDDEN.into_response()
                }
            }),
        )
        .route(
            "/form",
            post(|headers: HeaderMap, body: String| async move {
                let kind = headers
                    .get(CONTENT_TYPE)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                format!("{kind} {body}")
            }),
        )
        .with_state(Arc::clone(&device));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, device)
}

fn client(url: Url, password: &str) -> Client {
    Client::builder(url)
        .credentials("user", SecretString::from(password))
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

async fn device_info(client: &Client) -> Service {
    let description = client.description().await.unwrap();
    description.service("DeviceInfo1").unwrap().clone()
}

#[tokio::test]
async fn fetches_description_and_scpd() {
    let (url, _) = start().await;
    let client = Client::builder(url).build().unwrap();
    let service = device_info(&client).await;
    let scpd = client.scpd(&service).await.unwrap();
    assert!(scpd.action("GetInfo").is_some());
}

#[tokio::test]
async fn calls_action_with_digest_auth() {
    let (url, device) = start().await;
    let client = client(url, "secret");
    let service = device_info(&client).await;

    let out = client.call(&service, "GetInfo", &[]).await.unwrap();
    assert_eq!(out.get("NewManufacturerName"), Some("Example"));
    assert_eq!(out.get("NewUpTime"), Some("42"));
    assert_eq!(out.get("NewEmpty"), Some(""));
    assert_eq!(out.get("NewMissing"), None);
    assert_eq!(out.len(), 3);
    assert!(!out.is_empty());
    assert_eq!(
        out.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        ["NewManufacturerName", "NewUpTime", "NewEmpty"]
    );
    // The first call answers the challenge, the second reuses the answer.
    assert_eq!(device.requests(), 2);
    client.call(&service, "GetInfo", &[]).await.unwrap();
    assert_eq!(device.requests(), 3);
}

#[tokio::test]
async fn escapes_argument_values() {
    let (url, _) = start().await;
    let client = client(url, "secret");
    let service = device_info(&client).await;
    let value = r#"a<b & "c" 'd' >"#;
    let out = client
        .call(&service, "Echo", &[("NewValue", value)])
        .await
        .unwrap();
    let pairs: Vec<_> = out.into_iter().collect();
    assert_eq!(pairs, [("NewValue".to_owned(), value.to_owned())]);
}

#[tokio::test]
async fn answers_new_challenge_when_nonce_expires() {
    let (url, device) = start().await;
    let client = client(url, "secret");
    assert_eq!(client.get("/protected?x=1").await.unwrap(), "welcome");
    assert_eq!(device.requests(), 2);
    client.get("/expire").await.unwrap();
    assert_eq!(client.get("/protected?x=1").await.unwrap(), "welcome");
    assert_eq!(device.requests(), 4);
}

#[tokio::test]
async fn stops_after_rejected_password() {
    let (url, device) = start().await;
    let client = client(url, "wrong");
    let err = client.get("/protected").await.unwrap_err();
    assert!(matches!(err, Error::AuthenticationFailed));
    assert_eq!(err.to_string(), "the device rejected the credentials");
    assert_eq!(device.requests(), 2);
}

#[tokio::test]
async fn requires_credentials() {
    let (url, device) = start().await;
    let client = Client::builder(url).build().unwrap();
    let err = client.get("/protected").await.unwrap_err();
    assert!(matches!(err, Error::CredentialsRequired));
    assert_eq!(err.to_string(), "the device requires credentials");
    assert_eq!(device.requests(), 1);
}

#[tokio::test]
async fn rejects_unsupported_challenges() {
    let (url, _) = start().await;
    let client = client(url, "secret");
    let err = client.get("/basic").await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported authentication challenge: Basic realm=\"test\""
    );
    let err = client.get("/no-challenge").await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported authentication challenge: no WWW-Authenticate header"
    );
}

#[tokio::test]
async fn sends_additional_headers() {
    let (url, _) = start().await;
    let client = Client::builder(url).build().unwrap();
    let body = client
        .get_with_headers("/session", &[("Authorization", "Token abc")])
        .await
        .unwrap();
    assert_eq!(body, "session");
    let err = client.get("/session").await.unwrap_err();
    assert!(matches!(err, Error::Status { status: 403, .. }), "{err:?}");
    let err = client
        .get_with_headers("/session", &[("in valid", "x")])
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Http(_)), "{err:?}");
}

#[tokio::test]
async fn posts_forms() {
    let (url, _) = start().await;
    let client = Client::builder(url).build().unwrap();
    let body = client
        .post_form("/form", &[("username", "a b"), ("response", "x&y=z")])
        .await
        .unwrap();
    assert_eq!(
        body,
        "application/x-www-form-urlencoded username=a+b&response=x%26y%3Dz"
    );
    let err = client
        .post_form("http://192.0.2.1/form", &[])
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidUrl(_)), "{err:?}");
}

#[tokio::test]
async fn reports_upnp_errors() {
    let (url, _) = start().await;
    let client = client(url, "secret");
    let service = device_info(&client).await;
    let err = client.call(&service, "Fail", &[]).await.unwrap_err();
    assert!(matches!(err, Error::Upnp { code: 606, .. }));
    assert_eq!(err.to_string(), "UPnP error 606: Action not authorized");
}

#[tokio::test]
async fn reports_http_status() {
    let (url, _) = start().await;
    let client = client(url.clone(), "secret");
    let service = device_info(&client).await;
    let control = url.join("/upnp/control/deviceinfo").unwrap();
    for (action, status) in [("Broken", 500), ("Teapot", 418)] {
        let err = client.call(&service, action, &[]).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("HTTP status {status} from {control}")
        );
    }
    let err = client.get("/missing").await.unwrap_err();
    assert!(matches!(err, Error::Status { status: 404, .. }));
}

#[tokio::test]
async fn rejects_unexpected_responses() {
    let (url, _) = start().await;
    let client = client(url, "secret");
    let service = device_info(&client).await;
    let err = client.call(&service, "Wrong", &[]).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid response: no <WrongResponse> in the SOAP Body"
    );
    let err = client.call(&service, "NoBody", &[]).await.unwrap_err();
    assert_eq!(err.to_string(), "invalid response: no SOAP Body");
}

#[tokio::test]
async fn rejects_invalid_names_before_sending() {
    let (url, device) = start().await;
    let client = client(url, "secret");
    let service = device_info(&client).await;
    let err = client.call(&service, "Get Info", &[]).await.unwrap_err();
    assert_eq!(err.to_string(), r#"invalid action name "Get Info""#);
    let err = client
        .call(&service, "GetInfo", &[("1st", "x")])
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), r#"invalid argument name "1st""#);
    let err = client.call(&service, "", &[]).await.unwrap_err();
    assert_eq!(err.to_string(), r#"invalid action name """#);
    assert_eq!(device.requests(), 0);
}

#[tokio::test]
async fn keeps_requests_on_the_device() {
    let (url, _) = start().await;
    let client = client(url.clone(), "secret");
    let err = client.get("http://192.0.2.1/x").await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "invalid URL: http://192.0.2.1/x is not on the device {}",
            url.origin().ascii_serialization()
        )
    );
    let err = client.get("http://[::1").await.unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"invalid URL: "http://[::1": invalid IPv6 address"#
    );
}

#[tokio::test]
async fn reports_unreachable_devices() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    drop(listener);
    let err = Client::builder(url)
        .build()
        .unwrap()
        .get("/")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Http(_)));
    assert!(err.to_string().starts_with("HTTP request failed: "));
}
