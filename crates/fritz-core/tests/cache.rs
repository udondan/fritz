//! The cache of the service descriptions against the simulator.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use fritz_core::cache::Cache;
use fritz_core::policy;
use fritz_sim::{Fixture, Sim};
use tr064::Client;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritzbox-7690/8.25"
);

/// The class of every action of every service, as one line each.
async fn classes(client: &Client) -> Vec<String> {
    let description = client.description().await.unwrap();
    let mut lines = Vec::new();
    for service in description.services() {
        let scpd = client.scpd(service).await.unwrap();
        for action in &scpd.actions {
            let class = policy::classify(service, action);
            lines.push(format!("{} {} {class}", service.service_id, action.name));
        }
    }
    lines
}

/// The number of SCPDs the simulator served.
fn scpd_requests(sim: &Sim) -> usize {
    sim.requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path != "/tr64desc.xml")
        .count()
}

fn cached(sim: &Sim, dir: &Path) -> Client {
    sim.client()
        .cache(Arc::new(Cache::new(dir)))
        .build()
        .unwrap()
}

#[tokio::test]
async fn classifies_like_without_the_cache() {
    for name in Fixture::bundled_names() {
        let sim = Sim::start(Fixture::bundled(&name).unwrap()).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let expected = classes(&sim.client().build().unwrap()).await;
        let uncached = scpd_requests(&sim);
        assert!(uncached > 0, "{name}");

        // The first run fills the cache, the second reads it, in a new
        // process as far as the cache can tell.
        assert_eq!(classes(&cached(&sim, dir.path())).await, expected, "{name}");
        let first = scpd_requests(&sim);
        assert_eq!(classes(&cached(&sim, dir.path())).await, expected, "{name}");
        // Devices without a firmware version in their description, like
        // the repeaters, are not cached.
        let versioned = fs::read_dir(dir.path()).unwrap().count() > 0;
        let second = if versioned { 0 } else { uncached };
        assert_eq!(scpd_requests(&sim), first + second, "{name}");
    }
}

#[tokio::test]
async fn sends_no_scpd_requests_on_the_second_run() {
    let sim = Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    classes(&cached(&sim, dir.path())).await;
    let first = scpd_requests(&sim);
    assert!(first > 0);
    classes(&cached(&sim, dir.path())).await;
    assert_eq!(scpd_requests(&sim), first);
}

/// Copies a directory with its subdirectories.
fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[tokio::test]
async fn reads_again_after_a_firmware_update() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("fixture");
    copy(Path::new(FIXTURE), &fixture);
    let cache = dir.path().join("cache");

    let sim = Sim::start(Fixture::load(&fixture).unwrap()).await.unwrap();
    let before = classes(&cached(&sim, &cache)).await;
    let first = scpd_requests(&sim);
    drop(sim);

    let description = fixture.join("tr64desc.xml");
    let xml = fs::read_to_string(&description).unwrap();
    let updated = xml.replace(
        "<Display>285.08.25</Display>",
        "<Display>285.08.30</Display>",
    );
    assert_ne!(xml, updated);
    fs::write(&description, updated).unwrap();

    let sim = Sim::start(Fixture::load(&fixture).unwrap()).await.unwrap();
    assert_eq!(classes(&cached(&sim, &cache)).await, before);
    assert_eq!(scpd_requests(&sim), first);
    let entries = Cache::new(&cache).list().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].device.as_ref().unwrap().firmware, "285.08.30");
    assert_eq!(entries[0].scpds, first);
}

#[tokio::test]
async fn fetches_a_corrupt_file_again() {
    let sim = Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let expected = classes(&cached(&sim, dir.path())).await;
    let first = scpd_requests(&sim);

    let device = fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap();
    let scpd = fs::read_dir(device.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "xml"))
        .unwrap();
    fs::write(&scpd, "<scpd><actionList><action>").unwrap();

    assert_eq!(classes(&cached(&sim, dir.path())).await, expected);
    assert_eq!(scpd_requests(&sim), first + 1);
    assert!(tr064::Scpd::parse(&fs::read_to_string(&scpd).unwrap()).is_ok());
}
