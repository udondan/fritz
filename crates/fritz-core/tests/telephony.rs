//! The port forwardings, the telephony and the capabilities of the
//! simulated 7690.

use std::collections::BTreeMap;

use fritz_core::capabilities;
use fritz_core::ports::{self, Forwarding};
use fritz_core::read::Reader;
use fritz_core::telephony::{self, Account, AnsweringMachine, Deflection, Handset, Number};
use fritz_sim::{Answer, Fixture, Injection, Sim};
use tr064::SecretString;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritzbox-7690/8.25"
);

fn fixture() -> Fixture {
    Fixture::load(FIXTURE).unwrap()
}

async fn reader(sim: &Sim) -> Reader {
    Reader::new(sim.client().build().unwrap()).await.unwrap()
}

fn ok(out: &[(&str, &str)]) -> Answer {
    Answer::Ok(
        out.iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn mapping(port: &str, client: &str) -> Answer {
    ok(&[
        ("NewRemoteHost", ""),
        ("NewExternalPort", port),
        ("NewProtocol", "TCP"),
        ("NewInternalPort", "22"),
        ("NewInternalClient", client),
        ("NewEnabled", "1"),
        ("NewPortMappingDescription", "ssh"),
        ("NewLeaseDuration", "0"),
    ])
}

#[tokio::test]
async fn reads_no_forwardings() {
    let sim = Sim::start(fixture()).await.unwrap();
    let ports = ports::ports(&mut reader(&sim).await).await.unwrap();
    assert_eq!(ports.upnp, Some(true));
    assert_eq!(ports.forwardings, []);
}

#[tokio::test]
async fn reads_forwardings() {
    let service = "WANPPPConnection1";
    let fixture = fixture()
        .with_answer(
            service,
            "GetPortMappingNumberOfEntries",
            None,
            ok(&[("NewPortMappingNumberOfEntries", "3")]),
        )
        .with_answer(
            service,
            "GetGenericPortMappingEntry",
            Some(0),
            mapping("2222", "192.0.2.1"),
        )
        .with_answer(
            service,
            "GetGenericPortMappingEntry",
            Some(1),
            ok(&[("NewEnabled", "0"), ("NewLeaseDuration", "3600")]),
        );
    let sim = Sim::start(fixture).await.unwrap();
    let ports = ports::ports(&mut reader(&sim).await).await.unwrap();
    // The third entry is not recorded: the device ended the list early.
    assert_eq!(
        ports.forwardings,
        [
            Forwarding {
                service: service.to_owned(),
                enabled: true,
                protocol: Some("TCP".to_owned()),
                external_port: Some(2222),
                internal_client: Some("192.0.2.1".to_owned()),
                internal_port: Some(22),
                description: Some("ssh".to_owned()),
                remote_host: None,
                lease_duration: None,
            },
            Forwarding {
                service: service.to_owned(),
                lease_duration: Some(3600),
                ..Forwarding::default()
            },
        ]
    );
}

#[tokio::test]
async fn reads_the_telephony() {
    let sim = Sim::start(fixture()).await.unwrap();
    let telephony = telephony::telephony(&mut reader(&sim).await).await.unwrap();
    assert_eq!(
        telephony.numbers,
        [Number {
            number: Some("phone-1".to_owned()),
            kind: Some("eVoIP".to_owned()),
            name: None,
        }]
    );
    assert_eq!(
        telephony.accounts,
        [Account {
            index: 1,
            number: "phone-1".to_owned(),
            registrar: Some("server-4.example.net".to_owned()),
            status: Some("registered".to_owned()),
        }]
    );
    assert_eq!(telephony.deflections, []);
    assert_eq!(
        telephony.answering_machines,
        [AnsweringMachine {
            index: 0,
            name: Some("name-4".to_owned()),
            enabled: false,
        }]
    );
    assert_eq!(
        telephony.handsets,
        [Handset {
            id: Some("1".to_owned()),
            name: Some("name-1".to_owned()),
            model: Some("FRITZ!Fon C5".to_owned()),
            active: false,
            update_available: Some(false),
        }]
    );
    let serialized = serde_json::to_string(&telephony).unwrap();
    assert!(!serialized.contains("user-5"), "no user names");
}

#[tokio::test]
async fn reads_deflections() {
    let fixture = fixture().with_answer(
        "X_AVM-DE_OnTel1",
        "GetDeflections",
        None,
        ok(&[(
            "NewDeflectionList",
            "<List><Item><DeflectionId>0</DeflectionId><Enable>1</Enable>\
             <Type>fromNumber</Type><Number>phone-2</Number>\
             <DeflectionToNumber>phone-3</DeflectionToNumber>\
             <Mode>eImmediately</Mode></Item></List>",
        )]),
    );
    let sim = Sim::start(fixture).await.unwrap();
    let telephony = telephony::telephony(&mut reader(&sim).await).await.unwrap();
    assert_eq!(
        telephony.deflections,
        [Deflection {
            enabled: true,
            kind: Some("fromNumber".to_owned()),
            number: Some("phone-2".to_owned()),
            to: Some("phone-3".to_owned()),
            mode: Some("eImmediately".to_owned()),
        }]
    );
}

#[tokio::test]
async fn leaves_out_telephony_that_cannot_be_read() {
    let sim = Sim::start(fixture()).await.unwrap();
    for (service, action) in [
        ("X_VoIP1", "X_AVM-DE_GetNumbers"),
        ("X_VoIP1", "X_AVM-DE_GetVoIPAccounts"),
        ("X_AVM-DE_OnTel1", "GetDeflections"),
        ("X_AVM-DE_TAM1", "GetList"),
        ("X_AVM-DE_Dect1", "GetNumberOfDectEntries"),
        ("X_AVM-DE_UPnP1", "GetInfo"),
        ("WANPPPConnection1", "GetPortMappingNumberOfEntries"),
        ("LANConfigSecurity1", "X_AVM-DE_GetCurrentUser"),
        ("X_AVM-DE_Auth1", "GetInfo"),
    ] {
        sim.inject(Injection::fault(service, action, 606));
    }
    let mut reader = reader(&sim).await;
    let telephony = telephony::telephony(&mut reader).await.unwrap();
    assert_eq!(telephony, telephony::Telephony::default());
    let ports = ports::ports(&mut reader).await.unwrap();
    assert_eq!(ports, ports::Ports::default());
    let capabilities = capabilities::capabilities(&mut reader).await.unwrap();
    assert_eq!(capabilities.rights, []);
    assert_eq!(capabilities.two_factor, None);
}

#[tokio::test]
async fn rejects_telephony_lists_that_are_not_valid() {
    for (action, output) in [
        ("X_AVM-DE_GetNumbers", "NewNumberList"),
        ("X_AVM-DE_GetVoIPAccounts", "NewX_AVM-DE_VoIPAccountList"),
    ] {
        let fixture = fixture().with_answer("X_VoIP1", action, None, ok(&[(output, "<html/>")]));
        let sim = Sim::start(fixture).await.unwrap();
        let error = telephony::telephony(&mut reader(&sim).await)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not valid"), "{error}");
    }
}

#[tokio::test]
async fn reads_the_capabilities() {
    let sim = Sim::start(fixture()).await.unwrap();
    let capabilities = capabilities::capabilities(&mut reader(&sim).await)
        .await
        .unwrap();
    assert_eq!(
        capabilities.features,
        [
            "dsl",
            "mobile",
            "wlan",
            "hosts",
            "port_forwarding",
            "upnp",
            "parental_controls",
            "telephony",
            "dect",
            "answering_machine",
            "smarthome",
            "storage",
            "webdav",
            "powerline",
            "myfritz",
            "remote_access",
            "speedtest",
        ]
    );
    assert_eq!(capabilities.rights.len(), 6);
    assert_eq!(capabilities.two_factor, Some(false));
    assert_eq!(capabilities.services, 37);
}

#[tokio::test]
async fn fails_without_a_login() {
    let sim = Sim::start(fixture()).await.unwrap();
    let client = sim
        .client()
        .credentials(fritz_sim::USERNAME, SecretString::from("wrong"))
        .build()
        .unwrap();
    let mut reader = Reader::new(client).await.unwrap();
    assert!(ports::ports(&mut reader).await.is_err());
    assert!(telephony::telephony(&mut reader).await.is_err());
    assert!(capabilities::capabilities(&mut reader).await.is_err());
}
