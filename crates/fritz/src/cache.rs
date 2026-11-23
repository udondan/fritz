//! `fritz cache`: the cached service descriptions.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Context as _;
use fritz_core::cache::{self, Cache, ENV_CACHE_DIR};
use serde::Serialize;

use crate::status::{Format, json, table};

/// The cache in the directory of `FRITZ_CACHE_DIR` or the platform.
pub(crate) fn open() -> anyhow::Result<Cache> {
    let dir = cache::default_dir()
        .with_context(|| format!("cannot find the home directory, set {ENV_CACHE_DIR}"))?;
    Ok(Cache::new(dir))
}

/// The cache as JSON.
#[derive(Debug, Serialize)]
struct Listing {
    dir: PathBuf,
    devices: Vec<cache::Entry>,
}

/// Lists the cached devices and the size of their files.
pub(crate) fn show(cache: &Cache, format: Format, out: &mut impl Write) -> anyhow::Result<()> {
    let dir = cache.dir();
    let devices = cache
        .list()
        .with_context(|| format!("cannot read {}", dir.display()))?;
    match format {
        Format::Json => json(
            &Listing {
                dir: dir.to_owned(),
                devices,
            },
            out,
        ),
        Format::Text if devices.is_empty() => {
            writeln!(out, "the cache in {} is empty", dir.display())?;
            Ok(())
        }
        Format::Text => {
            let unknown = || "?".to_owned();
            table(
                &["MODEL", "FIRMWARE", "DEVICE", "SCPDS", "SIZE"],
                devices.iter().map(|entry| {
                    let device = entry.device.as_ref();
                    vec![
                        device.and_then(|d| d.model.clone()).unwrap_or_else(unknown),
                        device.map_or_else(unknown, |d| d.firmware.clone()),
                        device.map_or_else(unknown, |d| d.id.clone()),
                        entry.scpds.to_string(),
                        format!("{} KiB", entry.bytes.div_ceil(1024)),
                    ]
                }),
                out,
            )?;
            writeln!(out, "in {}", dir.display())?;
            Ok(())
        }
    }
}

/// Removes the cached descriptions.
pub(crate) fn clear(cache: &Cache, out: &mut impl Write) -> anyhow::Result<()> {
    let dir = cache.dir();
    let count = cache
        .clear()
        .with_context(|| format!("cannot clear {}", dir.display()))?;
    let devices = if count == 1 { "device" } else { "devices" };
    writeln!(
        out,
        "removed the descriptions of {count} {devices} from {}",
        dir.display()
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io;

    use tr064::{DescriptionCache as _, Scpd};

    use super::*;

    /// Output that cannot be written, like a closed pipe.
    struct Closed;

    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn reports_closed_output() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let description = "<root><systemVersion><Display>1</Display></systemVersion>\
            <device><UDN>uuid:1</UDN></device></root>";
        let scpd = "<scpd/>";
        cache.store(description, "/a.xml", scpd, &Scpd::parse(scpd).unwrap());

        assert!(show(&cache, Format::Text, &mut Closed).is_err());
        let mut out = Vec::new();
        show(&cache, Format::Text, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains("\n?      1         uuid:1  1      1 KiB\n"),
            "{out}"
        );
        assert!(clear(&cache, &mut Closed).is_err());
        Closed.flush().unwrap();
    }
}
