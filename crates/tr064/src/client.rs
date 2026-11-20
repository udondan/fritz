//! The HTTP side: fetching documents and calling actions.

use std::sync::Mutex;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue, WWW_AUTHENTICATE};
use reqwest::{Method, RequestBuilder, Response, StatusCode, Url, redirect};
use secrecy::{ExposeSecret as _, SecretString};

use crate::digest::{Challenge, DigestAuth};
use crate::{Arguments, DeviceDescription, Error, Scpd, Service, soap};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A TR-064 client for one device.
#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
    credentials: Option<(String, SecretString)>,
    /// The answer to the last challenge, reused until the device rejects it.
    auth: Mutex<Option<DigestAuth>>,
}

/// Configures a [`Client`].
#[derive(Debug)]
pub struct ClientBuilder {
    base: Url,
    credentials: Option<(String, SecretString)>,
    timeout: Duration,
}

impl ClientBuilder {
    /// The user name and password for HTTP Digest authentication.
    #[must_use]
    pub fn credentials(mut self, username: impl Into<String>, password: SecretString) -> Self {
        self.credentials = Some((username.into(), password));
        self
    }

    /// The timeout of each request, 10 seconds by default.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Builds the client.
    ///
    /// # Errors
    ///
    /// [`Error::Http`] if the HTTP client cannot be set up.
    pub fn build(self) -> Result<Client, Error> {
        let http = reqwest::Client::builder()
            .timeout(self.timeout)
            .redirect(redirect::Policy::none())
            .no_proxy()
            .build()?;
        Ok(Client {
            http,
            base: self.base,
            credentials: self.credentials,
            auth: Mutex::new(None),
        })
    }
}

impl Client {
    /// Starts configuring a client for the device at `base`, for example
    /// `http://192.168.178.1:49000`.
    pub fn builder(base: Url) -> ClientBuilder {
        ClientBuilder {
            base,
            credentials: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Fetches a document of the device, relative to the base URL.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidUrl`] if `path` points to another host, and the errors
    /// of a request.
    pub async fn get(&self, path: &str) -> Result<String, Error> {
        let url = self.url(path)?;
        let response = self
            .send(self.http.get(url.clone()), &Method::GET, &url)
            .await?;
        Ok(response.text().await?)
    }

    /// Fetches and parses the device description `/tr64desc.xml`.
    ///
    /// # Errors
    ///
    /// The errors of a request and of [`DeviceDescription::parse`].
    pub async fn description(&self) -> Result<DeviceDescription, Error> {
        DeviceDescription::parse(&self.get("/tr64desc.xml").await?)
    }

    /// Fetches and parses the SCPD of a service.
    ///
    /// # Errors
    ///
    /// The errors of a request and of [`Scpd::parse`].
    pub async fn scpd(&self, service: &Service) -> Result<Scpd, Error> {
        Scpd::parse(&self.get(&service.scpd_url).await?)
    }

    /// Calls an action of a service and returns its output arguments.
    ///
    /// # Errors
    ///
    /// [`Error::Upnp`] if the device answers with a UPnP error, and the errors
    /// of a request.
    pub async fn call(
        &self,
        service: &Service,
        action: &str,
        arguments: &[(&str, &str)],
    ) -> Result<Arguments, Error> {
        let url = self.url(&service.control_url)?;
        let body = soap::envelope(&service.service_type, action, arguments)?;
        let soap_action = format!("\"{}#{action}\"", service.service_type);
        let request = self
            .http
            .post(url.clone())
            .header("SOAPAction", soap_action)
            .header(CONTENT_TYPE, r#"text/xml; charset="utf-8""#)
            .body(body);
        let response = self.send(request, &Method::POST, &url).await?;
        soap::parse_response(&response.text().await?, action)
    }

    /// Resolves `path` against the base URL. Credentials must only ever go to
    /// the device, so the result must have the same origin.
    fn url(&self, path: &str) -> Result<Url, Error> {
        let url = self
            .base
            .join(path)
            .map_err(|e| Error::InvalidUrl(format!("{path:?}: {e}")))?;
        if url.origin() == self.base.origin() {
            Ok(url)
        } else {
            Err(Error::InvalidUrl(format!(
                "{url} is not on the device {}",
                self.base.origin().ascii_serialization()
            )))
        }
    }

    /// Sends a request. A cached authorization is reused; it only gets there
    /// after the device accepted it. If the device rejects it, the nonce has
    /// expired, so the client answers the new challenge once. If the device
    /// rejects a fresh answer, the credentials are wrong and the client stops,
    /// so that a wrong password does not lock the account.
    async fn send(
        &self,
        request: RequestBuilder,
        method: &Method,
        url: &Url,
    ) -> Result<Response, Error> {
        let uri = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        };
        let cached = self.cached_authorization(method, &uri);
        let mut response = with_authorization(&request, cached).send().await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            *self.lock() = None;
            let (auth, header) = self.answer(&response, method, &uri)?;
            response = with_authorization(&request, Some(header)).send().await?;
            if response.status() == StatusCode::UNAUTHORIZED {
                return Err(Error::AuthenticationFailed);
            }
            *self.lock() = Some(auth);
        }
        check_status(response, url).await
    }

    /// Answers the challenge of a 401 response.
    fn answer(
        &self,
        response: &Response,
        method: &Method,
        uri: &str,
    ) -> Result<(DigestAuth, String), Error> {
        let (username, password) = self
            .credentials
            .as_ref()
            .ok_or(Error::CredentialsRequired)?;
        let mut auth = DigestAuth::new(challenge(response.headers().get_all(WWW_AUTHENTICATE))?);
        let header = auth.authorization(username, password.expose_secret(), method.as_str(), uri);
        Ok((auth, header))
    }

    /// The `Authorization` header from the cached challenge, if there is one.
    fn cached_authorization(&self, method: &Method, uri: &str) -> Option<String> {
        let (username, password) = self.credentials.as_ref()?;
        self.lock().as_mut().map(|auth| {
            auth.authorization(username, password.expose_secret(), method.as_str(), uri)
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<DigestAuth>> {
        self.auth
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A copy of `request` with the given `Authorization` header.
fn with_authorization(request: &RequestBuilder, authorization: Option<String>) -> RequestBuilder {
    let request = request
        .try_clone()
        .expect("request bodies are held in memory");
    match authorization {
        Some(value) => request.header(AUTHORIZATION, value),
        None => request,
    }
}

/// The response if it is a success, otherwise the error it stands for.
async fn check_status(response: Response, url: &Url) -> Result<Response, Error> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let text = response.text().await.unwrap_or_default();
    if status == StatusCode::INTERNAL_SERVER_ERROR
        && let Some(fault) = soap::parse_fault(&text)
    {
        return Err(fault);
    }
    Err(Error::Status {
        status: status.as_u16(),
        url: url.to_string(),
    })
}

/// The first Digest challenge among the `WWW-Authenticate` headers, or the
/// error for the first unsupported one.
fn challenge<'a>(headers: impl IntoIterator<Item = &'a HeaderValue>) -> Result<Challenge, Error> {
    let mut first_error = None;
    for header in headers {
        match Challenge::parse(&String::from_utf8_lossy(header.as_bytes())) {
            Ok(challenge) => return Ok(challenge),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error
        .unwrap_or_else(|| Error::UnsupportedAuth("no WWW-Authenticate header".to_owned())))
}
