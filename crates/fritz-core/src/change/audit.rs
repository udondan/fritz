//! The audit log: a JSON line for every planned, applied and refused change,
//! with the values of secrets hidden.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::Error;
use crate::identity::Identity;

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Event {
    /// A change was planned and a token issued.
    Planned,
    /// A change is about to be sent to the device.
    Sending,
    /// The device accepted the change.
    Applied,
    /// The change was not planned or not sent.
    Refused,
    /// The device did not accept the change, or the connection failed.
    Failed,
}

/// A line of the log.
#[derive(Debug, Serialize)]
pub(crate) struct Entry<'a> {
    /// Seconds since the Unix epoch.
    pub(crate) time: u64,
    /// Where the change came from, like `cli` or `mcp`.
    pub(crate) source: &'a str,
    pub(crate) event: Event,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) device: Option<&'a Identity>,
    pub(crate) service: &'a str,
    pub(crate) action: &'a str,
    /// The arguments with secrets hidden.
    pub(crate) arguments: &'a [(String, String)],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) verified: Option<bool>,
}

/// The audit log in a file.
#[derive(Debug, Clone)]
pub(crate) struct Audit {
    path: PathBuf,
}

impl Audit {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Appends an entry.
    pub(crate) fn append(&self, entry: &Entry<'_>) -> Result<(), Error> {
        self.write(entry).map_err(|source| Error::State {
            path: self.path.clone(),
            source,
        })
    }

    fn write(&self, entry: &Entry<'_>) -> io::Result<()> {
        let mut line = serde_json::to_vec(entry)?;
        line.push(b'\n');
        std::fs::create_dir_all(self.path.parent().unwrap_or(Path::new("")))?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(&line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_json_lines() {
        let dir = tempfile::tempdir().unwrap();
        let audit = Audit::new(dir.path().join("state/audit.jsonl"));
        let device = Identity::of("uuid:1", "1");
        let arguments = [("NewEnable".to_owned(), "0".to_owned())];
        let mut entry = Entry {
            time: 1,
            source: "cli",
            event: Event::Planned,
            device: Some(&device),
            service: "WLANConfiguration1",
            action: "SetEnable",
            arguments: &arguments,
            error: None,
            verified: None,
        };
        audit.append(&entry).unwrap();
        entry.event = Event::Refused;
        entry.device = None;
        entry.error = Some("no".to_owned());
        audit.append(&entry).unwrap();
        let text = std::fs::read_to_string(dir.path().join("state/audit.jsonl")).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["event"], "planned");
        assert_eq!(lines[0]["device"], device.to_string());
        assert_eq!(lines[0]["arguments"][0][1], "0");
        assert_eq!(lines[1]["event"], "refused");
        assert_eq!(lines[1]["error"], "no");
        assert!(lines[1].get("device").is_none());
    }

    #[test]
    fn reports_unwritable_files() {
        let dir = tempfile::tempdir().unwrap();
        let error = Audit::new(dir.path().to_owned())
            .append(&Entry {
                time: 1,
                source: "cli",
                event: Event::Failed,
                device: None,
                service: "",
                action: "",
                arguments: &[],
                error: None,
                verified: None,
            })
            .unwrap_err();
        assert!(matches!(error, Error::State { .. }), "{error:?}");
    }
}
