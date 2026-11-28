//! The HTTP side: fetching documents and calling actions.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue, WWW_AUTHENTICATE};
use reqwest::{Method, RequestBuilder, Response, StatusCode, Url, redirect};
use secrecy::{ExposeSecret as _, SecretString};

use crate::digest::{Challenge, DigestAuth};
use crate::logging::{self, HideValues, Redact};
use crate::tls::PinnedVerifier;
use crate::{
    Arguments, DescriptionCache, DeviceDescription, Error, Fingerprint, Scpd, Service, soap,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A TR-064 client for one device.
#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    verifier: Arc<PinnedVerifier>,
    base: Url,
    credentials: Option<(String, SecretString)>,
    /// The answer to the last challenge, reused until the device rejects it.
    auth: Mutex<Option<DigestAuth>>,
    redact: Arc<dyn Redact>,
    cache: Option<Arc<dyn DescriptionCache>>,
    /// The device description as last fetched, for the cache.
    description: Mutex<Option<Arc<str>>>,
}

/// Configures a [`Client`].
#[derive(Debug)]
pub struct ClientBuilder {
    base: Url,
    credentials: Option<(String, SecretString)>,
    timeout: Duration,
    pin: Option<Fingerprint>,
    redact: Arc<dyn Redact>,
    cache: Option<Arc<dyn DescriptionCache>>,
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

    /// The fingerprint of the device's certificate. Required for `https`, and
    /// the only certificate the client accepts. See [`fetch_fingerprint`] to
    /// get it on first use.
    ///
    /// [`fetch_fingerprint`]: crate::fetch_fingerprint
    #[must_use]
    pub fn pin(mut self, fingerprint: Fingerprint) -> Self {
        self.pin = Some(fingerprint);
        self
    }

    /// How values appear in the log, see [`Redact`]. By default, every
    /// value is hidden.
    #[must_use]
    pub fn redact(mut self, redact: Arc<dyn Redact>) -> Self {
        self.redact = redact;
        self
    }

    /// A cache for the service descriptions, see [`DescriptionCache`].
    #[must_use]
    pub fn cache(mut self, cache: Arc<dyn DescriptionCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Builds the client.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidUrl`] if the base URL is `https` without a pinned
    /// certificate, `http` with one, or neither, and [`Error::Http`] if the
    /// HTTP client cannot be set up.
    pub fn build(self) -> Result<Client, Error> {
        match (self.base.scheme(), self.pin) {
            ("https", Some(_)) | ("http", None) => {}
            ("https", None) => {
                return Err(Error::InvalidUrl(format!(
                    "{} needs a pinned certificate",
                    self.base
                )));
            }
            _ => {
                return Err(Error::InvalidUrl(format!(
                    "{} is neither https with a pinned certificate nor http",
                    self.base
                )));
            }
        }
        let verifier = PinnedVerifier::new(self.pin);
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(verifier.config())
            .timeout(self.timeout)
            .redirect(redirect::Policy::none())
            .no_proxy()
            .build()?;
        Ok(Client {
            http,
            verifier,
            base: self.base,
            credentials: self.credentials,
            auth: Mutex::new(None),
            redact: self.redact,
            cache: self.cache,
            description: Mutex::new(None),
        })
    }
}

impl Client {
    /// Starts configuring a client for the device at `base`, for example
    /// `https://192.168.178.1:49443` with a [pinned](ClientBuilder::pin)
    /// certificate, or `http://192.168.178.1:49000`.
    pub fn builder(base: Url) -> ClientBuilder {
        ClientBuilder {
            base,
            credentials: None,
            timeout: DEFAULT_TIMEOUT,
            pin: None,
            redact: Arc::new(HideValues),
            cache: None,
        }
    }

    /// Fetches a document of the device, relative to the base URL.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidUrl`] if `path` points to another host, and the errors
    /// of a request.
    pub async fn get(&self, path: &str) -> Result<String, Error> {
        self.get_with_headers(path, &[]).await
    }

    /// Fetches a document of the device like [`get`](Self::get), with
    /// additional request headers, for example a session token of another
    /// interface of the device.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidUrl`] if `path` points to another host,
    /// [`Error::Http`] if a header name or value is invalid, and the errors
    /// of a request.
    pub async fn get_with_headers(
        &self,
        path: &str,
        headers: &[(&str, &str)],
    ) -> Result<String, Error> {
        let url = self.url(path)?;
        let request = headers
            .iter()
            .fold(self.http.get(url.clone()), |request, (name, value)| {
                request.header(*name, *value)
            });
        // Reports an invalid header now, before the request is cloned.
        let (http, request) = request.build_split();
        let request = RequestBuilder::from_parts(http, request?);
        let response = self.send(request, &Method::GET, &url, None).await?;
        self.document(response).await
    }

    /// Posts a form, `application/x-www-form-urlencoded`, to a path of the
    /// device and returns the body of the response. Some devices offer more
    /// than TR-064 on the same server, like a login page.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidUrl`] if `path` points to another host, and the errors
    /// of a request.
    pub async fn post_form(&self, path: &str, form: &[(&str, &str)]) -> Result<String, Error> {
        let url = self.url(path)?;
        let body = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let request = self
            .http
            .post(url.clone())
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body);
        let response = self.send(request, &Method::POST, &url, None).await?;
        self.document(response).await
    }

    /// Fetches and parses the device description `/tr64desc.xml`.
    ///
    /// # Errors
    ///
    /// The errors of a request and of [`DeviceDescription::parse`].
    pub async fn description(&self) -> Result<DeviceDescription, Error> {
        let xml = self.get("/tr64desc.xml").await?;
        let description = DeviceDescription::parse(&xml)?;
        if self.cache.is_some() {
            *lock(&self.description) = Some(xml.into());
        }
        Ok(description)
    }

    /// Fetches and parses the SCPD of a service. With a
    /// [cache](ClientBuilder::cache), and after a
    /// [device description](Self::description) was fetched, the cache is
    /// asked first.
    ///
    /// # Errors
    ///
    /// The errors of a request and of [`Scpd::parse`].
    pub async fn scpd(&self, service: &Service) -> Result<Scpd, Error> {
        let url = &service.scpd_url;
        let cache = self
            .cache
            .as_ref()
            .and_then(|cache| Some((cache, lock(&self.description).clone()?)));
        if let Some((cache, description)) = &cache
            && let Some(scpd) = cache.scpd(description, url)
        {
            return Ok(scpd);
        }
        let xml = self.get(url).await?;
        let scpd = Scpd::parse(&xml)?;
        if let Some((cache, description)) = &cache {
            cache.store(description, url, &xml, &scpd);
        }
        Ok(scpd)
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
        if tracing::enabled!(tracing::Level::TRACE) {
            let shown =
                logging::arguments(&*self.redact, service, action, arguments.iter().copied());
            tracing::trace!(action, arguments = %shown, "call");
        }
        let soap_action = format!("\"{}#{action}\"", service.service_type);
        let request = self
            .http
            .post(url.clone())
            .header("SOAPAction", soap_action)
            .header(CONTENT_TYPE, r#"text/xml; charset="utf-8""#)
            .body(body);
        let response = self
            .send(request, &Method::POST, &url, Some((service, action)))
            .await?;
        let out = soap::parse_response(&text(response).await?, action)?;
        if tracing::enabled!(tracing::Level::TRACE) {
            let shown = logging::arguments(&*self.redact, service, action, out.iter());
            tracing::trace!(action, arguments = %shown, "answer");
        }
        Ok(out)
    }

    /// The body of a document. Only its size is logged: documents can hold
    /// secrets and personal data the client knows nothing about.
    async fn document(&self, response: Response) -> Result<String, Error> {
        let body = text(response).await?;
        tracing::trace!(bytes = body.len(), "document");
        Ok(body)
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

    /// Sends a request and logs it: the action, if it is a call, the URL
    /// with redacted values, and the status or error.
    async fn send(
        &self,
        request: RequestBuilder,
        method: &Method,
        url: &Url,
        call: Option<(&Service, &str)>,
    ) -> Result<Response, Error> {
        let start = Instant::now();
        let result = self.authorized(request, method, url).await;
        let elapsed_ms = start.elapsed().as_millis();
        let action = call.map(|(service, action)| format!("{}#{action}", service.service_type));
        let shown = logging::url(url, &*self.redact);
        match &result {
            Ok(response) => {
                let status = response.status().as_u16();
                tracing::info!(%method, url = %shown, action, status, elapsed_ms, "request");
            }
            Err(error) => {
                let error = logging::error(error);
                tracing::info!(%method, url = %shown, action, %error, elapsed_ms, "request failed");
            }
        }
        let response = result?;
        check_status(response, url).await
    }

    /// Sends a request with authorization. A cached authorization is reused; it only gets there
    /// after the device accepted it. If the device rejects it, the nonce has
    /// expired, so the client answers the new challenge once. If the device
    /// rejects a fresh answer, the credentials are wrong and the client stops,
    /// so that a wrong password does not lock the account.
    async fn authorized(
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
        if cached.is_some() {
            tracing::debug!("answering the previous Digest challenge again");
        }
        let mut response = self.execute(with_authorization(&request, cached)).await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            *self.lock() = None;
            let (auth, header) = self.answer(&response, method, &uri)?;
            response = self
                .execute(with_authorization(&request, Some(header)))
                .await?;
            if response.status() == StatusCode::UNAUTHORIZED {
                tracing::debug!("the device rejected the Digest answer, not trying again");
                return Err(Error::AuthenticationFailed);
            }
            tracing::debug!("the device accepted the Digest answer");
            *self.lock() = Some(auth);
        }
        Ok(response)
    }

    /// Sends one request, reporting a certificate mismatch as such.
    async fn execute(&self, request: RequestBuilder) -> Result<Response, Error> {
        request.send().await.map_err(|e| self.verifier.explain(e))
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
        let challenge = challenge(response.headers().get_all(WWW_AUTHENTICATE))?;
        tracing::debug!(%challenge, "the device asks for Digest authentication");
        let mut auth = DigestAuth::new(challenge);
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
        lock(&self.auth)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    let body = text(response).await.unwrap_or_default();
    if status == StatusCode::INTERNAL_SERVER_ERROR
        && let Some(fault) = soap::parse_fault(&body)
    {
        return Err(fault);
    }
    Err(Error::Status {
        status: status.as_u16(),
        url: url.to_string(),
    })
}

/// The body of a response as text. A body without a length ends with the
/// connection, and devices like the FRITZ!Box close a TLS connection
/// without sending `close_notify` first. That counts as the end of such a
/// body. A body with a length that ends early is still an error.
async fn text(mut response: Response) -> Result<String, Error> {
    let delimited = response.content_length().is_none();
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(error) if delimited && ends_early(&error) => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Whether an error, or one of its causes, is an unexpected end of the
/// connection.
fn ends_early(error: &(dyn std::error::Error + 'static)) -> bool {
    std::iter::successors(Some(error), |e| e.source()).any(|e| {
        e.downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::UnexpectedEof)
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
