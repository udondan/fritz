//! Fixtures: devices recorded with `fritz capture` and anonymized.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tr064::{DeviceDescription, Scpd};

/// Why a fixture cannot be loaded.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} is not valid: {reason}", path.display())]
    Invalid { path: PathBuf, reason: String },
}

/// A recorded answer to an action call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The output arguments by name.
    Ok(BTreeMap<String, String>),
    /// A UPnP error.
    Fault { code: u16, description: String },
}

/// The recorded answers to one action: to a call without index, or to the
/// calls with each index.
#[derive(Debug, Clone, Default)]
pub(crate) struct Recording {
    pub(crate) once: Option<Answer>,
    pub(crate) indexed: BTreeMap<u32, Answer>,
}

/// A device as recorded in a fixture directory:
///
/// ```text
/// tr64desc.xml
/// scpd/<file>.xml
/// responses/<service>/<action>[@<index>].json
/// lists/<name>.<xml|json>
/// api/<path>.json
/// ```
///
/// A list document `lists/devicehostlist.xml` is served at the path
/// `/devicehostlist.lua` that an action like `X_AVM-DE_GetHostListPath`
/// returns. A document `api/v0/smarthome/overview.json` of the REST API is
/// served at `/api/v0/smarthome/overview`.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub(crate) description_xml: String,
    pub(crate) description: DeviceDescription,
    /// The service descriptions by file name, like `hostsSCPD.xml`.
    pub(crate) scpds: HashMap<String, (String, Scpd)>,
    /// The recorded answers by service, like `Hosts1`, and action.
    pub(crate) recordings: HashMap<(String, String), Recording>,
    /// The list documents by name, like `devicehostlist`.
    pub(crate) lists: HashMap<String, String>,
    /// The documents of the REST API by path, like
    /// `/api/v0/smarthome/overview`.
    pub(crate) api: HashMap<String, String>,
}

impl Fixture {
    /// Loads the fixture in `dir`.
    ///
    /// # Errors
    ///
    /// The errors of reading the files, and [`Error::Invalid`] if one of
    /// them cannot be parsed.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self, Error> {
        let dir = dir.as_ref();
        let path = dir.join("tr64desc.xml");
        let description_xml = read(&path)?;
        let description = DeviceDescription::parse(&description_xml).map_err(invalid(&path))?;
        let mut scpds = HashMap::new();
        for (name, path) in entries(&dir.join("scpd"))? {
            let xml = read(&path)?;
            let scpd = Scpd::parse(&xml).map_err(invalid(&path))?;
            scpds.insert(name, (xml, scpd));
        }
        let mut recordings: HashMap<_, Recording> = HashMap::new();
        for (service, service_dir) in entries(&dir.join("responses"))? {
            for (name, path) in entries(&service_dir)? {
                let Some(stem) = name.strip_suffix(".json") else {
                    continue;
                };
                let (action, index) = match stem.split_once('@') {
                    Some((action, index)) => {
                        let index = index.parse().map_err(invalid(&path))?;
                        (action, Some(index))
                    }
                    None => (stem, None),
                };
                let Some(answer) = answer(&read(&path)?).map_err(invalid(&path))? else {
                    continue;
                };
                let recording = recordings
                    .entry((service.clone(), action.to_owned()))
                    .or_default();
                match index {
                    Some(index) => {
                        recording.indexed.insert(index, answer);
                    }
                    None => recording.once = Some(answer),
                }
            }
        }
        let mut lists = HashMap::new();
        let lists_dir = dir.join("lists");
        if lists_dir.is_dir() {
            for (name, path) in entries(&lists_dir)? {
                let name = name.split_once('.').map_or(name.as_str(), |(stem, _)| stem);
                lists.insert(name.to_owned(), read(&path)?);
            }
        }
        let mut api = HashMap::new();
        let api_dir = dir.join("api");
        if api_dir.is_dir() {
            documents(&api_dir, "/api", &mut api)?;
        }
        Ok(Self {
            description_xml,
            description,
            scpds,
            recordings,
            lists,
            api,
        })
    }

    /// The device description.
    pub fn description(&self) -> &DeviceDescription {
        &self.description
    }

    /// The recorded answer to a call of `action` of the service `service`,
    /// like `Hosts1`, with the given index or without one.
    pub fn answer(&self, service: &str, action: &str, index: Option<u32>) -> Option<&Answer> {
        let recording = self
            .recordings
            .get(&(service.to_owned(), action.to_owned()))?;
        match index {
            Some(index) => recording.indexed.get(&index),
            None => recording.once.as_ref(),
        }
    }

    /// Records `answer` for calls of `action` of the service `service`,
    /// like `Hosts1`, with the given index or without one, in place of a
    /// recorded answer. For tests of answers the fixture lacks.
    #[must_use]
    pub fn with_answer(
        mut self,
        service: &str,
        action: &str,
        index: Option<u32>,
        answer: Answer,
    ) -> Self {
        let recording = self
            .recordings
            .entry((service.to_owned(), action.to_owned()))
            .or_default();
        match index {
            Some(index) => {
                recording.indexed.insert(index, answer);
            }
            None => recording.once = Some(answer),
        }
        self
    }
}

/// Parses a recorded answer: `{"status": "ok", "out": {…}}` or
/// `{"status": "fault", "code": 714, "description": "…"}`. Calls that failed
/// without an answer, `{"status": "failed", …}`, count as not recorded.
fn answer(json: &str) -> Result<Option<Answer>, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    match value["status"].as_str() {
        Some("ok") => {
            let out = value["out"].as_object().ok_or("no output arguments")?;
            out.iter()
                .map(|(name, value)| {
                    let value = value.as_str().ok_or(format!("{name} is not a string"))?;
                    Ok((name.clone(), value.to_owned()))
                })
                .collect::<Result<_, String>>()
                .map(|out| Some(Answer::Ok(out)))
        }
        Some("fault") => {
            let code = value["code"].as_u64().and_then(|c| u16::try_from(c).ok());
            Ok(Some(Answer::Fault {
                code: code.ok_or("no error code")?,
                description: value["description"].as_str().unwrap_or_default().to_owned(),
            }))
        }
        Some("failed") => Ok(None),
        _ => Err("unknown status".to_owned()),
    }
}

/// Adds the JSON documents in `dir` and its subdirectories to `found`, by
/// their path under `prefix` without the extension.
fn documents(dir: &Path, prefix: &str, found: &mut HashMap<String, String>) -> Result<(), Error> {
    for (name, path) in entries(dir)? {
        if path.is_dir() {
            documents(&path, &format!("{prefix}/{name}"), found)?;
        } else if let Some(stem) = name.strip_suffix(".json") {
            found.insert(format!("{prefix}/{stem}"), read(&path)?);
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })
}

/// The entries of a directory by name, without hidden ones like
/// `.DS_Store`.
fn entries(dir: &Path) -> Result<Vec<(String, PathBuf)>, Error> {
    let io_error = |source| Error::Io {
        path: dir.to_owned(),
        source,
    };
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') {
            entries.push((name, entry.path()));
        }
    }
    Ok(entries)
}

fn invalid<E: ToString>(path: &Path) -> impl FnOnce(E) -> Error {
    let path = path.to_owned();
    move |error| Error::Invalid {
        path,
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_answers() {
        assert_eq!(
            answer(r#"{"status": "ok", "out": {"NewEnable": "1"}}"#),
            Ok(Some(Answer::Ok(BTreeMap::from([(
                "NewEnable".to_owned(),
                "1".to_owned()
            )]))))
        );
        assert_eq!(
            answer(r#"{"status": "fault", "code": 714}"#),
            Ok(Some(Answer::Fault {
                code: 714,
                description: String::new()
            }))
        );
        assert_eq!(
            answer(r#"{"status": "failed", "error": "timeout"}"#),
            Ok(None)
        );
        for (json, error) in [
            ("{", "EOF while parsing an object at line 1 column 1"),
            (r#"{"status": "ok"}"#, "no output arguments"),
            (
                r#"{"status": "ok", "out": {"NewEnable": 1}}"#,
                "NewEnable is not a string",
            ),
            (r#"{"status": "fault", "code": 70000}"#, "no error code"),
            (r#"{"status": "skipped"}"#, "unknown status"),
        ] {
            assert_eq!(answer(json), Err(error.to_owned()), "{json}");
        }
    }
}
