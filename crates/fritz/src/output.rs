//! How commands print their results: as text, JSON or YAML.
//!
//! JSON and YAML quote the strings from the device, so that they cannot
//! change the structure of the document.

use std::io::Write;

use fritz_core::generic;
use serde::Serialize;
use serde_json::{Value, json};

/// How to print the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Format {
    Text,
    Json,
    Yaml,
}

impl Format {
    /// Text for a terminal, else JSON.
    pub(crate) fn for_terminal(terminal: bool) -> Self {
        if terminal { Self::Text } else { Self::Json }
    }
}

/// Prints `value` as JSON or YAML, or as text with `text`. Commands print
/// their results with this, so that every format works for all of them.
pub(crate) fn show<T, W>(
    value: &T,
    format: Format,
    out: &mut W,
    text: impl FnOnce(&T, &mut W) -> anyhow::Result<()>,
) -> anyhow::Result<()>
where
    T: Serialize + ?Sized,
    W: Write,
{
    match format {
        Format::Text => text(value, out),
        Format::Json => json(value, out),
        Format::Yaml => yaml(value, out),
    }
}

/// Prints one sample of a command that keeps printing, like a watch: JSON
/// as one line per sample, YAML as one document per sample, each after a
/// line `---`, or text with `text`.
pub(crate) fn sample<T, W>(
    value: &T,
    format: Format,
    out: &mut W,
    text: impl FnOnce(&T, &mut W) -> anyhow::Result<()>,
) -> anyhow::Result<()>
where
    T: Serialize + ?Sized,
    W: Write,
{
    match format {
        Format::Text => text(value, out)?,
        Format::Json => {
            serde_json::to_writer(&mut *out, value)?;
            writeln!(out)?;
        }
        Format::Yaml => {
            writeln!(out, "---")?;
            yaml(value, out)?;
        }
    }
    out.flush()?;
    Ok(())
}

fn json(value: &(impl Serialize + ?Sized), out: &mut impl Write) -> anyhow::Result<()> {
    serde_json::to_writer_pretty(&mut *out, value)?;
    writeln!(out)?;
    Ok(())
}

pub(crate) fn yaml(value: &(impl Serialize + ?Sized), out: &mut impl Write) -> anyhow::Result<()> {
    serde_saphyr::to_io_writer(out, &value)?;
    Ok(())
}

/// An error for machines: its message, and for faults of the device the
/// status, like `denied`, and the seconds to wait before trying again.
pub(crate) fn failure(error: &anyhow::Error) -> Value {
    let mut body = json!({ "error": format!("{error:#}") });
    let fault = error.chain().find_map(|e| match e.downcast_ref() {
        Some(generic::Error::Fault { fault, .. }) => Some(*fault),
        _ => None,
    });
    if let Some(fault) = fault {
        body["status"] = Value::from(fault.status());
        if let Some(retry) = fault.retry_after() {
            body["retry_after"] = Value::from(retry.as_secs());
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::pairs;
    use crate::status::tests::text;

    #[test]
    fn prints_text_only_to_terminals() {
        assert_eq!(Format::for_terminal(true), Format::Text);
        assert_eq!(Format::for_terminal(false), Format::Json);
    }

    /// Host names that would change the structure of a YAML document if
    /// they were not quoted.
    const TRICKY: [&str; 22] = [
        "a: b",
        "line\nbreak",
        "# comment",
        "name # comment",
        "- item",
        "-",
        "? key",
        "[list]",
        "{map}",
        "&anchor",
        "*alias",
        "!tag",
        "|",
        ">",
        "'single'",
        "\"double\"",
        "%directive",
        "@at",
        "`tick`",
        "---",
        "  spaces  ",
        "tab\there",
    ];

    /// Strings that a YAML parser reads as something else than a string
    /// if they were not quoted.
    const TYPED: [&str; 14] = [
        "",
        "null",
        "~",
        "true",
        "no",
        "yes",
        "on",
        "off",
        "Y",
        "1",
        "0x10",
        "1e3",
        ".inf",
        "2026-11-23",
    ];

    fn round_trip(value: &Value) -> Value {
        let yaml = text(|out| yaml(value, out));
        serde_saphyr::from_str(&yaml).unwrap_or_else(|e| panic!("{e}: {yaml}"))
    }

    #[test]
    fn quotes_strings_in_yaml() {
        for name in TRICKY.iter().chain(&TYPED) {
            let hosts = json!({ "hosts": [{ "name": name, "ip": "192.0.2.1" }] });
            assert_eq!(round_trip(&hosts), hosts, "{name:?}");
            let keys = json!({ *name: name });
            assert_eq!(round_trip(&keys), keys, "{name:?}");
        }
        // Quoted for parsers of YAML 1.1 as well, which read `no` as false.
        for name in TYPED {
            let yaml = text(|out| yaml(&json!({ "name": name }), out));
            let value = yaml.strip_prefix("name: ").unwrap().trim_end();
            assert!(
                value.starts_with(['"', '\'']) && value.len() == name.len() + 2,
                "{yaml}"
            );
        }
        let numbers = json!({ "count": 37, "big": u64::MAX, "rate": 7.0, "negative": -3,
                              "none": null, "on": true, "empty": [], "nested": {} });
        assert_eq!(round_trip(&numbers), numbers);
        let yaml = text(|out| yaml(&json!({ "name": "a: b" }), out));
        assert_eq!(yaml, "name: \"a: b\"\n");
    }

    #[test]
    fn prints_the_formats() {
        let value = json!({ "name": "device-1", "active": true });
        let print = |format| {
            text(|out| {
                show(&value, format, out, |v, out| {
                    pairs(&[("name", v["name"].as_str().map(str::to_owned))], out)
                })
            })
        };
        assert_eq!(print(Format::Text), "name  device-1\n");
        assert_eq!(
            print(Format::Json),
            "{\n  \"active\": true,\n  \"name\": \"device-1\"\n}\n"
        );
        assert_eq!(print(Format::Yaml), "active: true\nname: device-1\n");
        let list = text(|out| yaml(&[json!({ "ssid": "wlan-1", "channel": 6 })], out));
        assert_eq!(list, "- channel: 6\n  ssid: wlan-1\n");
    }

    #[test]
    fn prints_samples() {
        let samples = [
            json!({ "rx": 1, "name": "a: b" }),
            json!({ "rx": 2, "name": "---" }),
        ];
        let print = |format| {
            text(|out| {
                for value in &samples {
                    sample(value, format, out, |v, out| {
                        Ok(writeln!(out, "{}", v["rx"])?)
                    })?;
                }
                Ok(())
            })
        };
        assert_eq!(print(Format::Text), "1\n2\n");
        let json = print(Format::Json);
        let lines: Vec<Value> = json
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines, samples);
        let yaml = print(Format::Yaml);
        assert!(yaml.starts_with("---\n"), "{yaml}");
        assert_eq!(yaml.matches("\n---\n").count(), 1, "{yaml}");
        let documents: Vec<Value> = serde_saphyr::from_multiple(&yaml).unwrap();
        assert_eq!(documents, samples);
    }

    #[test]
    fn describes_errors_for_machines() {
        let error = anyhow::anyhow!("cannot read").context("cannot read the overview");
        assert_eq!(
            failure(&error),
            json!({ "error": "cannot read the overview: cannot read" })
        );
        let fault = generic::Error::Fault {
            fault: fritz_core::fault::Fault::Blocked,
            description: "GetInfo".to_owned(),
        };
        let error = anyhow::Error::new(fault).context("cannot read the overview");
        let body = failure(&error);
        assert_eq!(body["status"], "blocked");
        assert_eq!(body["retry_after"], 3600);
        let fault = generic::Error::Fault {
            fault: fritz_core::fault::Fault::Denied,
            description: "GetInfo".to_owned(),
        };
        let body = failure(&anyhow::Error::new(fault));
        assert_eq!(body["status"], "denied");
        assert!(body.get("retry_after").is_none());
        let yaml = text(|out| yaml(&body, out));
        assert_eq!(serde_saphyr::from_str::<Value>(&yaml).unwrap(), body);
    }
}
