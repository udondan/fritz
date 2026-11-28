//! Running a capture against a fake device and writing it to disk.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use fritz_core::capture::{Capture, List, Options, Outcome, plan, run, write};
use tr064::{Client, Error, Url};

const DESCRIPTION: &str = include_str!("data/tr64desc.xml");
const DEVICE_INFO: &str = include_str!("data/deviceinfoSCPD.xml");
const HOSTS: &str = include_str!("data/hostsSCPD.xml");
const ONTEL: &str = include_str!("data/ontelSCPD.xml");

struct Device {
    /// Whether the device locks the account at the second call.
    locks: bool,
    /// The status the host list is served with.
    list: StatusCode,
    calls: AtomicUsize,
}

async fn start(locks: bool, list: StatusCode) -> (Client, Arc<Device>, Url) {
    let device = Arc::new(Device {
        locks,
        list,
        calls: AtomicUsize::new(0),
    });
    let app = Router::new()
        .route("/tr64desc.xml", get(|| async { DESCRIPTION }))
        .route("/deviceinfoSCPD.xml", get(|| async { DEVICE_INFO }))
        .route("/hostsSCPD.xml", get(|| async { HOSTS }))
        .route("/x_contactSCPD.xml", get(|| async { ONTEL }))
        .route("/upnp/control/{service}", post(control))
        .route(
            "/devicehostlist.lua",
            get(|State(device): State<Arc<Device>>| async move { (device.list, "<List/>") }),
        )
        .with_state(Arc::clone(&device));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (Client::builder(url.clone()).build().unwrap(), device, url)
}

fn ok(action: &str, inner: &str) -> Response {
    envelope(&format!(
        r#"<u:{action}Response xmlns:u="urn:x">{inner}</u:{action}Response>"#
    ))
    .into_response()
}

fn fault(code: u16, description: &str) -> Response {
    let body = envelope(&format!(
        "<s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring>\
         <detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\">\
         <errorCode>{code}</errorCode><errorDescription>{description}</errorDescription>\
         </UPnPError></detail></s:Fault>"
    ));
    (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
}

fn envelope(body: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body>{body}</s:Body></s:Envelope>"#
    )
}

async fn control(State(device): State<Arc<Device>>, headers: HeaderMap, body: String) -> Response {
    let calls = device.calls.fetch_add(1, Ordering::SeqCst) + 1;
    if device.locks && calls == 2 {
        return fault(867, "locked");
    }
    let soap_action = headers["SOAPAction"].to_str().unwrap().trim_matches('"');
    let action = soap_action.rsplit('#').next().unwrap();
    match action {
        "GetInfo" => ok(action, "<NewSerialNumber>0123456789AB</NewSerialNumber>"),
        "GetSecurityPort" => fault(606, "Action not authorized"),
        "GetHostNumberOfEntries" => {
            ok(action, "<NewHostNumberOfEntries>2</NewHostNumberOfEntries>")
        }
        "GetGenericHostEntry" => match body.split("<NewIndex>").nth(1).unwrap() {
            index if index.starts_with("0<") => ok(action, "<NewHostName>laptop</NewHostName>"),
            index if index.starts_with("1<") => ok(action, "<NewHostName>phone</NewHostName>"),
            _ => fault(713, "SpecifiedArrayIndexInvalid"),
        },
        "X_AVM-DE_GetHostListPath" => ok(
            action,
            "<NewX_AVM-DE_HostListPath>/devicehostlist.lua?sid=0123abcd</NewX_AVM-DE_HostListPath>",
        ),
        "GetCallList" => ok(
            action,
            "<NewCallListURL>http://box:49000/calllist.lua?sid=0123abcd</NewCallListURL>",
        ),
        _ => (StatusCode::BAD_GATEWAY, "oops").into_response(),
    }
}

fn read(path: &std::path::Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[tokio::test]
async fn calls_the_planned_getters_and_writes_their_answers() {
    let (client, device, url) = start(false, StatusCode::OK).await;
    let plan = plan(&client).await.unwrap();
    let options = Options {
        delay: Duration::ZERO,
        ..Options::default()
    };
    let mut lines = Vec::new();
    let capture = run(&client, &plan, &options, |r| lines.push(r.to_string())).await;

    assert_eq!(
        lines,
        [
            "DeviceInfo1 GetInfo: ok, 1 values",
            "DeviceInfo1 GetSecurityPort: fault 606 Action not authorized",
            "Hosts1 GetHostNumberOfEntries: ok, 1 values",
            "Hosts1 GetGenericHostEntry@0: ok, 1 values",
            "Hosts1 GetGenericHostEntry@1: ok, 1 values",
            "Hosts1 GetGenericHostEntry@2: fault 713 SpecifiedArrayIndexInvalid",
            "Hosts1 X_AVM-DE_GetHostListPath: ok, 1 values",
            &format!(
                "X_AVM-DE_OnTel1 GetNumberOfDeflections: failed, HTTP status 502 from {url}upnp/control/x_contact"
            ),
        ]
    );
    assert_eq!(device.calls.load(Ordering::SeqCst), 8);
    assert!(capture.stopped.is_none());
    assert_eq!(
        capture.to_string(),
        "8 calls: 5 ok, 2 faults, 1 failed, 1 of 1 lists"
    );

    let dir = tempfile::tempdir().unwrap();
    write(&plan, &capture, dir.path()).unwrap();
    assert_eq!(read(&dir.path().join("tr64desc.xml")), DESCRIPTION);
    assert_eq!(read(&dir.path().join("scpd/hostsSCPD.xml")), HOSTS);
    assert!(!dir.path().join("scpd/missingSCPD.xml").exists());
    assert_eq!(
        read(&dir.path().join("lists/devicehostlist.xml")),
        "<List/>"
    );
    let responses = dir.path().join("responses");
    assert_eq!(
        read(&responses.join("Hosts1/GetGenericHostEntry@1.json")),
        "{\n  \"out\": {\n    \"NewHostName\": \"phone\"\n  },\n  \"status\": \"ok\"\n}\n"
    );
    assert!(
        read(&responses.join("Hosts1/X_AVM-DE_GetHostListPath.json"))
            .contains("\"/devicehostlist.lua?sid=***\"")
    );
    assert!(read(&responses.join("DeviceInfo1/GetSecurityPort.json")).contains("\"code\": 606"));
    assert!(
        read(&responses.join("X_AVM-DE_OnTel1/GetNumberOfDeflections.json"))
            .contains("\"status\": \"failed\"")
    );
    assert!(!responses.join("X_AVM-DE_OnTel1/GetCallList.json").exists());
}

#[tokio::test]
async fn calls_personal_getters_when_asked() {
    let (client, _, _) = start(false, StatusCode::OK).await;
    let plan = plan(&client).await.unwrap();
    let options = Options {
        include_personal: true,
        max_entries: 1,
        delay: Duration::ZERO,
    };
    let capture = run(&client, &plan, &options, |_| {}).await;
    let called: Vec<String> = capture
        .responses
        .iter()
        .map(|r| format!("{} {:?}", r.action, r.index))
        .collect();
    assert!(called.contains(&"GetGenericHostEntry Some(0)".to_owned()));
    assert!(!called.contains(&"GetGenericHostEntry Some(1)".to_owned()));
    let call_list = capture
        .responses
        .iter()
        .find(|r| r.action == "GetCallList")
        .unwrap();
    assert_eq!(
        call_list.outcome,
        Outcome::Ok(vec![(
            "NewCallListURL".to_owned(),
            "http://box:49000/calllist.lua?sid=***".to_owned()
        )])
    );
}

#[tokio::test]
async fn stops_when_the_account_is_locked() {
    let (client, device, _) = start(true, StatusCode::OK).await;
    let plan = plan(&client).await.unwrap();
    let options = Options {
        delay: Duration::ZERO,
        ..Options::default()
    };
    let capture: Capture = run(&client, &plan, &options, |_| {}).await;
    assert_eq!(device.calls.load(Ordering::SeqCst), 2);
    assert_eq!(capture.responses.len(), 1);
    assert!(
        matches!(capture.stopped, Some(Error::Upnp { code: 867, .. })),
        "{:?}",
        capture.stopped
    );
}

#[tokio::test]
async fn records_lists_it_cannot_fetch() {
    let (client, _, url) = start(false, StatusCode::NOT_FOUND).await;
    let plan = plan(&client).await.unwrap();
    let options = Options {
        delay: Duration::ZERO,
        ..Options::default()
    };
    let capture = run(&client, &plan, &options, |_| {}).await;
    assert!(capture.stopped.is_none());
    assert_eq!(
        capture.lists,
        [List {
            name: "devicehostlist.xml",
            outcome: Err(format!(
                "HTTP status 404 from {url}devicehostlist.lua?sid=***"
            )),
        }]
    );
    assert!(capture.to_string().ends_with(", 0 of 1 lists"));
    let dir = tempfile::tempdir().unwrap();
    write(&plan, &capture, dir.path()).unwrap();
    assert!(!dir.path().join("lists").exists());
}

#[tokio::test]
async fn stops_when_a_list_needs_credentials() {
    let (client, _, _) = start(false, StatusCode::UNAUTHORIZED).await;
    let plan = plan(&client).await.unwrap();
    let options = Options {
        delay: Duration::ZERO,
        ..Options::default()
    };
    let capture = run(&client, &plan, &options, |_| {}).await;
    assert!(
        matches!(capture.stopped, Some(Error::CredentialsRequired)),
        "{:?}",
        capture.stopped
    );
    assert_eq!(capture.responses.len(), 7);
    assert_eq!(capture.lists, []);
}

#[test]
fn fails_to_write_into_a_missing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let plan = fritz_core::capture::Plan {
        description: String::new(),
        services: Vec::new(),
    };
    let error = write(&plan, &Capture::default(), &dir.path().join("missing")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}
