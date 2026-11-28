//! The web interface: the session login of `login_sid.lua` and the REST API
//! under `/api/`, which wants the session in the header
//! `Authorization: AVM-SID <sid>`.
//!
//! The login answers a PBKDF2 challenge. A wrong answer reports a block time,
//! but the next login is not actually blocked; [`Injection::LockLogin`]
//! blocks every login.
//!
//! [`Injection::LockLogin`]: crate::Injection::LockLogin

use std::collections::HashMap;
use std::fmt::Write as _;

use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use sha2::Sha256;

use crate::{Injection, Options, SESSION, XML};

/// The login page.
pub(crate) const LOGIN: &str = "/login_sid.lua";
/// The prefix of the REST API.
pub(crate) const API: &str = "/api/";
/// The pages of the web interface, like `ecoStat`.
pub(crate) const DATA: &str = "/data.lua";
/// The Smart Home HTTP interface, for statistics.
pub(crate) const HOMEAUTO: &str = "/webservices/homeautoswitch.lua";
/// The page with the event log, which remembers its filter.
const LOG: &str = "log";
/// The filter of the page `log` that lists all entries.
const ALL: &str = "all";

/// The challenge of every login: 10 iterations with the salt `5e55104d`,
/// then 10 with `c0ffee55`.
const CHALLENGE: &str = "2$10$5e55104d$10$c0ffee55";
const SALTS: ([u8; 4], [u8; 4]) = ([0x5e, 0x55, 0x10, 0x4d], [0xc0, 0xff, 0xee, 0x55]);
const ITERATIONS: u32 = 10;
const NO_SESSION: &str = "0000000000000000";
/// The block time after a failed login.
const BLOCK_TIME: u64 = 8;
/// The block time of a [locked](crate::Injection::LockLogin) login.
const LOCKED_TIME: u64 = 60;

/// Whether the session is open, and the filter the page `log` remembers.
#[derive(Debug, Default)]
pub(crate) struct Web {
    open: bool,
    log_filter: Option<String>,
}

impl Web {
    /// A closed session, with the filter of the page `log` in `pages`.
    pub(crate) fn new(pages: &HashMap<String, String>) -> Self {
        let log_filter = pages
            .get(&format!("/{LOG}"))
            .and_then(|json| serde_json::from_str::<Value>(json).ok())
            .and_then(|page| page.pointer("/data/filter")?.as_str().map(str::to_owned));
        Self {
            open: false,
            log_filter,
        }
    }

    /// The filter the page `log` remembers.
    pub(crate) fn log_filter(&self) -> Option<&str> {
        self.log_filter.as_deref()
    }

    /// Answers a request of the login page: the challenge, a login or a
    /// logout.
    pub(crate) fn login(
        &mut self,
        method: &Method,
        body: &str,
        options: &Options,
        locked: bool,
    ) -> Response {
        let form: HashMap<String, String> = form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();
        let field = |name: &str| form.get(name).map(String::as_str);
        let blocked = if locked { LOCKED_TIME } else { 0 };
        let info = match *method {
            Method::GET => session_info(NO_SESSION, blocked),
            Method::POST if form.contains_key("logout") => {
                if field("sid") == Some(SESSION) {
                    self.open = false;
                }
                session_info(NO_SESSION, 0)
            }
            Method::POST
                if !locked
                    && field("username") == Some(&options.username)
                    && field("response") == Some(&response(&options.password)) =>
            {
                self.open = true;
                session_info(SESSION, 0)
            }
            Method::POST => session_info(NO_SESSION, blocked.max(BLOCK_TIME)),
            _ => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
        };
        ([(CONTENT_TYPE, XML)], info).into_response()
    }

    /// Answers a request of the REST API with a document of the fixture, if
    /// it carries the open session.
    pub(crate) fn api(
        &self,
        documents: &HashMap<String, String>,
        method: &Method,
        path: &str,
        headers: &HeaderMap,
    ) -> Response {
        if *method != Method::GET {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let authorization = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());
        if !self.open || authorization != Some(&format!("AVM-SID {SESSION}")) {
            return StatusCode::FORBIDDEN.into_response();
        }
        match documents.get(path) {
            Some(json) => ([(CONTENT_TYPE, "application/json")], json.clone()).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    /// Answers a form posted to `/data.lua` with the page it names in the
    /// field `page`, if the field `sid` carries the open session. The page
    /// `log` first takes its field `filter`, see [`Web::log`].
    pub(crate) fn page(
        &mut self,
        pages: &HashMap<String, String>,
        injections: &[Injection],
        method: &Method,
        body: &str,
    ) -> Response {
        let form = match self.form(method, body) {
            Ok(form) => form,
            Err(code) => return code.into_response(),
        };
        let Some((name, json)) = form
            .get("page")
            .and_then(|page| Some((page, pages.get(&format!("/{page}"))?)))
        else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let json = if name == LOG {
            self.log(json, form.get("filter"))
        } else {
            json.clone()
        };
        let failed = injections.iter().any(|i| {
            matches!(i, Injection::FailPage { page, field, value }
                if page == name && form.get(field) == Some(value))
        });
        if failed {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        ([(CONTENT_TYPE, "application/json")], json).into_response()
    }

    /// The page `log` like the device shows it: `filter`, if given, becomes
    /// the remembered filter, and the page lists only the entries of the
    /// remembered group, or all of them for `all`, and names it in
    /// `data.filter`. Without a remembered filter, it is the page of the
    /// fixture.
    fn log(&mut self, json: &str, filter: Option<&String>) -> String {
        if let Some(filter) = filter {
            self.log_filter = Some(filter.clone());
        }
        let (Some(filter), Ok(mut page)) = (&self.log_filter, serde_json::from_str::<Value>(json))
        else {
            return json.to_owned();
        };
        if let Some(entries) = page.pointer_mut("/data/log").and_then(Value::as_array_mut)
            && filter != ALL
        {
            entries.retain(|entry| {
                let group = entry.get("group").or_else(|| entry.get(4));
                group.and_then(Value::as_str) == Some(filter)
            });
        }
        if let Some(data) = page.get_mut("data").and_then(Value::as_object_mut) {
            data.insert("filter".to_owned(), Value::String(filter.clone()));
        }
        page.to_string()
    }

    /// Answers a form posted to `/webservices/homeautoswitch.lua` with the
    /// statistics of the device in the field `ain`, if the field `sid`
    /// carries the open session. Only the command `getbasicdevicestats` is
    /// served: the others may switch devices. Unknown devices get 400, like
    /// on the device.
    pub(crate) fn stats(
        &self,
        stats: &HashMap<String, String>,
        method: &Method,
        body: &str,
    ) -> Response {
        let form = match self.form(method, body) {
            Ok(form) => form,
            Err(code) => return code.into_response(),
        };
        if form.get("switchcmd").map(String::as_str) != Some("getbasicdevicestats") {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let ain: String = form
            .get("ain")
            .map_or_default(|ain| ain.chars().filter(|c| *c != ' ').collect());
        match stats.get(&ain) {
            Some(xml) => ([(CONTENT_TYPE, XML)], xml.clone()).into_response(),
            None => StatusCode::BAD_REQUEST.into_response(),
        }
    }

    /// The fields of a form posted in the open session, which the field
    /// `sid` carries, or the status of the answer to anything else.
    fn form(&self, method: &Method, body: &str) -> Result<HashMap<String, String>, StatusCode> {
        if *method != Method::POST {
            return Err(StatusCode::METHOD_NOT_ALLOWED);
        }
        let form: HashMap<String, String> = form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();
        if !self.open || form.get("sid").map(String::as_str) != Some(SESSION) {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(form)
    }
}

/// The answer of the login page. An open session comes with its rights.
fn session_info(sid: &str, block_time: u64) -> String {
    let rights = if sid == NO_SESSION {
        "<Rights/>"
    } else {
        "<Rights><Name>HomeAuto</Name><Access>2</Access>\
         <Name>BoxAdmin</Name><Access>2</Access></Rights>"
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><SessionInfo><SID>{sid}</SID>\
         <Challenge>{CHALLENGE}</Challenge><BlockTime>{block_time}</BlockTime>{rights}\
         <Users><User last=\"1\">fritz-sim</User></Users></SessionInfo>"
    )
}

/// The right answer to the challenge with `password`.
fn response(password: &str) -> String {
    let (salt1, salt2) = SALTS;
    let first = pbkdf2::pbkdf2_hmac_array::<Sha256, 32>(password.as_bytes(), &salt1, ITERATIONS);
    let second = pbkdf2::pbkdf2_hmac_array::<Sha256, 32>(&first, &salt2, ITERATIONS);
    format!("{}${}", hex(&salt2), hex(&second))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_the_challenge() {
        // Checked with Python's hashlib.pbkdf2_hmac.
        assert_eq!(
            response(crate::PASSWORD),
            "c0ffee55$e3850e0cc988d0275516b6399262de94826857caefc0180798e5df870d725bbf"
        );
    }

    #[test]
    fn allows_only_get_and_post() {
        let mut web = Web::default();
        let options = Options::default();
        let response = web.login(&Method::PUT, "", &options, false);
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let response = web.api(&HashMap::new(), &Method::POST, "/api/v0", &HeaderMap::new());
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let response = web.page(&HashMap::new(), &[], &Method::GET, "");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let response = web.stats(&HashMap::new(), &Method::GET, "");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn serves_pages_only_in_the_session() {
        let mut web = Web::default();
        let pages = HashMap::from([("/ecoStat".to_owned(), "{}".to_owned())]);
        let form = format!("page=ecoStat&sid={SESSION}");
        let response = web.page(&pages, &[], &Method::POST, &form);
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        web.open = true;
        let response = web.page(&pages, &[], &Method::POST, "page=ecoStat&sid=0");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = web.page(&pages, &[], &Method::POST, &form);
        assert_eq!(response.status(), StatusCode::OK);
        let other = format!("page=other&sid={SESSION}");
        let response = web.page(&pages, &[], &Method::POST, &other);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// Posts `page` with the extra form fields `fields`: the status and
    /// the body of the answer.
    async fn post(
        web: &mut Web,
        pages: &HashMap<String, String>,
        injections: &[Injection],
        fields: &str,
    ) -> (StatusCode, String) {
        let form = format!("sid={SESSION}&{fields}");
        let response = web.page(pages, injections, &Method::POST, &form);
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn remembers_the_filter_of_the_log() {
        let log = r#"{"data": {"log": [
            {"msg": "a", "group": "net"},
            ["d", "t", "b", 1, "usb"],
            {"msg": "c", "group": 2}
        ]}}"#;
        let pages = HashMap::from([
            ("/log".to_owned(), log.to_owned()),
            ("/odd".to_owned(), "[1]".to_owned()),
        ]);
        let mut web = Web::new(&pages);
        assert_eq!(web.log_filter(), None);
        web.open = true;
        let json =
            |answer: (StatusCode, String)| -> Value { serde_json::from_str(&answer.1).unwrap() };
        // Without a filter, the page is the one of the fixture.
        assert_eq!(post(&mut web, &pages, &[], "page=log").await.1, log);
        let page = json(post(&mut web, &pages, &[], "page=log&filter=usb").await);
        assert_eq!(page["data"]["filter"], "usb");
        assert_eq!(page["data"]["log"].as_array().unwrap().len(), 1);
        let page = json(post(&mut web, &pages, &[], "page=log").await);
        assert_eq!(page["data"]["filter"], "usb");
        let page = json(post(&mut web, &pages, &[], "page=log&filter=all").await);
        assert_eq!(page["data"]["log"].as_array().unwrap().len(), 3);
        // A failed answer still changes the filter.
        let fail = [Injection::fail_page("log", "filter", "net")];
        let (status, _) = post(&mut web, &pages, &fail, "page=log&filter=net").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(web.log_filter(), Some("net"));
        let (status, _) = post(&mut web, &pages, &fail, "page=log&filter=sys").await;
        assert_eq!(status, StatusCode::OK);
        let odd = post(&mut web, &pages, &fail, "page=odd&filter=net").await;
        assert_eq!(odd, (StatusCode::OK, "[1]".to_owned()));
        assert_eq!(web.log_filter(), Some("sys"));
    }

    #[tokio::test]
    async fn keeps_a_log_page_without_data() {
        let pages = HashMap::from([("/log".to_owned(), "[]".to_owned())]);
        let mut web = Web::new(&pages);
        web.open = true;
        let answer = post(&mut web, &pages, &[], "page=log&filter=net").await;
        assert_eq!(answer, (StatusCode::OK, "[]".to_owned()));
        assert_eq!(web.log_filter(), Some("net"));
    }

    #[test]
    fn takes_the_filter_of_the_fixture() {
        let pages = |json: &str| HashMap::from([("/log".to_owned(), json.to_owned())]);
        let web = Web::new(&pages(r#"{"data": {"filter": "net", "log": []}}"#));
        assert_eq!(web.log_filter(), Some("net"));
        assert_eq!(Web::new(&pages("<html>")).log_filter(), None);
        assert_eq!(Web::new(&HashMap::new()).log_filter(), None);
    }

    #[test]
    fn serves_only_statistics() {
        let mut web = Web::default();
        let stats = HashMap::from([("116300000001".to_owned(), "<devicestats/>".to_owned())]);
        let form =
            |command: &str, ain: &str| format!("switchcmd={command}&ain={ain}&sid={SESSION}");
        let get = form("getbasicdevicestats", "11630+0000001");
        let response = web.stats(&stats, &Method::POST, &get);
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        web.open = true;
        let response = web.stats(&stats, &Method::POST, &get);
        assert_eq!(response.status(), StatusCode::OK);
        let response = web.stats(&stats, &Method::POST, &form("setswitchoff", "116300000001"));
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = web.stats(&stats, &Method::POST, &form("getbasicdevicestats", "1"));
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
