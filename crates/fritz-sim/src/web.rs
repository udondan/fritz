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
use sha2::Sha256;

use crate::{Options, SESSION, XML};

/// The login page.
pub(crate) const LOGIN: &str = "/login_sid.lua";
/// The prefix of the REST API.
pub(crate) const API: &str = "/api/";
/// The pages of the web interface, like `ecoStat`.
pub(crate) const DATA: &str = "/data.lua";
/// The Smart Home HTTP interface, for statistics.
pub(crate) const HOMEAUTO: &str = "/webservices/homeautoswitch.lua";

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

/// Whether the session is open.
#[derive(Debug, Default)]
pub(crate) struct Web {
    open: bool,
}

impl Web {
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
    /// field `page`, if the field `sid` carries the open session.
    pub(crate) fn page(
        &self,
        pages: &HashMap<String, String>,
        method: &Method,
        body: &str,
    ) -> Response {
        let form = match self.form(method, body) {
            Ok(form) => form,
            Err(code) => return code.into_response(),
        };
        match form
            .get("page")
            .and_then(|page| pages.get(&format!("/{page}")))
        {
            Some(json) => ([(CONTENT_TYPE, "application/json")], json.clone()).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
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
        let response = web.page(&HashMap::new(), &Method::GET, "");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let response = web.stats(&HashMap::new(), &Method::GET, "");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn serves_pages_only_in_the_session() {
        let mut web = Web::default();
        let pages = HashMap::from([("/ecoStat".to_owned(), "{}".to_owned())]);
        let form = format!("page=ecoStat&sid={SESSION}");
        let response = web.page(&pages, &Method::POST, &form);
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        web.open = true;
        let response = web.page(&pages, &Method::POST, "page=ecoStat&sid=0");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = web.page(&pages, &Method::POST, &form);
        assert_eq!(response.status(), StatusCode::OK);
        let response = web.page(&pages, &Method::POST, &format!("page=other&sid={SESSION}"));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
