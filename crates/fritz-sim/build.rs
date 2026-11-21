//! Embeds the fixtures of `fixtures/`, so that `Fixture::bundled` works
//! without the source of the crate, like after `cargo install`.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=fixtures");
    let root = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("fixtures");
    let mut files = Vec::new();
    walk(&root, "", &mut files);
    files.sort();
    let mut code = String::from("&[\n");
    for (name, path) in files {
        // Debug quotes and escapes both as string literals.
        let path = path.to_str().expect("fixture paths are UTF-8");
        writeln!(code, "    ({name:?}, include_str!({path:?})),").unwrap();
    }
    code.push_str("]\n");
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("bundled.rs");
    fs::write(out, code).unwrap();
}

/// Adds the files below `dir` to `files`, by their path relative to the
/// fixtures with `/` between the parts, without hidden ones like `.DS_Store`.
fn walk(dir: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let relative = format!("{prefix}{name}");
        if entry.file_type().unwrap().is_dir() {
            walk(&entry.path(), &format!("{relative}/"), files);
        } else {
            files.push((relative, entry.path()));
        }
    }
}
