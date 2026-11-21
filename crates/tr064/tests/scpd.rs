use tr064::{AllowedRange, Direction, Scpd};

const SCPD: &str = include_str!("data/deviceinfoSCPD.xml");

fn scpd() -> Scpd {
    Scpd::parse(SCPD).expect("valid SCPD")
}

#[test]
fn parses_actions_and_arguments() {
    let scpd = scpd();
    let names: Vec<&str> = scpd.actions.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["GetInfo", "SetProvisioningCode", "Reboot"]);

    let get_info = scpd.action("GetInfo").unwrap();
    assert_eq!(get_info.inputs().count(), 0);
    let outputs: Vec<(&str, &str)> = get_info
        .outputs()
        .map(|a| (a.name.as_str(), a.related_state_variable.as_str()))
        .collect();
    assert_eq!(
        outputs,
        [
            ("NewManufacturerName", "ManufacturerName"),
            ("NewUpTime", "UpTime")
        ]
    );

    // The direction is matched without regard to case.
    let set = scpd.action("SetProvisioningCode").unwrap();
    let input = set.inputs().next().unwrap();
    assert_eq!(input.name, "NewProvisioningCode");
    assert_eq!(input.direction, Direction::In);
    assert_eq!(set.outputs().count(), 0);

    assert_eq!(
        scpd.action("Reboot").unwrap().arguments,
        [] as [tr064::Argument; 0]
    );
    assert!(scpd.action("Nope").is_none());
}

#[test]
fn parses_state_variables() {
    let scpd = scpd();

    let name = scpd.state_variable("ManufacturerName").unwrap();
    assert_eq!(name.data_type, "string");
    assert!(!name.send_events);
    assert_eq!(name.default_value, None);
    assert_eq!(name.allowed_values, [] as [String; 0]);
    assert_eq!(name.allowed_range, None);

    let uptime = scpd.state_variable("UpTime").unwrap();
    assert_eq!(uptime.data_type, "ui4");
    assert!(uptime.send_events);
    assert_eq!(uptime.default_value.as_deref(), Some("0"));
    assert_eq!(
        uptime.allowed_range,
        Some(AllowedRange {
            minimum: Some("0".into()),
            maximum: Some("4294967295".into()),
            step: None,
        })
    );

    // Without the attribute, events are sent.
    assert!(scpd.state_variable("ProvisioningCode").unwrap().send_events);

    // Values are trimmed, empty ones dropped.
    let status = scpd.state_variable("Status").unwrap();
    assert_eq!(status.allowed_values, ["Up", "Down"]);

    assert!(scpd.state_variable("Nope").is_none());
}

#[test]
fn reads_empty_scpd() {
    let scpd = Scpd::parse("<scpd/>").unwrap();
    assert_eq!(scpd.actions, [] as [tr064::Action; 0]);
    assert_eq!(scpd.state_variables, [] as [tr064::StateVariable; 0]);
}

#[test]
fn rejects_invalid_documents() {
    let arg = |inner: &str| {
        format!(
            "<scpd><actionList><action><name>A</name><argumentList><argument>{inner}</argument></argumentList></action></actionList></scpd>"
        )
    };
    let cases = [
        ("<root/>".to_owned(), "unexpected root element <root>, expected <scpd>"),
        (
            "<scpd><actionList><action/></actionList></scpd>".to_owned(),
            "<action> has no <name>",
        ),
        (arg("<name>X</name>"), "<argument> has no <direction>"),
        (
            arg("<name>X</name><direction>sideways</direction><relatedStateVariable>V</relatedStateVariable>"),
            "invalid value \"sideways\" in <direction>",
        ),
        (
            arg("<direction>in</direction><relatedStateVariable>V</relatedStateVariable>"),
            "<argument> has no <name>",
        ),
        (
            arg("<name>X</name><direction>out</direction>"),
            "<argument> has no <relatedStateVariable>",
        ),
        (
            "<scpd><serviceStateTable><stateVariable/></serviceStateTable></scpd>".to_owned(),
            "<stateVariable> has no <name>",
        ),
        (
            "<scpd><serviceStateTable><stateVariable><name>V</name></stateVariable></serviceStateTable></scpd>"
                .to_owned(),
            "<stateVariable> has no <dataType>",
        ),
    ];
    for (xml, message) in cases {
        let err = Scpd::parse(&xml).unwrap_err();
        assert_eq!(err.to_string(), message, "{xml}");
    }
}
