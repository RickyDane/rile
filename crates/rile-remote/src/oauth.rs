//! OAuth 2.0 sign-in and token refresh for Google Drive and OneDrive.
//!
//! Users bring their own OAuth app (a Google Cloud "Desktop app" client, or a
//! Microsoft Entra public-client app with the `http://localhost` redirect URI).
//! [`begin`] + [`Pending::finish`] run the authorization-code flow with PKCE through the system
//! browser and a one-shot loopback HTTP listener. The resulting refresh token
//! (plus the Google client secret) is serialized as the connection's secret
//! and stored in the keychain like a password. Rotated refresh tokens are
//! written back to the keychain automatically.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::{Auth, Cancelled, ConnectionInfo, ConnectionLost, Protocol};

/// How long [`Pending::finish`] waits for the browser to come back.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

struct Provider {
    auth_url: &'static str,
    token_url: &'static str,
    scope: &'static str,
    /// Host in the loopback redirect URI.
    redirect_host: &'static str,
    extra: &'static [(&'static str, &'static str)],
    /// Microsoft wants the scope repeated on token requests.
    scope_on_token: bool,
}

const GOOGLE: Provider = Provider {
    auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
    token_url: "https://oauth2.googleapis.com/token",
    scope: "https://www.googleapis.com/auth/drive",
    redirect_host: "127.0.0.1",
    // `prompt=consent` makes Google return a refresh token every time.
    extra: &[("access_type", "offline"), ("prompt", "consent")],
    scope_on_token: false,
};

const MICROSOFT: Provider = Provider {
    auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
    token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
    scope: "offline_access Files.ReadWrite.All User.Read",
    // Entra only allows plain-http redirects to `localhost` (any port).
    redirect_host: "localhost",
    extra: &[("response_mode", "query"), ("prompt", "select_account")],
    scope_on_token: true,
};

fn provider(protocol: Protocol) -> Result<&'static Provider> {
    match protocol {
        Protocol::GoogleDrive => Ok(&GOOGLE),
        Protocol::OneDrive => Ok(&MICROSOFT),
        p => bail!("{} doesn't use OAuth", p.label()),
    }
}

/// What's stored in the keychain for a cloud connection.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
struct CloudSecret {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    refresh_token: String,
}

impl CloudSecret {
    fn parse(secret: &str) -> Result<Self> {
        serde_json::from_str(secret).map_err(|_| anyhow!("the saved sign-in is unreadable; edit the connection and sign in again"))
    }
    fn encode(&self) -> String {
        serde_json::to_string(self).expect("plain struct")
    }
}

/// Result of a successful [`Pending::finish`].
#[derive(Clone, Debug)]
pub struct SignedIn {
    /// Pass as the connection secret (and save it in the keychain).
    pub secret: String,
    /// The account's e-mail address (or name), for [`ConnectionInfo::username`].
    pub account: String,
}

/// A started browser sign-in: show [`Pending::url`] to the user, then call
/// [`Pending::finish`] on a background thread.
pub struct Pending {
    pub url: String,
    protocol: Protocol,
    client_id: String,
    client_secret: Option<String>,
    redirect_uri: String,
    verifier: String,
    state: String,
    listeners: Vec<TcpListener>,
}

/// Start the browser sign-in for `protocol` with the user's OAuth app: opens a
/// loopback port for the redirect and builds the authorization URL (PKCE).
pub fn begin(protocol: Protocol, client_id: &str, client_secret: Option<&str>) -> Result<Pending> {
    let p = provider(protocol)?;
    let client_id = client_id.trim();
    if client_id.is_empty() {
        bail!("Enter the OAuth client ID");
    }
    let client_secret = client_secret.map(str::trim).filter(|s| !s.is_empty());
    if protocol == Protocol::GoogleDrive && client_secret.is_none() {
        bail!("Enter the OAuth client secret");
    }
    // Browsers may resolve `localhost` to either address family.
    let v4 = TcpListener::bind("127.0.0.1:0").context("couldn't open a local port for the sign-in")?;
    let port = v4.local_addr()?.port();
    let listeners: Vec<TcpListener> = std::iter::once(v4).chain(TcpListener::bind(("::1", port)).ok()).collect();
    for l in &listeners {
        l.set_nonblocking(true)?;
    }
    let redirect_uri = format!("http://{}:{port}", p.redirect_host);
    let verifier = random_token(48);
    let state = random_token(24);
    let challenge = URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()));
    let mut params = vec![
        ("client_id", client_id),
        ("redirect_uri", redirect_uri.as_str()),
        ("response_type", "code"),
        ("scope", p.scope),
        ("state", state.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ];
    params.extend_from_slice(p.extra);
    let url = url::Url::parse_with_params(p.auth_url, &params)?.to_string();
    Ok(Pending {
        url,
        protocol,
        client_id: client_id.to_string(),
        client_secret: client_secret.map(str::to_string),
        redirect_uri,
        verifier,
        state,
        listeners,
    })
}

impl Pending {
    /// Wait for the browser to come back and exchange the code for tokens.
    /// Blocks until then, `cancel` is set (→ [`Cancelled`]) or five minutes pass.
    pub fn finish(self, cancel: &AtomicBool) -> Result<SignedIn> {
        let p = provider(self.protocol)?;
        let code = self.wait_for_code(cancel)?;
        let Pending { protocol, client_id, client_secret, redirect_uri, verifier, .. } = self;
        let client_id = client_id.as_str();
        let client_secret = client_secret.as_deref();
        let agent = agent(true);
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id),
            ("code_verifier", verifier.as_str()),
        ];
        if let Some(s) = client_secret {
            form.push(("client_secret", s));
        }
        if p.scope_on_token {
            form.push(("scope", p.scope));
        }
        let tokens = token_request(&agent, p, &form)?;
        let refresh_token = tokens.refresh_token.ok_or_else(|| anyhow!("The provider didn't return a refresh token"))?;
        let account = account_name(&agent, protocol, &tokens.access_token).unwrap_or_else(|e| {
            log::warn!("couldn't read the {} account name: {e:#}", protocol.label());
            String::new()
        });
        let secret = CloudSecret { client_secret: client_secret.map(str::to_string), refresh_token }.encode();
        Ok(SignedIn { secret, account })
    }

    fn wait_for_code(&self, cancel: &AtomicBool) -> Result<String> {
        let deadline = Instant::now() + SIGN_IN_TIMEOUT;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Cancelled.into());
            }
            if Instant::now() > deadline {
                bail!("The sign-in timed out. Try again.");
            }
            let mut accepted = None;
            for l in &self.listeners {
                match l.accept() {
                    Ok((stream, _)) => {
                        accepted = Some(stream);
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e).context("sign-in listener failed"),
                }
            }
            let Some(mut stream) = accepted else {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            };
            match read_callback(&mut stream) {
                Ok(Callback::Other) => respond(&mut stream, "404 Not Found", "Not found."),
                Ok(Callback::Code { code, state }) if state == self.state => {
                    respond(&mut stream, "200 OK", "Signed in. You can close this tab and return to Rile.");
                    return Ok(code);
                }
                Ok(Callback::Code { .. }) => {
                    respond(&mut stream, "400 Bad Request", "This sign-in link is out of date. Start again from Rile.");
                }
                Ok(Callback::Error(message)) => {
                    respond(&mut stream, "200 OK", &format!("Sign-in failed: {message}. You can close this tab."));
                    bail!("Sign-in failed: {message}");
                }
                Err(e) => log::debug!("ignoring malformed sign-in request: {e:#}"),
            }
        }
    }
}

/// The client secret saved with a cloud sign-in (Google), to sign in again
/// without retyping it.
pub fn saved_client_secret(secret: &str) -> Option<String> {
    CloudSecret::parse(secret).ok()?.client_secret
}

fn random_token(bytes: usize) -> String {
    use ring::rand::SecureRandom;
    let mut buf = vec![0u8; bytes];
    ring::rand::SystemRandom::new().fill(&mut buf).expect("system randomness");
    URL_SAFE_NO_PAD.encode(buf)
}

#[derive(Debug, PartialEq)]
enum Callback {
    Code { code: String, state: String },
    Error(String),
    /// Anything else (e.g. `/favicon.ico`).
    Other,
}

fn read_callback(stream: &mut TcpStream) -> Result<Callback> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 64 * 1024 {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    parse_callback(&String::from_utf8_lossy(&buf))
}

fn parse_callback(request: &str) -> Result<Callback> {
    let line = request.lines().next().unwrap_or_default();
    let mut parts = line.split(' ');
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        bail!("not a GET request");
    };
    let url = url::Url::parse(&format!("http://localhost{target}"))?;
    let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
    if let Some(error) = query.get("error") {
        return Ok(Callback::Error(query.get("error_description").unwrap_or(error).clone()));
    }
    match (query.get("code"), query.get("state")) {
        (Some(code), Some(state)) => Ok(Callback::Code { code: code.clone(), state: state.clone() }),
        _ => Ok(Callback::Other),
    }
}

fn respond(stream: &mut TcpStream, status: &str, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Rile</title>\
         <body style=\"font:16px -apple-system,system-ui,sans-serif;margin:4em auto;max-width:32em\">\
         <h2>Rile</h2><p>{}</p>",
        message.replace('&', "&amp;").replace('<', "&lt;")
    );
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    _ = stream.write_all(reply.as_bytes());
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// The saved sign-in no longer works; the user must sign in again.
#[derive(Debug, Clone)]
pub struct SignInExpired(pub String);

impl std::fmt::Display for SignInExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}. Edit the connection and sign in again.", self.0)
    }
}
impl std::error::Error for SignInExpired {}

fn token_request(agent: &ureq::Agent, p: &Provider, form: &[(&str, &str)]) -> Result<TokenResponse> {
    let mut resp = agent.post(p.token_url).send_form(form.iter().copied()).map_err(|e| ConnectionLost(e.to_string()))?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| ConnectionLost(e.to_string()))?;
    if (200..300).contains(&status) {
        return serde_json::from_str(&body).context("unexpected token response");
    }
    match serde_json::from_str::<TokenError>(&body) {
        Ok(e) if matches!(e.error.as_str(), "invalid_grant" | "invalid_client" | "unauthorized_client") => {
            let detail = e.error_description.unwrap_or(e.error);
            Err(SignInExpired(format!("The sign-in was rejected ({})", detail.lines().next().unwrap_or_default())).into())
        }
        Ok(e) => bail!("token request failed: {}", e.error_description.unwrap_or(e.error)),
        Err(_) => bail!("token request failed (HTTP {status})"),
    }
}

fn account_name(agent: &ureq::Agent, protocol: Protocol, access: &str) -> Result<String> {
    let url = match protocol {
        Protocol::GoogleDrive => "https://www.googleapis.com/drive/v3/about?fields=user(emailAddress,displayName)",
        _ => "https://graph.microsoft.com/v1.0/me?$select=mail,userPrincipalName,displayName",
    };
    let mut resp = agent.get(url).header("Authorization", format!("Bearer {access}")).call()?;
    let v: serde_json::Value = serde_json::from_str(&resp.body_mut().read_to_string()?)?;
    let pick = |v: &serde_json::Value, keys: &[&str]| {
        keys.iter().find_map(|k| v.get(*k).and_then(|s| s.as_str()).filter(|s| !s.is_empty()).map(str::to_string))
    };
    Ok(match protocol {
        Protocol::GoogleDrive => pick(&v["user"], &["emailAddress", "displayName"]),
        _ => pick(&v, &["mail", "userPrincipalName", "displayName"]),
    }
    .unwrap_or_default())
}

/// HTTP agent for the cloud APIs. Status codes are handled by the caller.
/// Authorization headers are never forwarded on redirects (ureq's default).
pub(crate) fn agent(follow_redirects: bool) -> ureq::Agent {
    let mut cfg = ureq::Agent::config_builder()
        .user_agent(concat!("Rile/", env!("CARGO_PKG_VERSION")))
        .http_status_as_error(false)
        .timeout_connect(Some(crate::CONNECT_TIMEOUT))
        .timeout_recv_response(Some(Duration::from_secs(120)))
        .timeout_per_read(Some(Duration::from_secs(60)));
    if !follow_redirects {
        // Drive answers 308 to resumable upload chunks; that's not a redirect.
        cfg = cfg.max_redirects(0).max_redirects_will_error(false);
    }
    cfg.build().into()
}

// ---------------------------------------------------------------- tokens

/// Token state for one connection, shared by all of its sessions in this process.
pub(crate) struct Tokens {
    protocol: Protocol,
    conn_id: String,
    client_id: String,
    secret: CloudSecret,
    /// Secrets (keychain values) known to describe this state, so a session
    /// opened with an older value doesn't reset the rotated refresh token.
    known: Vec<String>,
    access: Option<(String, Instant)>,
}

pub(crate) type SharedTokens = Arc<Mutex<Tokens>>;

fn registry() -> &'static Mutex<HashMap<String, SharedTokens>> {
    static R: OnceLock<Mutex<HashMap<String, SharedTokens>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Token state for `info`: reused across sessions unless `secret` is a new sign-in.
pub(crate) fn tokens_for(info: &ConnectionInfo, secret: Option<&str>) -> Result<SharedTokens> {
    let Auth::OAuth { client_id } = &info.auth else {
        bail!("{} connections must be signed in with OAuth", info.protocol.label());
    };
    provider(info.protocol)?;
    let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = reg.get(&info.id) {
        let t = existing.lock().unwrap_or_else(|e| e.into_inner());
        if t.client_id == *client_id && t.protocol == info.protocol && secret.is_none_or(|s| t.known.iter().any(|k| k == s)) {
            drop(t);
            return Ok(existing.clone());
        }
    }
    let secret_str = secret.ok_or_else(|| SignInExpired(format!("{} isn't signed in", info.protocol.label())))?;
    let tokens = Arc::new(Mutex::new(Tokens {
        protocol: info.protocol,
        conn_id: info.id.clone(),
        client_id: client_id.clone(),
        secret: CloudSecret::parse(secret_str)?,
        known: vec![secret_str.to_string()],
        access: None,
    }));
    reg.insert(info.id.clone(), tokens.clone());
    Ok(tokens)
}

/// A valid access token; refreshes it when it is about to expire or `force`.
pub(crate) fn access_token(tokens: &SharedTokens, agent: &ureq::Agent, force: bool) -> Result<String> {
    let mut t = tokens.lock().unwrap_or_else(|e| e.into_inner());
    if !force && let Some((token, expires)) = &t.access && *expires > Instant::now() + Duration::from_secs(60) {
        return Ok(token.clone());
    }
    let p = provider(t.protocol)?;
    let refresh = t.secret.refresh_token.clone();
    let client_id = t.client_id.clone();
    let client_secret = t.secret.client_secret.clone();
    let mut form = vec![("grant_type", "refresh_token"), ("refresh_token", refresh.as_str()), ("client_id", client_id.as_str())];
    if let Some(s) = &client_secret {
        form.push(("client_secret", s));
    }
    if p.scope_on_token {
        form.push(("scope", p.scope));
    }
    let resp = token_request(agent, p, &form)?;
    let expires = Instant::now() + Duration::from_secs(resp.expires_in.unwrap_or(3600));
    t.access = Some((resp.access_token.clone(), expires));
    if let Some(new) = resp.refresh_token.filter(|r| *r != t.secret.refresh_token) {
        // Microsoft rotates refresh tokens; keep the newest one in the keychain.
        t.secret.refresh_token = new;
        let encoded = t.secret.encode();
        if let Err(e) = crate::credentials::save(&t.conn_id, &encoded) {
            log::warn!("couldn't save the refreshed sign-in: {e:#}");
        }
        t.known.push(encoded);
    }
    Ok(resp.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_callbacks() {
        assert_eq!(
            parse_callback("GET /?code=4%2Fabc&state=xyz&scope=drive HTTP/1.1\r\nHost: x\r\n\r\n").unwrap(),
            Callback::Code { code: "4/abc".into(), state: "xyz".into() }
        );
        assert_eq!(
            parse_callback("GET /?error=access_denied&error_description=User+cancelled HTTP/1.1\r\n\r\n").unwrap(),
            Callback::Error("User cancelled".into())
        );
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1\r\n\r\n").unwrap(), Callback::Other);
        assert!(parse_callback("POST / HTTP/1.1\r\n\r\n").is_err());
    }

    #[test]
    fn loopback_receives_the_code() {
        assert!(begin(Protocol::GoogleDrive, "app", None).is_err(), "Google needs a client secret");
        let pending = begin(Protocol::OneDrive, " app ", None).unwrap();
        let url = url::Url::parse(&pending.url).unwrap();
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "app");
        assert_eq!(q["code_challenge_method"], "S256");
        assert!(q["scope"].contains("offline_access"));
        let redirect = url::Url::parse(&q["redirect_uri"]).unwrap();
        assert_eq!(redirect.host_str(), Some("localhost"));
        let port = redirect.port().unwrap();
        let state = q["state"].clone();
        let client = std::thread::spawn(move || {
            let get = |target: &str| {
                let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(s, "GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
                let mut reply = String::new();
                s.read_to_string(&mut reply).unwrap();
                reply
            };
            assert!(get("/favicon.ico").starts_with("HTTP/1.1 404"));
            assert!(get("/?code=c&state=wrong").starts_with("HTTP/1.1 400"));
            assert!(get(&format!("/?code=the-code&state={state}")).contains("Signed in"));
        });
        assert_eq!(pending.wait_for_code(&AtomicBool::new(false)).unwrap(), "the-code");
        client.join().unwrap();
        let cancelled = begin(Protocol::OneDrive, "app", None).unwrap().finish(&AtomicBool::new(true)).unwrap_err();
        assert!(crate::is_cancelled(&cancelled));
    }

    #[test]
    fn secret_roundtrip() {
        let s = CloudSecret { client_secret: None, refresh_token: "r".into() };
        assert_eq!(s.encode(), r#"{"refresh_token":"r"}"#);
        assert_eq!(CloudSecret::parse(&s.encode()).unwrap(), s);
        assert!(CloudSecret::parse("hunter2").is_err());
    }

    #[test]
    fn pkce_tokens_are_url_safe() {
        let v = random_token(48);
        assert_eq!(v.len(), 64);
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(v, random_token(48));
    }

    #[test]
    fn sessions_share_tokens_until_a_new_sign_in() {
        let mut info = crate::ConnectionInfo {
            id: crate::ConnectionInfo::new_id(),
            name: String::new(),
            protocol: Protocol::OneDrive,
            host: String::new(),
            port: 443,
            username: String::new(),
            auth: Auth::OAuth { client_id: "app".into() },
            initial_path: None,
        };
        let first = r#"{"refresh_token":"a"}"#;
        let a = tokens_for(&info, Some(first)).unwrap();
        assert!(Arc::ptr_eq(&a, &tokens_for(&info, Some(first)).unwrap()));
        assert!(Arc::ptr_eq(&a, &tokens_for(&info, None).unwrap()));
        let b = tokens_for(&info, Some(r#"{"refresh_token":"b"}"#)).unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
        info.auth = Auth::OAuth { client_id: "other".into() };
        assert!(tokens_for(&info, None).is_err());
    }
}
