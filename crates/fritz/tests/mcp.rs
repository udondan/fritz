//! End-to-end tests of `fritz mcp`, with a client that starts the binary.

use std::path::Path;

use fritz_sim::{Injection, Sim};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt as _, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt as _};
use serde_json::{Value, json};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritzbox-7690/8.25"
);

/// The tool list that clients see. `FRITZ_UPDATE_GOLDEN=1` rewrites it.
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/mcp-tools.json");

/// The most tokens the tool list may cost, counted as four characters each.
const TOKEN_BUDGET: usize = 5000;

type Client = RunningService<RoleClient, ()>;

async fn sim() -> Sim {
    Sim::start(fritz_sim::Fixture::load(FIXTURE).unwrap())
        .await
        .unwrap()
}

/// Starts `fritz mcp` for `sim`, without the `FRITZ_*` variables of the
/// caller and without the cache.
async fn connect(sim: &Sim) -> Client {
    connect_with(sim, &[]).await
}

/// Like [`connect`], with more arguments for `fritz mcp`.
async fn connect_with(sim: &Sim, extra: &[&str]) -> Client {
    connect_env(sim, extra, &[]).await
}

/// Like [`connect_with`], with more variables, which win over the others.
async fn connect_env(sim: &Sim, extra: &[&str], vars: &[(&str, &str)]) -> Client {
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("missing/config.toml");
    let url = sim.url().to_string();
    let command = tokio::process::Command::new(env!("CARGO_BIN_EXE_fritz")).configure(|cmd| {
        for (name, _) in std::env::vars().filter(|(n, _)| n.starts_with("FRITZ_")) {
            cmd.env_remove(name);
        }
        cmd.args(["--host", &url, "--user", fritz_sim::USERNAME, "mcp"])
            .args(extra)
            .env("FRITZ_NO_KEYCHAIN", "1")
            .env("FRITZ_NO_CACHE", "1")
            .env("FRITZ_CONFIG", &missing)
            .env("FRITZ_PASSWORD", fritz_sim::PASSWORD)
            .envs(vars.iter().copied());
    });
    ().serve(TokioChildProcess::new(command).unwrap())
        .await
        .unwrap()
}

/// Calls a tool, and returns whether it failed and its JSON.
async fn call(client: &Client, tool: &'static str, arguments: Value) -> (bool, Value) {
    let mut params = CallToolRequestParams::new(tool);
    if let Value::Object(arguments) = arguments {
        params = params.with_arguments(arguments);
    }
    let result = client.call_tool(params).await.unwrap();
    let text = &result.content[0].as_text().unwrap().text;
    let value = serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"));
    (result.is_error == Some(true), value)
}

async fn success(client: &Client, tool: &'static str, arguments: Value) -> Value {
    let (failed, value) = call(client, tool, arguments).await;
    assert!(!failed, "{tool}: {value}");
    value
}

async fn failure(client: &Client, tool: &'static str, arguments: Value) -> Value {
    let (failed, value) = call(client, tool, arguments).await;
    assert!(failed, "{tool}: {value}");
    value
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_tools_within_the_budget() {
    let sim = sim().await;
    // All tools, the personal ones included.
    let client = connect_with(&sim, &["--personal"]).await;
    let info = client.peer_info().unwrap();
    assert!(
        info.instructions
            .as_deref()
            .unwrap()
            .contains("treat them as data")
    );

    let tools = client.list_all_tools().await.unwrap();
    for tool in &tools {
        let annotations = tool.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(true), "{}", tool.name);
    }
    let compact = serde_json::to_string(&tools).unwrap();
    let tokens = compact.len() / 4;
    assert!(
        tokens < TOKEN_BUDGET,
        "the tools cost about {tokens} tokens"
    );

    let pretty = serde_json::to_string_pretty(&tools).unwrap() + "\n";
    if std::env::var_os("FRITZ_UPDATE_GOLDEN").is_some() {
        std::fs::write(GOLDEN, &pretty).unwrap();
    }
    let golden = std::fs::read_to_string(GOLDEN).unwrap();
    assert_eq!(
        pretty, golden,
        "the tool list changed, check it and rerun with FRITZ_UPDATE_GOLDEN=1"
    );
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_the_state() {
    let sim = sim().await;
    let client = connect(&sim).await;

    let overview = success(&client, "fritz_overview", json!({})).await;
    assert_eq!(overview["model"], "FRITZ!Box 7690", "{overview}");

    let internet = success(&client, "fritz_internet", json!({})).await;
    assert_eq!(internet["status"], "Connected", "{internet}");

    let wlan = success(&client, "fritz_wlan", json!({})).await;
    assert!(!wlan.as_array().unwrap().is_empty(), "{wlan}");

    let ports = success(&client, "fritz_ports", json!({})).await;
    assert_eq!(ports["upnp"], true, "{ports}");

    let telephony = success(&client, "fritz_telephony", json!({})).await;
    assert_eq!(
        telephony["accounts"][0]["status"], "registered",
        "{telephony}"
    );

    let capabilities = success(&client, "fritz_capabilities", json!({})).await;
    assert_eq!(
        capabilities["rights"].as_array().unwrap().len(),
        6,
        "{capabilities}"
    );

    let mesh = success(&client, "fritz_mesh", json!({})).await;
    assert!(!mesh["nodes"].as_array().unwrap().is_empty(), "{mesh}");
    let tree = success(&client, "fritz_mesh", json!({"tree": true})).await;
    assert_eq!(tree[0]["name"], "device-50", "{tree}");
    assert_eq!(tree[0]["children"][0]["name"], "device-51", "{tree}");
    assert_eq!(
        tree[0]["children"][0]["uplinks"][0]["kind"], "WLAN",
        "{tree}"
    );

    let smarthome = success(&client, "fritz_smarthome", json!({})).await;
    assert!(smarthome.is_array(), "{smarthome}");

    let settings = success(&client, "fritz_settings", json!({})).await;
    assert_eq!(settings["modules"]["ipv6"]["mtu_override"], "1492");
    assert!(settings.get("unavailable").is_none(), "{settings}");
    let wlan = success(&client, "fritz_settings", json!({"modules": ["wlan"]})).await;
    assert_eq!(wlan["modules"]["wlan"]["pskvalue"], "***", "{wlan}");

    let all = success(&client, "fritz_hosts", json!({})).await;
    let total = all["total"].as_u64().unwrap();
    assert!(total > 1, "{all}");
    assert_eq!(all["hosts"].as_array().unwrap().len() as u64, total);

    let page = success(&client, "fritz_hosts", json!({"offset": 1, "limit": 1})).await;
    assert_eq!(page["total"], total);
    assert_eq!(page["offset"], 1);
    assert_eq!(page["hosts"], json!([all["hosts"][1]]));

    let active = success(&client, "fritz_hosts", json!({"active": true})).await;
    let hosts = active["hosts"].as_array().unwrap();
    assert!(hosts.iter().all(|h| h["active"] == true), "{active}");

    let name = all["hosts"][0]["name"].as_str().unwrap().to_uppercase();
    let found = success(&client, "fritz_hosts", json!({"search": name})).await;
    assert!(found["total"].as_u64().unwrap() >= 1, "{found}");
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_describes_and_calls_actions() {
    let sim = sim().await;
    let client = connect(&sim).await;

    let services = success(&client, "fritz_list_services", json!({})).await;
    assert_eq!(services["DeviceInfo1"], "DeviceInfo:1", "{services}");
    let actions = success(
        &client,
        "fritz_list_services",
        json!({"service": "DeviceConfig:1"}),
    )
    .await;
    assert_eq!(actions["Reboot"], "critical", "{actions}");
    assert_eq!(actions["X_AVM-DE_GetConfigFile"], "secret", "{actions}");

    let describe = success(
        &client,
        "fritz_describe_action",
        json!({"service": "WANIPConnection1", "action": "GetInfo"}),
    )
    .await;
    assert_eq!(describe["class"], "read", "{describe}");
    assert_eq!(describe["in"], json!({}), "{describe}");
    assert_eq!(describe["out"]["NewUptime"], "ui4 (0..4294967295)");
    let describe = success(
        &client,
        "fritz_describe_action",
        json!({"service": "Hosts1", "action": "GetGenericHostEntry"}),
    )
    .await;
    assert!(describe["in"]["NewIndex"].is_string(), "{describe}");

    let info = success(
        &client,
        "fritz_call_action",
        json!({"service": "DeviceInfo1", "action": "GetInfo"}),
    )
    .await;
    assert_eq!(info["NewModelName"], "FRITZ!Box 7690", "{info}");
    let host = success(
        &client,
        "fritz_call_action",
        json!({
            "service": "Hosts1",
            "action": "GetGenericHostEntry",
            "arguments": {"NewIndex": "0"},
        }),
    )
    .await;
    assert!(host["NewMACAddress"].is_string(), "{host}");

    let error = failure(
        &client,
        "fritz_call_action",
        json!({"service": "DeviceConfig1", "action": "Reboot"}),
    )
    .await;
    assert!(
        error["error"].as_str().unwrap().contains("Reboot"),
        "{error}"
    );
    assert!(error.get("status").is_none(), "{error}");
    assert!(
        sim.requests()
            .iter()
            .all(|r| r.action.as_deref() != Some("Reboot"))
    );

    let error = failure(
        &client,
        "fritz_describe_action",
        json!({"service": "Hosts1", "action": "Nothing"}),
    )
    .await;
    assert!(
        error["error"].as_str().unwrap().contains("Nothing"),
        "{error}"
    );
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_personal_lists_only_on_request() {
    let sim = sim().await;
    let client = connect(&sim).await;
    let tools = client.list_all_tools().await.unwrap();
    for personal in ["fritz_calls", "fritz_phonebook", "fritz_log"] {
        assert!(tools.iter().all(|t| t.name != personal), "{personal}");
    }
    let call = |service: &str, action: &str| json!({"service": service, "action": action});
    let error = failure(
        &client,
        "fritz_call_action",
        call("DeviceInfo1", "GetDeviceLog"),
    )
    .await;
    assert!(
        error["error"].as_str().unwrap().contains("--personal"),
        "{error}"
    );
    assert!(
        sim.requests()
            .iter()
            .all(|r| r.action.as_deref() != Some("GetDeviceLog"))
    );
    // GetInfo is no personal action, but returns the log as well.
    let info = success(&client, "fritz_call_action", call("DeviceInfo1", "GetInfo")).await;
    assert_eq!(
        info["NewDeviceLog"], "(personal, needs `fritz mcp --personal`)",
        "{info}"
    );
    assert!(info["NewModelName"].is_string(), "{info}");
    for (service, action) in [("Nothing1", "GetInfo"), ("DeviceInfo1", "Nothing")] {
        let error = failure(&client, "fritz_call_action", call(service, action)).await;
        assert!(error["error"].is_string(), "{error}");
    }
    client.cancel().await.unwrap();

    let client = connect_with(&sim, &["--personal"]).await;
    let calls = success(&client, "fritz_calls", json!({})).await;
    assert_eq!(calls.as_array().unwrap().len(), 3, "{calls}");
    let missed = success(&client, "fritz_calls", json!({"kind": "missed"})).await;
    assert_eq!(missed.as_array().unwrap().len(), 1, "{missed}");
    let latest = success(&client, "fritz_calls", json!({"limit": 1})).await;
    assert_eq!(latest[0]["kind"], "missed", "{latest}");

    let found = success(&client, "fritz_phonebook", json!({"search": "name-8"})).await;
    assert_eq!(found["total"], 1, "{found}");
    assert_eq!(found["contacts"][0]["phonebook"], "name-5", "{found}");
    assert_eq!(found["contacts"][0]["numbers"][0]["number"], "phone-5");
    let page = success(&client, "fritz_phonebook", json!({"offset": 1, "limit": 1})).await;
    assert_eq!(page["total"], 2, "{page}");
    assert_eq!(page["contacts"][0]["name"], "name-8", "{page}");

    let log = success(&client, "fritz_log", json!({})).await;
    assert_eq!(log.as_array().unwrap().len(), 1, "{log}");
    let none = success(&client, "fritz_log", json!({"search": "nothing"})).await;
    assert_eq!(none, json!([]));
    let log = success(
        &client,
        "fritz_call_action",
        call("DeviceInfo1", "GetDeviceLog"),
    )
    .await;
    assert!(log["NewDeviceLog"].is_string(), "{log}");
    let info = success(&client, "fritz_call_action", call("DeviceInfo1", "GetInfo")).await;
    assert_eq!(
        info["NewDeviceLog"], "(removed by fritz fixture anonymize)",
        "{info}"
    );
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_the_descriptions_once() {
    let sim = sim().await;
    let client = connect(&sim).await;
    let fetched = |name: &str| {
        sim.requests()
            .iter()
            .filter(|r| r.method == "GET" && r.path.contains(name))
            .count()
    };

    success(&client, "fritz_overview", json!({})).await;
    let scpds = fetched("SCPD");
    assert!(scpds > 0);
    success(&client, "fritz_overview", json!({})).await;
    success(&client, "fritz_list_services", json!({})).await;
    success(
        &client,
        "fritz_describe_action",
        json!({"service": "DeviceInfo1", "action": "GetInfo"}),
    )
    .await;
    assert_eq!(fetched("tr64desc"), 1);
    assert_eq!(fetched("SCPD"), scpds);

    // After an error, the next tool starts over.
    failure(
        &client,
        "fritz_call_action",
        json!({"service": "DeviceConfig1", "action": "Reboot"}),
    )
    .await;
    success(&client, "fritz_overview", json!({})).await;
    assert_eq!(fetched("tr64desc"), 2);
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_faults_with_a_status() {
    let sim = sim().await;
    let client = connect(&sim).await;

    sim.inject(Injection::fault("DeviceInfo1", "GetInfo", 867));
    let error = failure(&client, "fritz_overview", json!({})).await;
    assert_eq!(error["status"], "blocked", "{error}");
    assert_eq!(error["retry_after"], 3600, "{error}");

    sim.clear_injections();
    sim.inject(Injection::fault("DeviceInfo1", "GetInfo", 866));
    let error = failure(
        &client,
        "fritz_call_action",
        json!({"service": "DeviceInfo1", "action": "GetInfo"}),
    )
    .await;
    assert_eq!(error["status"], "confirmation_required", "{error}");
    assert!(error.get("retry_after").is_none(), "{error}");
    client.cancel().await.unwrap();
}

/// The tools that change the device, in builds with the feature `write`.
#[cfg(feature = "write")]
mod change {
    use fritz_core::identity::Identity;

    use super::*;

    /// The identity of the device of the fixture.
    fn identity() -> String {
        Identity::of("uuid:00000000-0000-4000-8000-000000000001", "AABBCC000001").to_string()
    }

    struct Setup {
        sim: Sim,
        dir: tempfile::TempDir,
    }

    impl Setup {
        /// A simulator, and a configuration with the section `[write]`.
        async fn new(write: &str) -> Self {
            let setup = Self {
                sim: sim().await,
                dir: tempfile::tempdir().unwrap(),
            };
            let config = format!("[write]\n{write} = [\"{}\"]\n", identity());
            std::fs::write(setup.dir.path().join("config.toml"), config).unwrap();
            setup
        }

        /// Starts `fritz mcp` with the configuration, a state directory and
        /// the variables.
        async fn connect(&self, vars: &[(&str, &str)]) -> Client {
            let config = self.dir.path().join("config.toml");
            let state = self.dir.path().join("state");
            let mut all = vec![
                ("FRITZ_CONFIG", config.to_str().unwrap()),
                ("FRITZ_STATE_DIR", state.to_str().unwrap()),
            ];
            all.extend_from_slice(vars);
            connect_env(&self.sim, &[], &all).await
        }

        fn enabled(&self) -> Option<String> {
            self.sim.state("WLANConfiguration1", "Enable")
        }

        fn audit(&self) -> String {
            std::fs::read_to_string(self.dir.path().join("state/audit.jsonl")).unwrap_or_default()
        }
    }

    fn enable(value: &str) -> Value {
        json!({
            "service": "WLANConfiguration1",
            "action": "SetEnable",
            "arguments": {"NewEnable": value},
        })
    }

    fn with_token(mut change: Value, token: &Value) -> Value {
        change["token"] = token.clone();
        change
    }

    const WRITE: (&str, &str) = ("FRITZ_ALLOW_WRITE", "1");

    #[tokio::test(flavor = "multi_thread")]
    async fn offers_the_tools_only_with_the_permit() {
        let setup = Setup::new("allow").await;
        let client = setup.connect(&[]).await;
        let names: Vec<_> = client
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(!names.iter().any(|n| n.contains("change")), "{names:?}");
        client.cancel().await.unwrap();

        let client = setup.connect(&[WRITE]).await;
        let info = client.peer_info().unwrap();
        let instructions = info.instructions.as_deref().unwrap();
        assert!(instructions.contains("fritz_plan_change"), "{instructions}");
        let tools = client.list_all_tools().await.unwrap();
        let plan = tools
            .iter()
            .find(|t| t.name == "fritz_plan_change")
            .unwrap();
        let annotations = plan.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(true));
        let apply = tools
            .iter()
            .find(|t| t.name == "fritz_apply_change")
            .unwrap();
        let annotations = apply.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(true));
        client.cancel().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plans_and_applies_changes() {
        let setup = Setup::new("allow").await;
        let client = setup.connect(&[WRITE]).await;
        let plan = success(&client, "fritz_plan_change", enable("0")).await;
        assert_eq!(plan["device"], identity());
        assert_eq!(plan["class"], "write");
        assert_eq!(
            plan["changes"],
            json!([{"argument": "NewEnable", "before": "1", "after": "0"}])
        );
        assert_eq!(setup.enabled().as_deref(), Some("1"));

        let change = with_token(enable("0"), &plan["token"]);
        let outcome = success(&client, "fritz_apply_change", change.clone()).await;
        assert_eq!(outcome["verified"], true, "{outcome}");
        assert_eq!(setup.enabled().as_deref(), Some("0"));

        let error = failure(&client, "fritz_apply_change", change).await;
        let message = error["error"].as_str().unwrap();
        assert!(message.contains("the token is unknown, used"), "{message}");
        let audit = setup.audit();
        assert_eq!(audit.lines().count(), 4, "{audit}");
        assert!(
            audit.lines().all(|l| l.contains("\"source\":\"mcp\"")),
            "{audit}"
        );
        client.cancel().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refuses_changes_to_denied_devices() {
        let setup = Setup::new("deny").await;
        let client = setup.connect(&[WRITE]).await;
        let error = failure(&client, "fritz_plan_change", enable("0")).await;
        let message = error["error"].as_str().unwrap();
        assert!(message.contains(&identity()), "{message}");
        let change = with_token(enable("0"), &json!("0123"));
        failure(&client, "fritz_apply_change", change).await;
        assert_eq!(setup.enabled().as_deref(), Some("1"));
        assert!(
            !setup
                .sim
                .requests()
                .iter()
                .any(|r| r.action.as_deref() == Some("SetEnable"))
        );
        client.cancel().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn needs_the_critical_permit_for_critical_changes() {
        let setup = Setup::new("allow").await;
        let client = setup.connect(&[WRITE]).await;
        let reboot = json!({"service": "DeviceConfig1", "action": "Reboot"});
        let error = failure(&client, "fritz_plan_change", reboot).await;
        let message = error["error"].as_str().unwrap();
        assert!(message.contains("FRITZ_ALLOW_CRITICAL=1"), "{message}");
        client.cancel().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn keeps_the_descriptions_in_the_cache() {
    let sim = sim().await;
    let dir = tempfile::tempdir().unwrap();
    let vars = [
        ("FRITZ_NO_CACHE", "0"),
        ("FRITZ_CACHE_DIR", dir.path().to_str().unwrap()),
    ];
    let scpds = || {
        sim.requests()
            .iter()
            .filter(|r| r.method == "GET" && r.path != "/tr64desc.xml")
            .count()
    };

    let client = connect_env(&sim, &[], &vars).await;
    let first = success(&client, "fritz_overview", json!({})).await;
    let fetched = scpds();
    assert!(fetched > 0);
    // After an error, the descriptions come from memory.
    failure(
        &client,
        "fritz_call_action",
        json!({"service": "DeviceInfo1", "action": "SetProvisioningCode"}),
    )
    .await;
    success(&client, "fritz_overview", json!({})).await;
    assert_eq!(scpds(), fetched);
    client.cancel().await.unwrap();

    // The next server reads them from the cache directory.
    let client = connect_env(&sim, &[], &vars).await;
    assert_eq!(success(&client, "fritz_overview", json!({})).await, first);
    assert_eq!(scpds(), fetched);
    client.cancel().await.unwrap();
}
