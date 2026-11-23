//! Build details for `fritz version`: the commit, the date and the target.
//!
//! The commit comes from `.cargo_vcs_info.json`, which `cargo package` adds
//! to the crates on crates.io, else from git. Builds without either have no
//! commit. `SOURCE_DATE_EPOCH` sets the date for reproducible builds.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=.cargo_vcs_info.json");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let commit = std::fs::read_to_string(".cargo_vcs_info.json")
        .ok()
        .and_then(|info| sha1(&info))
        .or_else(git_commit);
    if let Some(commit) = commit {
        println!("cargo:rustc-env=FRITZ_BUILD_COMMIT={commit}");
    }
    let seconds = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        });
    println!("cargo:rustc-env=FRITZ_BUILD_DATE={}", date(seconds));
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=FRITZ_BUILD_TARGET={target}");
}

/// The commit in `.cargo_vcs_info.json`, like `"sha1": "0123abc…"`.
fn sha1(info: &str) -> Option<String> {
    let rest = &info[info.find("\"sha1\"")? + "\"sha1\"".len()..];
    let rest = &rest[rest.find('"')? + 1..];
    let sha1 = &rest[..rest.find('"')?];
    let short = sha1.get(..12)?;
    short
        .chars()
        .all(|c| c.is_ascii_hexdigit())
        .then(|| short.to_owned())
}

/// The commit of the git checkout, and rebuilds when it changes.
fn git_commit() -> Option<String> {
    let commit = git(&["rev-parse", "--short=12", "HEAD"])?;
    let mut watched = vec![git(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "HEAD",
    ])];
    if let Some(reference) = git(&["rev-parse", "--symbolic-full-name", "HEAD"]) {
        watched.push(git(&[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            &reference,
        ]));
    }
    watched.push(git(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "packed-refs",
    ]));
    // Cargo reruns the script every time for a path that does not exist.
    for path in watched.into_iter().flatten() {
        if Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    Some(commit)
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (output.status.success() && !text.is_empty()).then(|| text.to_owned())
}

/// The date of a Unix time, like `2026-11-23`.
fn date(seconds: u64) -> String {
    // The civil calendar from days since 1970-01-01, after Howard Hinnant's
    // `civil_from_days`.
    let days = seconds / 86_400 + 719_468;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
