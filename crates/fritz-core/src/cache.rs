//! A cache of the service descriptions (SCPDs) of devices, so that a run
//! only fetches the device description.
//!
//! The cache keeps one directory per device and firmware, named after its
//! UDN (or its serial number if it has no UDN) and a hash of its model and
//! firmware version. The file `device.json` in it names the device, its
//! model and its firmware version, as the device description states them;
//! the other files are the SCPDs as the device sent them. Since every
//! firmware has its own directory, a process that still runs with the old
//! firmware never writes into the directory of the new one, and a firmware
//! update never uses old action lists. The directories of other firmware
//! versions of the device are removed when a new one is created. Devices
//! whose description has no firmware version, like some FRITZ!Repeaters, are
//! not cached.
//!
//! A file that cannot be read or parsed counts as missing, and the SCPD is
//! fetched again. The descriptions need no login and hold no secrets;
//! nothing else goes into the cache.
//!
//! Besides the files, the cache keeps the parsed SCPDs in memory, for
//! processes that run long, like the MCP server.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tr064::{DescriptionCache, Scpd};

/// The environment variable for another cache directory.
pub const ENV_CACHE_DIR: &str = "FRITZ_CACHE_DIR";

/// The file in the directory of a device that names it.
const DEVICE_FILE: &str = "device.json";

/// The cache directory: the one of [`ENV_CACHE_DIR`], else the one of the
/// platform (see [`crate::dirs::cache`]). `None` if neither is known.
pub fn default_dir() -> Option<PathBuf> {
    std::env::var_os(ENV_CACHE_DIR)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(crate::dirs::cache)
}

/// A device as its description names it, the key of its cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Device {
    /// The UDN, or the serial number if there is no UDN.
    pub id: String,
    /// The model.
    pub model: Option<String>,
    /// The firmware version, like `285.08.25`.
    pub firmware: String,
}

impl Device {
    /// The device that a device description describes. `None` if the
    /// description cannot be parsed, or names neither a UDN nor a serial
    /// number, or no firmware version.
    pub fn of(description: &str) -> Option<Self> {
        let doc = Document::parse(description).ok()?;
        let root = doc.root_element();
        let device = child(root, "device")?;
        let firmware = child(root, "systemVersion").and_then(|v| text(v, "Display"))?;
        let id = text(device, "UDN").or_else(|| text(device, "serialNumber"))?;
        Some(Self {
            id,
            model: text(device, "modelName"),
            firmware,
        })
    }

    /// The name of its directory: the id with only letters, digits and
    /// dashes, and a hash of the model and the firmware version, so that
    /// every firmware has its own directory.
    fn dir_name(&self) -> String {
        let id: String = self
            .id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let mut hash = Sha256::new();
        hash.update(self.model.as_deref().unwrap_or_default());
        hash.update([0]);
        hash.update(&self.firmware);
        format!("{id}-{}", crate::session::to_hex(&hash.finalize()[..4]))
    }
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
}

/// The text of a child element, if it is not empty.
fn text(node: Node<'_, '_>, name: &str) -> Option<String> {
    let text = child(node, name)?.text()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The cached descriptions of one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// The device, `None` if its file cannot be read.
    pub device: Option<Device>,
    /// The directory.
    pub path: PathBuf,
    /// The number of SCPDs.
    pub scpds: usize,
    /// The size of all files, in bytes.
    pub bytes: u64,
}

/// The cache of the service descriptions in a directory.
#[derive(Debug)]
pub struct Cache {
    dir: PathBuf,
    memory: Mutex<HashMap<(Device, String), Scpd>>,
}

impl Cache {
    /// A cache in `dir`, which is created when the first SCPD is stored.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            memory: Mutex::new(HashMap::new()),
        }
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The cached devices, sorted by their directory.
    ///
    /// # Errors
    ///
    /// The errors of reading the directory, except that it does not exist.
    pub fn list(&self) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for path in self.device_dirs()? {
            let mut scpds = 0;
            let mut bytes = 0;
            for file in fs::read_dir(&path)? {
                let file = file?;
                bytes += file.metadata()?.len();
                if file.file_name() != DEVICE_FILE {
                    scpds += 1;
                }
            }
            entries.push(Entry {
                device: read_device(&path),
                path,
                scpds,
                bytes,
            });
        }
        Ok(entries)
    }

    /// Removes the cached devices, and the directory if nothing else is in
    /// it. Other files are left alone, in case the directory was set to
    /// one that holds more. Returns the number of devices removed.
    ///
    /// # Errors
    ///
    /// The errors of removing the files.
    pub fn clear(&self) -> io::Result<usize> {
        self.lock().clear();
        let dirs = self.device_dirs()?;
        for dir in &dirs {
            fs::remove_dir_all(dir)?;
        }
        // Fails if other files remain, which stay.
        let _ = fs::remove_dir(&self.dir);
        Ok(dirs.len())
    }

    /// The directories of devices: those with a [`DEVICE_FILE`].
    fn device_dirs(&self) -> io::Result<Vec<PathBuf>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut dirs = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.join(DEVICE_FILE).is_file() {
                dirs.push(path);
            }
        }
        dirs.sort();
        Ok(dirs)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(Device, String), Scpd>> {
        self.memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reads an SCPD from the directory of the device, if it is there and
    /// was stored for the same device.
    fn read(&self, device: &Device, url: &str) -> Option<Scpd> {
        let dir = self.dir.join(device.dir_name());
        if read_device(&dir).as_ref() != Some(device) {
            return None;
        }
        Scpd::parse(&fs::read_to_string(dir.join(file_name(url))).ok()?).ok()
    }

    /// Writes an SCPD to the directory of the device and firmware. A new
    /// directory replaces the ones of other firmware versions of the device,
    /// and one that names another device is replaced.
    fn write(&self, device: &Device, url: &str, xml: &str) -> io::Result<()> {
        let dir = self.dir.join(device.dir_name());
        if read_device(&dir).as_ref() != Some(device) {
            for old in self.device_dirs()? {
                if old != dir && read_device(&old).is_some_and(|old| old.id == device.id) {
                    // A process with the old firmware may still write there;
                    // then its next run removes it.
                    let _ = fs::remove_dir_all(&old);
                }
            }
            match fs::remove_dir_all(&dir) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            fs::create_dir_all(&dir)?;
            write_atomic(&dir.join(DEVICE_FILE), &serde_json::to_vec(device)?)?;
        }
        write_atomic(&dir.join(file_name(url)), xml.as_bytes())
    }
}

impl DescriptionCache for Cache {
    fn scpd(&self, description: &str, url: &str) -> Option<Scpd> {
        let key = (Device::of(description)?, url.to_owned());
        if let Some(scpd) = self.lock().get(&key) {
            return Some(scpd.clone());
        }
        let scpd = self.read(&key.0, url)?;
        self.lock().insert(key, scpd.clone());
        Some(scpd)
    }

    fn store(&self, description: &str, url: &str, xml: &str, scpd: &Scpd) {
        let Some(device) = Device::of(description) else {
            return;
        };
        // The cache only saves time; without it, the SCPD is fetched again.
        let _ = self.write(&device, url, xml);
        self.lock().insert((device, url.to_owned()), scpd.clone());
    }
}

/// The device named in the directory, if its file can be read.
fn read_device(dir: &Path) -> Option<Device> {
    serde_json::from_slice(&fs::read(dir.join(DEVICE_FILE)).ok()?).ok()
}

/// The file of the SCPD at `url`: a hash, so that any URL gives a valid and
/// distinct name.
fn file_name(url: &str) -> String {
    let hash = Sha256::digest(url.as_bytes());
    format!("{}.xml", crate::session::to_hex(&hash[..16]))
}

/// Writes a file through a temporary file and a rename, so that readers
/// never see a part of it.
fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("{}.{count}.tmp", std::process::id()));
    let result = fs::write(&temporary, contents).and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIPTION: &str = r"<root>
<systemVersion><Display>285.08.25</Display></systemVersion>
<device><UDN>uuid:1</UDN><modelName>FRITZ!Box 7690</modelName></device>
</root>";

    const SCPD: &str =
        r"<scpd><actionList><action><name>GetInfo</name></action></actionList></scpd>";

    fn device() -> Device {
        Device {
            id: "uuid:1".to_owned(),
            model: Some("FRITZ!Box 7690".to_owned()),
            firmware: "285.08.25".to_owned(),
        }
    }

    #[test]
    fn names_devices() {
        assert_eq!(Device::of(DESCRIPTION), Some(device()));
        let serial = "<root><systemVersion><Display>7.32</Display></systemVersion>\
            <device><serialNumber>AA:BB</serialNumber></device></root>";
        let device = Device::of(serial).unwrap();
        assert_eq!(
            (device.id.as_str(), device.model.as_deref()),
            ("AA:BB", None)
        );
        assert_eq!(device.dir_name(), "AA-BB-1009c30b");
        for unknown in [
            "not xml",
            "<root/>",
            "<root><device><UDN>uuid:1</UDN></device></root>",
            "<root><systemVersion><Display> </Display></systemVersion><device><UDN>u</UDN></device></root>",
            "<root><systemVersion><Display>1</Display></systemVersion><device/></root>",
        ] {
            assert_eq!(Device::of(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn keeps_scpds_per_device_and_firmware() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("cache"));
        assert_eq!(cache.dir(), dir.path().join("cache"));
        assert_eq!(cache.list().unwrap(), []);
        assert_eq!(cache.scpd(DESCRIPTION, "/a.xml"), None);

        let scpd = Scpd::parse(SCPD).unwrap();
        cache.store(DESCRIPTION, "/a.xml", SCPD, &scpd);
        assert_eq!(cache.scpd(DESCRIPTION, "/a.xml"), Some(scpd.clone()));
        assert_eq!(cache.scpd(DESCRIPTION, "/b.xml"), None);

        // Another process reads the files.
        let other = Cache::new(cache.dir());
        assert_eq!(other.scpd(DESCRIPTION, "/a.xml"), Some(scpd.clone()));
        let entries = other.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].device, Some(device()));
        assert_eq!(entries[0].path, cache.dir().join(device().dir_name()));
        assert!(device().dir_name().starts_with("uuid-1-"));
        assert_eq!(entries[0].scpds, 1);
        assert!(entries[0].bytes > SCPD.len() as u64);

        // A new firmware replaces the files of the device.
        let updated = DESCRIPTION.replace("285.08.25", "285.08.30");
        assert_eq!(Cache::new(cache.dir()).scpd(&updated, "/a.xml"), None);
        other.store(&updated, "/b.xml", SCPD, &scpd);
        assert_eq!(Cache::new(cache.dir()).scpd(DESCRIPTION, "/a.xml"), None);
        assert_eq!(Cache::new(cache.dir()).scpd(&updated, "/a.xml"), None);
        assert_eq!(Cache::new(cache.dir()).scpd(&updated, "/b.xml"), Some(scpd));
        assert_eq!(other.list().unwrap()[0].scpds, 1);
    }

    #[test]
    fn keeps_every_firmware_in_its_own_directory() {
        let dir = tempfile::tempdir().unwrap();
        let scpd = Scpd::parse(SCPD).unwrap();
        let updated = DESCRIPTION.replace("285.08.25", "285.08.30");
        let old = Device::of(DESCRIPTION).unwrap();
        let new = Device::of(&updated).unwrap();
        assert_ne!(old.dir_name(), new.dir_name());
        let mut other_model = old.clone();
        other_model.model = None;
        assert_ne!(old.dir_name(), other_model.dir_name());

        // A process with the old firmware checked its directory, then
        // another one stored the new firmware. The SCPD the first one writes
        // afterwards never ends up in the directory of the new firmware.
        let before = Cache::new(dir.path());
        before.store(DESCRIPTION, "/a.xml", SCPD, &scpd);
        let after = Cache::new(dir.path());
        after.store(&updated, "/b.xml", SCPD, &scpd);
        assert!(!dir.path().join(old.dir_name()).exists());
        let late = dir.path().join(old.dir_name()).join(file_name("/c.xml"));
        write_atomic(&late, SCPD.as_bytes()).unwrap_err();
        let entries = after.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].device, Some(new.clone()));
        assert_eq!(entries[0].scpds, 1);
        assert_eq!(Cache::new(dir.path()).scpd(&updated, "/c.xml"), None);

        // Each firmware replaces the directory of the other one.
        before.store(DESCRIPTION, "/c.xml", SCPD, &scpd);
        assert_eq!(after.list().unwrap()[0].device, Some(old));
        after.store(&updated, "/b.xml", SCPD, &scpd);
        let entries = after.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].device, Some(new));
        assert_eq!(Cache::new(dir.path()).scpd(DESCRIPTION, "/c.xml"), None);
    }

    #[test]
    fn ignores_what_it_cannot_parse() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let scpd = Scpd::parse(SCPD).unwrap();
        cache.store(DESCRIPTION, "/a.xml", SCPD, &scpd);
        let file = dir
            .path()
            .join(device().dir_name())
            .join(file_name("/a.xml"));
        fs::write(&file, "<scpd><actionList>").unwrap();
        assert_eq!(Cache::new(dir.path()).scpd(DESCRIPTION, "/a.xml"), None);

        fs::write(dir.path().join(device().dir_name()).join(DEVICE_FILE), "{").unwrap();
        let entries = cache.list().unwrap();
        assert_eq!(entries[0].device, None);
        assert_eq!(Cache::new(dir.path()).scpd(DESCRIPTION, "/a.xml"), None);

        // A description without a firmware version is not cached.
        let unversioned = "<root><device><UDN>uuid:2</UDN></device></root>";
        cache.store(unversioned, "/a.xml", SCPD, &scpd);
        assert_eq!(cache.scpd(unversioned, "/a.xml"), None);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn clears_only_its_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path());
        assert_eq!(Cache::new(dir.path().join("missing")).clear().unwrap(), 0);
        let scpd = Scpd::parse(SCPD).unwrap();
        cache.store(DESCRIPTION, "/a.xml", SCPD, &scpd);
        fs::create_dir(dir.path().join("other")).unwrap();
        fs::write(dir.path().join("notes.txt"), "kept").unwrap();

        assert_eq!(cache.clear().unwrap(), 1);
        assert!(!dir.path().join(device().dir_name()).exists());
        assert!(dir.path().join("other").exists());
        assert!(dir.path().join("notes.txt").exists());
        assert_eq!(cache.scpd(DESCRIPTION, "/a.xml"), None);

        // An empty directory is removed.
        let cache = Cache::new(dir.path().join("cache"));
        cache.store(DESCRIPTION, "/a.xml", SCPD, &scpd);
        assert_eq!(cache.clear().unwrap(), 1);
        assert!(!cache.dir().exists());
    }

    #[test]
    fn survives_an_unwritable_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, "").unwrap();
        // The cache directory is a file, so nothing can be written.
        let cache = Cache::new(&file);
        let scpd = Scpd::parse(SCPD).unwrap();
        cache.store(DESCRIPTION, "/a.xml", SCPD, &scpd);
        // The memory still has it.
        assert_eq!(cache.scpd(DESCRIPTION, "/a.xml"), Some(scpd));
        assert!(Cache::new(&file).list().is_err());
        assert!(write_atomic(&file.join("x"), b"").is_err());
    }
}
