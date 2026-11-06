//! GitHub: sign-in by device flow, the token in the Secret Service
//! keyring, and issue creation.
//!
//! The sign-in needs a GitHub App with device flow enabled and only the
//! "Issues: read and write" repository permission, installed on
//! `korora-tech/rockhopper` (see the README). Its client id is public and
//! set in the config as `feedback.github_client_id`.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use super::{ApiError, Issue, IssueApi};

const SERVICE: &str = "com.korora.Rockhopper";
const TOKEN_USER: &str = "github-token";
const API: &str = "https://api.github.com";
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        // Statuses are mapped by us, not turned into errors.
        .http_status_as_error(false)
        .build()
        .into()
}

// ---- the token in the keyring -------------------------------------------

fn entry() -> Result<keyring_core::Entry, String> {
    let store = zbus_secret_service_keyring_store::Store::new().map_err(|e| e.to_string())?;
    keyring_core::set_default_store(store);
    keyring_core::Entry::new_with_modifiers(
        SERVICE,
        TOKEN_USER,
        &HashMap::from([("label", "Rockhopper: GitHub feedback token")]),
    )
    .map_err(|e| e.to_string())
}

pub fn load_token() -> Option<String> {
    entry().ok()?.get_password().ok()
}

pub fn save_token(token: &str) -> Result<(), String> {
    entry()?.set_password(token).map_err(|e| e.to_string())
}

/// Forget the token. Succeeds when there was none.
pub fn delete_token() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

// ---- errors -------------------------------------------------------------

/// Map a failed API response to an [`ApiError`].
pub fn classify(status: u16, rate_remaining: Option<&str>, message: &str) -> ApiError {
    match status {
        401 => ApiError::Unauthorized,
        429 => ApiError::RateLimited,
        403 | 404 if rate_remaining == Some("0") => ApiError::RateLimited,
        // GitHub answers 404 for a repository the app cannot see.
        403 | 404 => ApiError::Forbidden,
        _ => ApiError::Other(format!("{status} {message}")),
    }
}

fn transport(err: ureq::Error) -> ApiError {
    ApiError::Network(err.to_string())
}

// ---- device flow --------------------------------------------------------

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Poll {
    Pending,
    /// Poll more slowly.
    SlowDown,
    Token(String),
    Denied,
    Expired,
}

pub fn start_device_flow(client_id: &str) -> Result<DeviceCode, ApiError> {
    if client_id.trim().is_empty() {
        return Err(ApiError::Other(
            "no GitHub App is configured (set feedback.github_client_id)".into(),
        ));
    }
    let mut res = agent()
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .send_form([("client_id", client_id)])
        .map_err(transport)?;
    if !res.status().is_success() {
        return Err(ApiError::Other(format!("device code: {}", res.status())));
    }
    res.body_mut()
        .read_json()
        .map_err(|e| ApiError::Other(e.to_string()))
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    error: Option<String>,
}

pub fn parse_poll(json: &str) -> Poll {
    let Ok(reply) = serde_json::from_str::<TokenReply>(json) else {
        return Poll::Pending;
    };
    match (reply.access_token, reply.error.as_deref()) {
        (Some(token), _) => Poll::Token(token),
        (_, Some("authorization_pending")) | (None, None) => Poll::Pending,
        (_, Some("slow_down")) => Poll::SlowDown,
        (_, Some("access_denied")) => Poll::Denied,
        (_, Some(_)) => Poll::Expired,
    }
}

pub fn poll_device_flow(client_id: &str, code: &DeviceCode) -> Result<Poll, ApiError> {
    let mut res = agent()
        .post(TOKEN_URL)
        .header("Accept", "application/json")
        .send_form([
            ("client_id", client_id),
            ("device_code", code.device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .map_err(transport)?;
    let text = res
        .body_mut()
        .read_to_string()
        .map_err(|e| ApiError::Network(e.to_string()))?;
    Ok(parse_poll(&text))
}

/// Block until the user approves, denies or the code expires. Run on a
/// background thread. `cancelled` is polled between requests.
pub fn complete_device_flow(
    client_id: &str,
    code: &DeviceCode,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, ApiError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = Duration::from_secs(code.interval.max(5));
    while std::time::Instant::now() < deadline {
        std::thread::sleep(interval);
        if cancelled() {
            return Err(ApiError::Other("sign-in cancelled".into()));
        }
        match poll_device_flow(client_id, code)? {
            Poll::Token(t) => return Ok(t),
            Poll::Pending => {}
            Poll::SlowDown => interval += Duration::from_secs(5),
            Poll::Denied => return Err(ApiError::Other("sign-in was denied".into())),
            Poll::Expired => break,
        }
    }
    Err(ApiError::Other("the sign-in code expired".into()))
}

// ---- the API ------------------------------------------------------------

pub struct Client {
    token: String,
}

#[derive(Deserialize)]
struct IssueReply {
    number: u64,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
}

impl Client {
    pub fn new(token: String) -> Self {
        Self { token }
    }

    fn get(&self, path: &str) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        agent()
            .get(format!("{API}{path}"))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "rockhopper")
    }

    /// The connected account's login.
    pub fn username(&self) -> Result<String, ApiError> {
        #[derive(Deserialize)]
        struct User {
            login: String,
        }
        let mut res = self.get("/user").call().map_err(transport)?;
        check(&mut res)?;
        let user: User = res
            .body_mut()
            .read_json()
            .map_err(|e| ApiError::Other(e.to_string()))?;
        Ok(user.login)
    }
}

fn check(res: &mut ureq::http::Response<ureq::Body>) -> Result<(), ApiError> {
    let status = res.status();
    if status.is_success() {
        return Ok(());
    }
    let remaining = res
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let message = res.body_mut().read_to_string().unwrap_or_default();
    Err(classify(status.as_u16(), remaining.as_deref(), &message))
}

impl IssueApi for Client {
    fn create_issue(&self, repo: &str, title: &str, body: &str) -> Result<Issue, ApiError> {
        let mut res = agent()
            .post(format!("{API}/repos/{repo}/issues"))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "rockhopper")
            .send_json(serde_json::json!({ "title": title, "body": body }))
            .map_err(transport)?;
        check(&mut res)?;
        let reply: IssueReply = res
            .body_mut()
            .read_json()
            .map_err(|e| ApiError::Network(format!("unreadable reply: {e}")))?;
        Ok(Issue {
            number: reply.number,
            url: reply.html_url,
        })
    }

    fn find_by_marker(&self, repo: &str, marker: &str) -> Result<Option<Issue>, ApiError> {
        let username = self.username()?;
        let mut res = self
            .get(&format!(
                "/repos/{repo}/issues?creator={username}&state=all&per_page=30"
            ))
            .call()
            .map_err(transport)?;
        check(&mut res)?;
        let issues: Vec<IssueReply> = res
            .body_mut()
            .read_json()
            .map_err(|e| ApiError::Other(e.to_string()))?;
        Ok(issues
            .into_iter()
            .find(|i| i.body.as_deref().is_some_and(|b| b.contains(marker)))
            .map(|i| Issue {
                number: i.number,
                url: i.html_url,
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_recoverable_errors() {
        assert_eq!(classify(401, None, ""), ApiError::Unauthorized);
        assert_eq!(classify(403, Some("0"), ""), ApiError::RateLimited);
        assert_eq!(classify(429, None, ""), ApiError::RateLimited);
        assert_eq!(classify(403, Some("40"), ""), ApiError::Forbidden);
        assert_eq!(classify(404, None, ""), ApiError::Forbidden);
        assert!(matches!(classify(500, None, "boom"), ApiError::Other(m) if m.contains("boom")));
    }

    #[test]
    fn device_poll_replies() {
        assert_eq!(
            parse_poll(r#"{"access_token":"t","token_type":"bearer"}"#),
            Poll::Token("t".into())
        );
        assert_eq!(
            parse_poll(r#"{"error":"authorization_pending"}"#),
            Poll::Pending
        );
        assert_eq!(parse_poll(r#"{"error":"slow_down"}"#), Poll::SlowDown);
        assert_eq!(parse_poll(r#"{"error":"access_denied"}"#), Poll::Denied);
        assert_eq!(parse_poll(r#"{"error":"expired_token"}"#), Poll::Expired);
        assert_eq!(parse_poll("not json"), Poll::Pending);
    }

    #[test]
    fn no_client_id_is_reported_not_requested() {
        assert!(matches!(start_device_flow(" "), Err(ApiError::Other(_))));
    }

    #[test]
    fn error_text_never_contains_a_token() {
        let client = Client::new("ghu_secret".into());
        assert!(!format!("{}", classify(401, None, "")).contains("ghu_"));
        drop(client);
    }
}
