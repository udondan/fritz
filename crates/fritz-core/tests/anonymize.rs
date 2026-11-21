//! Anonymizing raw captures into fixtures and checking fixtures.

use std::fs;
use std::path::{Path, PathBuf};

use fritz_core::anonymize::{self, Error, Finding, Kind};

/// Writes files into a directory, creating the parent directories.
fn tree(dir: &Path, files: &[(&str, &str)]) {
    for (file, text) in files {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

fn read(dir: &Path, file: &str) -> String {
    fs::read_to_string(dir.join(file)).unwrap()
}

fn finding(file: &str, kind: Kind) -> Finding {
    Finding {
        file: PathBuf::from(file),
        kind,
    }
}

const DESCRIPTION: &str = "<root><device><friendlyName>FRITZ!Box 7590</friendlyName>\
    <serialNumber>3810D501020A</serialNumber>\
    <UDN>uuid:75802409-bccb-40e7-8e6c-3810d501020a</UDN></device></root>\n";

const SCPD: &str = "<scpd><action><name>GetInfo</name>\
    <argument><name>NewSSID</name></argument></action></scpd>\n";

const INFO: &str = r#"{
  "out": {
    "NewBSSID": "38:10:D5:01:02:0A",
    "NewEnable": "1",
    "NewSSID": "Max Home"
  },
  "status": "ok"
}
"#;

const FAULT: &str = r#"{"status": "fault", "code": 714, "description": "NoSuchEntryInArray"}"#;

/// A raw capture with a device description, a service description, an
/// answer and a fault, and a hidden file that is left out.
fn raw(dir: &Path) {
    tree(
        dir,
        &[
            ("tr64desc.xml", DESCRIPTION),
            ("scpd/wlanconfigSCPD.xml", SCPD),
            ("responses/WLANConfiguration1/GetInfo.json", INFO),
            ("responses/Hosts1/GetGenericHostEntry@9.json", FAULT),
            (".DS_Store", "\u{0}"),
        ],
    );
}

#[test]
fn anonymizes_a_capture_consistently() {
    let temp = tempfile::tempdir().unwrap();
    let (raw_dir, out) = (temp.path().join("raw"), temp.path().join("out"));
    raw(&raw_dir);

    assert_eq!(anonymize::anonymize(&raw_dir, &out, &[]).unwrap(), 4);

    assert_eq!(
        read(&out, "tr64desc.xml"),
        "<root><device><friendlyName>FRITZ!Box 7590</friendlyName>\
         <serialNumber>AABBCC000001</serialNumber>\
         <UDN>uuid:00000000-0000-4000-8000-000000000001</UDN></device></root>\n"
    );
    assert_eq!(read(&out, "scpd/wlanconfigSCPD.xml"), SCPD);
    assert_eq!(
        read(&out, "responses/WLANConfiguration1/GetInfo.json"),
        INFO.replace("38:10:D5:01:02:0A", "AA:BB:CC:00:00:01")
            .replace("Max Home", "wlan-1")
    );
    assert!(read(&out, "responses/Hosts1/GetGenericHostEntry@9.json").contains("714"));
    assert!(!out.join(".DS_Store").exists());
    assert_eq!(anonymize::check(&out, &[]).unwrap(), []);
}

#[test]
fn writes_nothing_when_values_remain() {
    let temp = tempfile::tempdir().unwrap();
    let (raw_dir, out) = (temp.path().join("raw"), temp.path().join("out"));
    raw(&raw_dir);
    tree(
        &raw_dir,
        &[("notes.txt", "seen: 3810d501020a, max home, Musterweg")],
    );

    let private = vec!["musterweg".to_owned()];
    let Err(Error::Leftovers(findings)) = anonymize::anonymize(&raw_dir, &out, &private) else {
        panic!("expected leftovers");
    };
    assert_eq!(
        findings,
        [
            finding("notes.txt", Kind::Mac),
            finding("notes.txt", Kind::Wlan),
            finding("notes.txt", Kind::Private)
        ]
    );
    assert!(!out.exists());
    assert_eq!(
        Error::Leftovers(findings).to_string(),
        "3 personal values remain after anonymizing"
    );
}

#[test]
fn refuses_to_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    let raw_dir = temp.path().join("raw");
    raw(&raw_dir);

    let error = anonymize::anonymize(&raw_dir, temp.path(), &[]).unwrap_err();
    assert!(matches!(error, Error::Io { .. }), "{error}");
}

#[test]
fn checks_a_directory_of_fixtures() {
    let temp = tempfile::tempdir().unwrap();
    let fixtures = temp.path().join("fixtures");
    for version in ["8.25", "8.50"] {
        let raw_dir = temp.path().join(version);
        raw(&raw_dir);
        let out = fixtures.join("fritzbox-7590").join(version);
        anonymize::anonymize(&raw_dir, &out, &[]).unwrap();
    }
    // The service descriptions name an action `GetInfo`, which counts as a
    // protocol word in the fixture they belong to.
    tree(
        &fixtures,
        &[(
            "fritzbox-7590/8.50/responses/WLANConfiguration1/GetSSID.json",
            r#"{"out": {"NewSSID": "GetInfo"}}"#,
        )],
    );

    assert_eq!(anonymize::check(&fixtures, &[]).unwrap(), []);
}

#[test]
fn anonymizes_strings_in_json_arrays() {
    let temp = tempfile::tempdir().unwrap();
    let (raw_dir, out) = (temp.path().join("raw"), temp.path().join("out"));
    tree(
        &raw_dir,
        &[(
            "lists/hosts.json",
            r#"[{"NewHostName": "laptop"}, "10.0.0.2"]"#,
        )],
    );

    assert_eq!(anonymize::anonymize(&raw_dir, &out, &[]).unwrap(), 1);
    assert_eq!(
        read(&out, "lists/hosts.json"),
        "[\n  {\n    \"NewHostName\": \"device-1\"\n  },\n  \"192.0.2.1\"\n]\n"
    );
}

#[test]
fn anonymizes_the_host_and_mesh_lists() {
    let temp = tempfile::tempdir().unwrap();
    let (raw_dir, out) = (temp.path().join("raw"), temp.path().join("out"));
    tree(
        &raw_dir,
        &[
            ("scpd/hostsSCPD.xml", "<scpd><name>AP:5G:0</name></scpd>\n"),
            (
                "lists/devicehostlist.xml",
                "<List><Item><HostName>laptop</HostName><ClientID>01:aa</ClientID>\
                 <X_AVM-DE_FriendlyName>laptop</X_AVM-DE_FriendlyName>\
                 <X_AVM-DE_InfoURL>http://laptop.home/info</X_AVM-DE_InfoURL>\
                 <InterfaceType>802.11</InterfaceType></Item></List>\n",
            ),
            (
                "lists/meshlist.json",
                r#"{"nodes": [{"device_name": "laptop", "device_model": "AX 3000",
                    "node_interfaces": [{"name": "AP:5G:0", "ssid": "Max Home"}]}]}"#,
            ),
        ],
    );

    assert_eq!(anonymize::anonymize(&raw_dir, &out, &[]).unwrap(), 3);
    assert_eq!(
        read(&out, "lists/devicehostlist.xml"),
        "<List><Item><HostName>device-1</HostName><ClientID>id-1</ClientID>\
         <X_AVM-DE_FriendlyName>device-1</X_AVM-DE_FriendlyName>\
         <X_AVM-DE_InfoURL>https://url-1.example.net/</X_AVM-DE_InfoURL>\
         <InterfaceType>802.11</InterfaceType></Item></List>\n"
    );
    let mesh = read(&out, "lists/meshlist.json");
    assert!(mesh.contains("\"device_name\": \"device-1\""), "{mesh}");
    assert!(mesh.contains("\"ssid\": \"wlan-1\""), "{mesh}");
    assert!(mesh.contains("\"device_model\": \"AX 3000\""), "{mesh}");
    assert!(mesh.contains("\"name\": \"AP:5G:0\""), "{mesh}");
    assert_eq!(anonymize::check(&out, &[]).unwrap(), []);
}

#[test]
fn reports_an_unwritable_destination() {
    let temp = tempfile::tempdir().unwrap();
    let raw_dir = temp.path().join("raw");
    raw(&raw_dir);
    tree(temp.path(), &[("file", "")]);

    let error = anonymize::anonymize(&raw_dir, &temp.path().join("file/sub/out"), &[]);
    assert!(matches!(error, Err(Error::Io { .. })), "{error:?}");
}

#[test]
fn reports_unreadable_input() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    let error = anonymize::check(&missing, &[]).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("cannot read or write {}", missing.display())
    );

    tree(temp.path(), &[("raw/responses/S/A.json", "{")]);
    let error = anonymize::anonymize(&temp.path().join("raw"), &missing, &[]).unwrap_err();
    assert!(
        error.to_string().ends_with("A.json is not valid JSON"),
        "{error}"
    );
}

#[test]
fn checks_fixtures_without_naming_values() {
    let temp = tempfile::tempdir().unwrap();
    raw(temp.path());
    tree(temp.path(), &[("meta.json", "{\"model\": \"Musterbox\"}")]);

    let private = vec!["musterbox".to_owned()];
    let findings = anonymize::check(temp.path(), &private).unwrap();
    assert_eq!(
        findings,
        [
            finding("meta.json", Kind::Private),
            finding("responses/WLANConfiguration1/GetInfo.json", Kind::Mac),
            finding("responses/WLANConfiguration1/GetInfo.json", Kind::Wlan),
            finding("tr64desc.xml", Kind::Mac),
            finding("tr64desc.xml", Kind::Uuid),
        ]
    );
    assert_eq!(
        findings[1].to_string(),
        "responses/WLANConfiguration1/GetInfo.json: MAC address"
    );
}

#[test]
fn reads_private_patterns() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(".private-patterns");
    fs::write(&path, "# own data\n\n  Musterweg 1 \nMax\n").unwrap();
    assert_eq!(
        anonymize::read_private_patterns(&path).unwrap(),
        ["Musterweg 1", "Max"]
    );
}

#[test]
fn labels_every_kind() {
    let kinds = [
        Kind::Mac,
        Kind::Ipv4,
        Kind::Ipv6,
        Kind::Uuid,
        Kind::Email,
        Kind::MyFritz,
        Kind::Device,
        Kind::Wlan,
        Kind::Name,
        Kind::User,
        Kind::Phone,
        Kind::Provider,
        Kind::Server,
        Kind::Url,
        Kind::Id,
        Kind::Serial,
        Kind::Log,
        Kind::Private,
    ];
    let labels: Vec<String> = kinds.iter().map(ToString::to_string).collect();
    let mut unique = labels.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), kinds.len(), "{labels:?}");
}
