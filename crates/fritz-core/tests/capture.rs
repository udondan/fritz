//! Planning a capture against a fake device.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::routing::get;
use fritz_core::capture::{Summary, plan};
use tr064::{Client, Error, Url};

const DESCRIPTION: &str = include_str!("data/tr64desc.xml");
const DEVICE_INFO: &str = include_str!("data/deviceinfoSCPD.xml");
const HOSTS: &str = include_str!("data/hostsSCPD.xml");
const ONTEL: &str = include_str!("data/ontelSCPD.xml");

/// Serves the documents and counts the requests of each method.
async fn start(description: bool) -> (Url, Arc<[AtomicUsize; 2]>) {
    let counts = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
    let mut app = Router::new()
        .route("/deviceinfoSCPD.xml", get(|| async { DEVICE_INFO }))
        .route("/hostsSCPD.xml", get(|| async { HOSTS }))
        .route("/x_contactSCPD.xml", get(|| async { ONTEL }));
    if description {
        app = app.route("/tr64desc.xml", get(|| async { DESCRIPTION }));
    }
    let app = app.layer(middleware::from_fn_with_state(
        Arc::clone(&counts),
        |State(counts): State<Arc<[AtomicUsize; 2]>>, request: Request, next: Next| async move {
            let slot = usize::from(request.method() != axum::http::Method::GET);
            counts[slot].fetch_add(1, Ordering::SeqCst);
            next.run(request).await
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, counts)
}

#[tokio::test]
async fn plans_from_the_descriptions_without_calling_actions() {
    let (url, counts) = start(true).await;
    let client = Client::builder(url.clone()).build().unwrap();
    let plan = plan(&client).await.unwrap();

    // The description and four SCPDs, nothing else.
    assert_eq!(counts[0].load(Ordering::SeqCst), 5);
    assert_eq!(counts[1].load(Ordering::SeqCst), 0);
    assert_eq!(
        plan.summary(),
        Summary {
            call: 5,
            iterate: 1,
            needs_input: 2,
            personal: 1,
            denied: 1,
            not_getter: 2,
            failed_services: 1,
        }
    );
    let expected = format!(
        "\
DeviceInfo:1 (DeviceInfo1)
  call         GetInfo
  call         GetSecurityPort
  never        not getters: 1

Hosts:1 (Hosts1)
  call         GetHostNumberOfEntries
  iterate      GetGenericHostEntry (NewIndex)
  needs input  GetSpecificHostEntry (NewMACAddress)
  needs input  X_AVM-DE_GetAutoWakeOnLANByMACAddress (NewMACAddress)
  call         X_AVM-DE_GetHostListPath
  denied       X_AVM-DE_GetHostListToken: the name points to a secret
  never        not getters: 1

X_AVM-DE_OnTel:1 (X_AVM-DE_OnTel1)
  personal     GetCallList
  call         GetNumberOfDeflections

Missing:1 (Missing1)
  SCPD failed: HTTP status 404 from {url}missingSCPD.xml

4 services: 5 to call, 1 to iterate, 2 need input, 1 personal, 1 denied, 2 not getters, 1 SCPDs failed"
    );
    assert_eq!(plan.to_string(), expected);
}

#[tokio::test]
async fn fails_without_a_description() {
    let (url, _) = start(false).await;
    let client = Client::builder(url).build().unwrap();
    let error = plan(&client).await.unwrap_err();
    assert!(
        matches!(error, Error::Status { status: 404, .. }),
        "{error:?}"
    );
}
