//! A capture of the simulator gives the fixture it serves.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fritz_core::capture::{self, Options};
use fritz_sim::{Fixture, Sim};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

/// The fixture directories, like `fritzbox-7690/8.25`.
fn fixtures() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for model in fs::read_dir(FIXTURES).unwrap() {
        for version in fs::read_dir(model.unwrap().path()).unwrap() {
            dirs.push(version.unwrap().path());
        }
    }
    assert_ne!(dirs, [] as [PathBuf; 0], "no fixtures in {FIXTURES}");
    dirs
}

/// The files below `dir` with their content, by relative path.
fn files(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        } else {
            let relative = path.strip_prefix(dir).unwrap().to_owned();
            files.push((relative, fs::read_to_string(&path).unwrap()));
        }
    }
    files.sort();
    files
}

#[tokio::test]
async fn capturing_the_simulator_gives_the_fixture() {
    for dir in fixtures() {
        let sim = Sim::start(Fixture::load(&dir).unwrap()).await.unwrap();
        let client = sim.client().build().unwrap();
        let plan = capture::plan(&client).await.unwrap();
        let options = Options {
            delay: Duration::ZERO,
            ..Options::default()
        };
        let capture = capture::run(&client, &plan, &options, |_| {}).await;
        assert!(capture.stopped.is_none(), "{:?}", capture.stopped);

        let out = tempfile::tempdir().unwrap();
        capture::write(&plan, &capture, out.path()).unwrap();
        let (expected, actual) = (files(&dir), files(out.path()));
        let names = |files: &[(PathBuf, String)]| -> Vec<PathBuf> {
            files.iter().map(|(name, _)| name.clone()).collect()
        };
        assert_eq!(names(&actual), names(&expected), "{}", dir.display());
        for ((name, actual), (_, expected)) in actual.iter().zip(&expected) {
            assert_eq!(actual, expected, "{}", dir.join(name).display());
        }
    }
}
