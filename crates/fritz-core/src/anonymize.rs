//! Turning raw captures into fixtures that can be published.
//!
//! [`anonymize`] replaces personal values with placeholders: addresses,
//! UUIDs, e-mail addresses and MyFRITZ! names wherever they appear, and the
//! values of arguments and XML elements whose names point to personal data,
//! like host names, WLAN names and phone numbers. The same value always gets
//! the same placeholder, so the answers stay consistent with each other. The
//! mapping is never stored.
//!
//! Before anything is written, the result is searched for every replaced
//! value and for private patterns, like one's own name or street. If any of
//! them is still there, nothing is written. [`check`] searches existing
//! fixtures for values the anonymizer would replace and for private
//! patterns, and reports where it found them, but never the values.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

mod addresses;
mod leftovers;
mod mapper;
mod rules;

use mapper::Mapper;

/// A kind of personal value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Mac,
    Ipv4,
    Ipv6,
    Uuid,
    Email,
    MyFritz,
    Device,
    Wlan,
    Name,
    User,
    Phone,
    Provider,
    Server,
    Url,
    Id,
    Serial,
    Log,
    /// A line of the private patterns.
    Private,
}

impl Kind {
    /// A short name like `MAC address`.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Mac => "MAC address",
            Kind::Ipv4 => "IPv4 address",
            Kind::Ipv6 => "IPv6 address",
            Kind::Uuid => "UUID",
            Kind::Email => "e-mail address",
            Kind::MyFritz => "MyFRITZ! name",
            Kind::Device => "device name",
            Kind::Wlan => "WLAN name",
            Kind::Name => "name",
            Kind::User => "user name",
            Kind::Phone => "phone number",
            Kind::Provider => "provider name",
            Kind::Server => "server name",
            Kind::Url => "URL",
            Kind::Id => "identifier",
            Kind::Serial => "serial number",
            Kind::Log => "event log",
            Kind::Private => "private pattern",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A file that contains a value of a kind it must not contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The path of the file, relative to the directory searched.
    pub file: PathBuf,
    pub kind: Kind,
}

impl fmt::Display for Finding {
    /// A line like `responses/Hosts1/GetGenericHostEntry@0.json: MAC address`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file.display(), self.kind)
    }
}

/// Why anonymizing or checking failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read or write {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} is not valid JSON", path.display())]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// Replaced values or private patterns are still in the result, which
    /// was therefore not written.
    #[error("{} personal values remain after anonymizing", .0.len())]
    Leftovers(Vec<Finding>),
}

/// Anonymizes the raw capture in `raw` into the new directory `out` and
/// returns the number of files written. Lines of `private` are searched for
/// in the result, regardless of case.
///
/// # Errors
///
/// [`Error::Leftovers`] when replaced values or private patterns remain,
/// and the errors of reading, parsing and writing, including when `out`
/// exists. Nothing is written unless the result is clean.
pub fn anonymize(raw: &Path, out: &Path, private: &[String]) -> Result<usize, Error> {
    let files = read_tree(raw)?;
    let mut mapper = Mapper::new(descriptions(&files, Path::new("")));
    let files = files
        .into_iter()
        .map(|(file, text)| {
            let anonymized = anonymize_file(&mut mapper, &file, &text, raw)?;
            Ok((file, anonymized))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let needles = leftovers::needles(&mapper.replaced, private);
    let findings = search(&files, &needles);
    if !findings.is_empty() {
        return Err(Error::Leftovers(findings));
    }
    let io_error = |path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io { path, source }
    };
    let parent = out.parent().unwrap_or(out);
    fs::create_dir_all(parent).map_err(io_error(parent))?;
    fs::create_dir(out).map_err(io_error(out))?;
    for (file, text) in &files {
        let path = out.join(file);
        let parent = path.parent().unwrap_or(out);
        fs::create_dir_all(parent).map_err(io_error(parent))?;
        fs::write(&path, text).map_err(io_error(&path))?;
    }
    Ok(files.len())
}

/// Searches the files in `dir` for values the anonymizer would replace and
/// for lines of `private`, and returns where it found them. `dir` can be
/// one fixture or a directory with several, like `fixtures/<model>/<fw>/`.
///
/// # Errors
///
/// The errors of reading and parsing the files.
pub fn check(dir: &Path, private: &[String]) -> Result<Vec<Finding>, Error> {
    let needles = leftovers::needles(&[], private);
    let mut findings = Vec::new();
    let files = read_tree(dir)?;
    let mut protocols = HashMap::new();
    for (file, text) in &files {
        let root = fixture_root(file);
        let protocol = protocols
            .entry(root)
            .or_insert_with(|| Mapper::new(descriptions(&files, root)));
        let mut mapper = protocol.fresh();
        anonymize_file(&mut mapper, file, text, dir)?;
        let mut kinds: Vec<Kind> = mapper.replaced.iter().map(|(kind, _)| *kind).collect();
        kinds.extend(leftovers::find(text, &needles));
        kinds.sort_unstable();
        kinds.dedup();
        findings.extend(kinds.into_iter().map(|kind| Finding {
            file: file.clone(),
            kind,
        }));
    }
    Ok(findings)
}

/// Reads private patterns from a file: one text per line, without empty
/// lines and comments starting with `#`.
///
/// # Errors
///
/// The errors of reading the file.
pub fn read_private_patterns(path: &Path) -> io::Result<Vec<String>> {
    Ok(fs::read_to_string(path)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect())
}

/// The texts of the service descriptions of the fixture at `root`.
fn descriptions<'a>(
    files: &'a [(PathBuf, String)],
    root: &'a Path,
) -> impl Iterator<Item = &'a str> {
    files
        .iter()
        .filter(move |(file, _)| is_description(file) && fixture_root(file) == root)
        .map(|(_, text)| text.as_str())
}

/// Whether a file is a service description, which lies in `scpd/`.
fn is_description(file: &Path) -> bool {
    file.parent().is_some_and(|dir| dir.ends_with("scpd"))
}

/// The directory of the fixture or capture a file belongs to: the one
/// above its `scpd/` or `responses/` directory, else its own directory.
fn fixture_root(file: &Path) -> &Path {
    file.ancestors()
        .skip(1)
        .find(|dir| dir.ends_with("scpd") || dir.ends_with("responses"))
        .unwrap_or(file)
        .parent()
        .unwrap_or(file)
}

/// Where the needles are in the files.
fn search(files: &[(PathBuf, String)], needles: &[leftovers::Needle]) -> Vec<Finding> {
    files
        .iter()
        .flat_map(|(file, text)| {
            leftovers::find(text, needles)
                .into_iter()
                .map(|kind| Finding {
                    file: file.clone(),
                    kind,
                })
        })
        .collect()
}

/// Anonymizes one file of a capture, depending on what it is: the service
/// descriptions under `scpd/` only describe actions, so only values
/// anywhere are replaced there. Answers in JSON are replaced by argument
/// name, and other XML documents by element name.
fn anonymize_file(
    mapper: &mut Mapper,
    file: &Path,
    text: &str,
    dir: &Path,
) -> Result<String, Error> {
    if is_description(file) {
        return Ok(mapper.text(text));
    }
    match file.extension().and_then(|e| e.to_str()) {
        Some("json") => {
            let json_error = |source| Error::Json {
                path: dir.join(file),
                source,
            };
            let mut value: Value = serde_json::from_str(text).map_err(json_error)?;
            anonymize_json(mapper, &mut value, None);
            let mut text = serde_json::to_string_pretty(&value).map_err(json_error)?;
            text.push('\n');
            Ok(text)
        }
        Some("xml") => Ok(mapper.xml(text)),
        _ => Ok(mapper.text(text)),
    }
}

/// Replaces the strings in a JSON value. `name` is the key of the value in
/// its object.
fn anonymize_json(mapper: &mut Mapper, value: &mut Value, name: Option<&str>) {
    match value {
        Value::String(text) => {
            *text = match name {
                Some(name) => mapper.argument(name, text),
                None => mapper.text(text),
            };
        }
        Value::Array(items) => {
            for item in items {
                anonymize_json(mapper, item, None);
            }
        }
        Value::Object(entries) => {
            for (key, item) in entries.iter_mut() {
                anonymize_json(mapper, item, Some(key));
            }
        }
        _ => {}
    }
}

/// The text files below `dir`, by path relative to it, sorted. Hidden files
/// like `.DS_Store` are left out.
fn read_tree(dir: &Path) -> Result<Vec<(PathBuf, String)>, Error> {
    let mut files = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        // Collecting the components drops the trailing slash of `dir/`.
        let path: PathBuf = dir.join(&relative).components().collect();
        let io_error = |source| Error::Io {
            path: path.clone(),
            source,
        };
        if path.is_dir() {
            for entry in fs::read_dir(&path).map_err(io_error)? {
                let name = entry.map_err(io_error)?.file_name();
                if !name.to_string_lossy().starts_with('.') {
                    pending.push(relative.join(name));
                }
            }
        } else {
            files.push((relative, fs::read_to_string(&path).map_err(io_error)?));
        }
    }
    files.sort();
    Ok(files)
}
