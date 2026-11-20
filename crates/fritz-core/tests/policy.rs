//! The class of every action of every fixture, kept in `tests/data/policy.txt`
//! so that each change of the policy shows in review.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use fritz_core::policy;
use tr064::{DeviceDescription, Scpd};

/// The fixture directories, like `fixtures/fritzbox-7690/8.25`.
fn fixtures(root: &Path) -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    for model in fs::read_dir(root).unwrap() {
        for version in fs::read_dir(model.unwrap().path()).unwrap() {
            dirs.push(version.unwrap().path());
        }
    }
    dirs.sort();
    dirs
}

#[test]
fn classifies_every_action_of_the_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    // The class depends on the service name and the action, so services
    // with the same name in several fixtures or versions give one line.
    let mut lines = BTreeSet::new();
    for dir in fixtures(&root) {
        let xml = fs::read_to_string(dir.join("tr64desc.xml")).unwrap();
        let description = DeviceDescription::parse(&xml).unwrap();
        for service in description.services() {
            let file = service.scpd_url.rsplit('/').next().unwrap();
            let scpd = Scpd::parse(&fs::read_to_string(dir.join("scpd").join(file)).unwrap());
            for action in scpd.unwrap().actions {
                let class = policy::classify(service, &action);
                lines.insert(format!("{} {} {class}", service.name(), action.name));
            }
        }
    }
    let mut actual = lines.into_iter().collect::<Vec<_>>().join("\n");
    actual.push('\n');
    let written = Path::new(env!("CARGO_TARGET_TMPDIR")).join("policy.txt");
    fs::write(&written, &actual).unwrap();
    let expected = include_str!("data/policy.txt");
    assert!(
        actual == expected,
        "the classes changed, review {} and copy it to tests/data/policy.txt",
        written.display()
    );
}
