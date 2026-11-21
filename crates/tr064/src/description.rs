//! The device description (`tr64desc.xml` on most devices): the devices and
//! the services they offer.

use roxmltree::{Document, Node};

use crate::{Error, xml};

/// A parsed device description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDescription {
    /// The root device. Further devices are nested in [`Device::devices`].
    pub device: Device,
}

/// A device of a [`DeviceDescription`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// The device type, for example `urn:dslforum-org:device:InternetGatewayDevice:1`.
    pub device_type: String,
    /// A name for people.
    pub friendly_name: Option<String>,
    /// The manufacturer.
    pub manufacturer: Option<String>,
    /// The model name.
    pub model_name: Option<String>,
    /// The model number.
    pub model_number: Option<String>,
    /// A description of the model.
    pub model_description: Option<String>,
    /// The serial number.
    pub serial_number: Option<String>,
    /// The unique device name, usually `uuid:` followed by a UUID.
    pub udn: Option<String>,
    /// The URL of the device's web interface.
    pub presentation_url: Option<String>,
    /// The services of this device, without those of embedded devices.
    pub services: Vec<Service>,
    /// Embedded devices.
    pub devices: Vec<Device>,
}

/// A service of a [`Device`]. The URLs are as given in the description,
/// usually paths relative to the address of the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// For example `urn:dslforum-org:service:DeviceInfo:1`.
    pub service_type: String,
    /// For example `urn:DeviceInfo-com:serviceId:DeviceInfo1`.
    pub service_id: String,
    /// The URL action calls go to.
    pub control_url: String,
    /// The URL for event subscriptions.
    pub event_sub_url: Option<String>,
    /// The URL of the service's [`Scpd`](crate::Scpd).
    pub scpd_url: String,
}

impl DeviceDescription {
    /// Parses a device description document.
    pub fn parse(xml: &str) -> Result<Self, Error> {
        let doc = Document::parse(xml)?;
        let root = xml::root(&doc, "root")?;
        let device = xml::child(root, "device").ok_or(Error::MissingElement {
            parent: "root",
            element: "device",
        })?;
        Ok(Self {
            device: parse_device(device)?,
        })
    }

    /// The root device and all embedded devices, depth first.
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        let mut stack = vec![&self.device];
        std::iter::from_fn(move || {
            let device = stack.pop()?;
            stack.extend(device.devices.iter().rev());
            Some(device)
        })
    }

    /// The services of all devices, depth first.
    pub fn services(&self) -> impl Iterator<Item = &Service> {
        self.devices().flat_map(|d| d.services.iter())
    }

    /// Finds a service by its type (`urn:dslforum-org:service:DeviceInfo:1`),
    /// its id (`urn:DeviceInfo-com:serviceId:DeviceInfo1`), or a short form of
    /// either (`DeviceInfo:1`, `DeviceInfo1`).
    pub fn service(&self, reference: &str) -> Result<&Service, Error> {
        let matches: Vec<&Service> = self.services().filter(|s| s.matches(reference)).collect();
        match matches.as_slice() {
            [] => Err(Error::UnknownService(reference.to_owned())),
            [service] => Ok(service),
            _ => Err(Error::AmbiguousService {
                reference: reference.to_owned(),
                candidates: matches.iter().map(|s| s.service_id.clone()).collect(),
            }),
        }
    }
}

impl Service {
    /// The name part of the service type, for example `DeviceInfo`.
    pub fn name(&self) -> &str {
        self.service_type.rsplit(':').nth(1).unwrap_or_default()
    }

    /// The version part of the service type, for example `1`.
    pub fn version(&self) -> &str {
        self.service_type.rsplit(':').next().unwrap_or_default()
    }

    /// Whether `reference` names this service; see [`DeviceDescription::service`].
    pub fn matches(&self, reference: &str) -> bool {
        let (name, version) = (self.name(), self.version());
        reference == self.service_type
            || reference == self.service_id
            || self.service_id.rsplit(':').next() == Some(reference)
            || reference
                .strip_prefix(name)
                .is_some_and(|rest| rest == version || rest.strip_prefix(':') == Some(version))
    }
}

fn parse_device(node: Node<'_, '_>) -> Result<Device, Error> {
    Ok(Device {
        device_type: xml::required(node, "device", "deviceType")?,
        friendly_name: xml::optional(node, "friendlyName"),
        manufacturer: xml::optional(node, "manufacturer"),
        model_name: xml::optional(node, "modelName"),
        model_number: xml::optional(node, "modelNumber"),
        model_description: xml::optional(node, "modelDescription"),
        serial_number: xml::optional(node, "serialNumber"),
        udn: xml::optional(node, "UDN"),
        presentation_url: xml::optional(node, "presentationURL"),
        services: xml::list(node, "serviceList", "service", parse_service)?,
        devices: xml::list(node, "deviceList", "device", parse_device)?,
    })
}

fn parse_service(node: Node<'_, '_>) -> Result<Service, Error> {
    Ok(Service {
        service_type: xml::required(node, "service", "serviceType")?,
        service_id: xml::required(node, "service", "serviceId")?,
        control_url: xml::required(node, "service", "controlURL")?,
        event_sub_url: xml::optional(node, "eventSubURL"),
        scpd_url: xml::required(node, "service", "SCPDURL")?,
    })
}
