use tr064::{DeviceDescription, Error};

const DESC: &str = include_str!("data/tr64desc.xml");

fn desc() -> DeviceDescription {
    DeviceDescription::parse(DESC).expect("valid description")
}

#[test]
fn parses_root_device() {
    let d = desc().device;
    assert_eq!(
        d.device_type,
        "urn:dslforum-org:device:InternetGatewayDevice:1"
    );
    assert_eq!(d.friendly_name.as_deref(), Some("Example Router"));
    assert_eq!(d.manufacturer.as_deref(), Some("Example Corp"));
    assert_eq!(d.model_name.as_deref(), Some("Example Router 1000"));
    assert_eq!(d.model_description.as_deref(), Some("Example Router 1000"));
    // Text is trimmed, empty elements are absent.
    assert_eq!(d.model_number.as_deref(), Some("1000"));
    assert_eq!(d.serial_number, None);
    assert_eq!(
        d.udn.as_deref(),
        Some("uuid:00000000-0000-0000-0000-000000000001")
    );
    assert_eq!(d.presentation_url.as_deref(), Some("http://192.0.2.1"));
    assert_eq!(d.devices.len(), 2);
}

#[test]
fn walks_devices_depth_first() {
    let desc = desc();
    let types: Vec<&str> = desc.devices().map(|d| d.device_type.as_str()).collect();
    assert_eq!(
        types,
        [
            "urn:dslforum-org:device:InternetGatewayDevice:1",
            "urn:dslforum-org:device:LANDevice:1",
            "urn:dslforum-org:device:WANDevice:1",
            "urn:dslforum-org:device:WANConnectionDevice:1",
        ]
    );
}

#[test]
fn lists_services_of_all_devices() {
    let desc = desc();
    let ids: Vec<&str> = desc
        .services()
        .map(|s| s.service_id.rsplit(':').next().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "DeviceInfo1",
            "X_EXAMPLE-COM_Storage1",
            "WLANConfiguration1",
            "WLANConfiguration2",
            "WLANConfiguration3",
            "WANCommonInterfaceConfig1",
            "WANIPConnection1",
            "WANIPConnection2",
        ]
    );
}

#[test]
fn parses_service() {
    let desc = desc();
    let s = desc.service("DeviceInfo1").unwrap();
    assert_eq!(s.service_type, "urn:dslforum-org:service:DeviceInfo:1");
    assert_eq!(s.service_id, "urn:DeviceInfo-com:serviceId:DeviceInfo1");
    assert_eq!(s.control_url, "/upnp/control/deviceinfo");
    assert_eq!(s.event_sub_url.as_deref(), Some("/upnp/control/deviceinfo"));
    assert_eq!(s.scpd_url, "/deviceinfoSCPD.xml");
    assert_eq!(s.name(), "DeviceInfo");
    assert_eq!(s.version(), "1");

    let storage = desc.service("X_EXAMPLE-COM_Storage1").unwrap();
    assert_eq!(storage.event_sub_url, None);
}

#[test]
fn finds_service_by_any_reference() {
    let desc = desc();
    for reference in [
        "urn:dslforum-org:service:WLANConfiguration:2",
        "urn:WLANConfiguration-com:serviceId:WLANConfiguration2",
        "WLANConfiguration2",
        "WLANConfiguration:2",
    ] {
        let s = desc.service(reference).unwrap();
        assert_eq!(s.control_url, "/upnp/control/wlanconfig2", "{reference}");
    }
}

#[test]
fn tells_services_of_the_same_type_apart_by_id() {
    let desc = desc();
    let s = desc.service("WANIPConnection2").unwrap();
    assert_eq!(s.control_url, "/upnp/control/wanipconnection2");

    let err = desc.service("WANIPConnection:1").unwrap_err();
    assert!(matches!(&err, Error::AmbiguousService { candidates, .. } if candidates.len() == 2));
    assert_eq!(
        err.to_string(),
        "\"WANIPConnection:1\" matches several services: \
         urn:WANIPConnection-com:serviceId:WANIPConnection1, \
         urn:WANIPConnection-com:serviceId:WANIPConnection2"
    );
}

#[test]
fn rejects_unknown_service() {
    let desc = desc();
    for reference in [
        "Nope1",
        "WLANConfiguration4",
        "WLANConfiguration",
        "DeviceInfo:",
    ] {
        let err = desc.service(reference).unwrap_err();
        assert!(matches!(err, Error::UnknownService(_)), "{reference}");
    }
    assert_eq!(
        desc.service("Nope1").unwrap_err().to_string(),
        "no service matches \"Nope1\""
    );
}

#[test]
fn reads_service_type_without_colons() {
    let xml = r"<root><device><deviceType>t</deviceType><serviceList><service>
        <serviceType>Plain</serviceType><serviceId>id</serviceId>
        <controlURL>/c</controlURL><SCPDURL>/s</SCPDURL>
        </service></serviceList></device></root>";
    let desc = DeviceDescription::parse(xml).unwrap();
    let s = desc.service("Plain").unwrap();
    assert_eq!(s.name(), "");
    assert_eq!(s.version(), "Plain");
}

#[test]
fn reads_minimal_device() {
    let desc = DeviceDescription::parse("<root><device><deviceType>t</deviceType></device></root>")
        .unwrap();
    assert_eq!(desc.device.services, [] as [tr064::Service; 0]);
    assert_eq!(desc.device.devices, [] as [tr064::Device; 0]);
    assert_eq!(desc.device.friendly_name, None);
}

#[test]
fn rejects_invalid_documents() {
    let cases = [
        ("<root>", "invalid XML: "),
        ("<scpd/>", "unexpected root element <scpd>, expected <root>"),
        ("<root/>", "<root> has no <device>"),
        ("<root><device/></root>", "<device> has no <deviceType>"),
        (
            "<root><device><deviceType> </deviceType></device></root>",
            "<device> has no <deviceType>",
        ),
        (
            "<root><device><deviceType>t</deviceType><serviceList><service/></serviceList></device></root>",
            "<service> has no <serviceType>",
        ),
        (
            "<root><device><deviceType>t</deviceType><deviceList><device/></deviceList></device></root>",
            "<device> has no <deviceType>",
        ),
    ];
    for (xml, message) in cases {
        let err = DeviceDescription::parse(xml).unwrap_err();
        assert!(err.to_string().starts_with(message), "{xml}: {err}");
    }
}
