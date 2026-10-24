//! Calls to the Node services on this machine: the backend's `/admin/v1` routes, signed with the
//! manager-operator credential (`paradox/backend/src/security/managerOperator.ts` holds the contract),
//! its `/reforged/api` kick with the admin key, its public status, and the Director's instances.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::hmac::{hex, hmac_sha256, sha256_hex};
use crate::input::{self, Instance};

const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY: usize = 1 << 20;
/// The backend's page size limit for `/admin/v1/players`.
const PAGE: usize = 100;
/// 10,000 accounts: past that a name search gives up rather than hold the bot's command.
const MAX_PAGES: usize = 100;

pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamError {
    /// Not reachable, timed out, or an answer that is not what the service sends.
    Unavailable(String),
    /// An answer other than 2xx: the status and the backend's error code, when it gave one.
    Refused { status: u16, code: Option<String> },
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "{why}"),
            Self::Refused { status, code } => write!(f, "answered {status} {}", code.as_deref().unwrap_or("")),
        }
    }
}

/// Who the moderation is done for: the Discord user id the bot names, or `0` for the host's own tools.
/// The backend's audit trail records it as `manager:<id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operator(String);

impl Operator {
    pub const HOST: &'static str = "0";

    /// 1-32 digits, as the backend's operator id; anything else is `None`.
    pub fn parse(id: &str) -> Option<Self> {
        (!id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit())).then(|| Self(id.to_owned()))
    }

    pub fn host() -> Self {
        Self(Self::HOST.to_owned())
    }

    pub fn id(&self) -> &str {
        &self.0
    }
}

/// An account as `/admin/v1` answers it (`PublicAccount` in `controllers/admin.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub user_id: String,
    pub display_name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub roles: Vec<String>,
    pub created_at: Option<String>,
    pub last_login_at: Option<String>,
    pub approval_updated_at: Option<String>,
    pub approval_updated_by: Option<String>,
    pub approval_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AccountPage {
    accounts: Vec<Account>,
    #[serde(default)]
    total: usize,
}

#[derive(Debug, Deserialize)]
struct AccountAnswer {
    account: Account,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlinePlayer {
    pub user_id: String,
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
struct OnlineAnswer {
    players: Vec<OnlinePlayer>,
}

/// `POST /reforged/api/Kick`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Kicked {
    pub user_id: String,
    pub username: String,
    pub closed: u32,
}

/// The parts of `/reforged/api/ServerStatus` (its keyless view) the admin API passes on.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub commit: String,
    pub registration: String,
    pub uptime_seconds: u64,
    #[serde(default)]
    pub chat: bool,
    #[serde(default)]
    pub game_data: String,
}

pub struct Upstream {
    backend: String,
    director: String,
    admin_key: String,
    operator_secret: String,
    clock: Clock,
    /// The last signature's timestamp: the backend refuses a signature it has seen in the last two
    /// minutes, so two identical calls in one millisecond must not sign the same text.
    last_signed: AtomicI64,
    client: Client<HttpConnector, Full<Bytes>>,
}

fn error_code(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    // `/admin/v1` nests it ({error: {code}}), `/reforged/api` does not ({error: "code"})
    let error = value.get("error")?;
    error
        .get("code")
        .unwrap_or(error)
        .as_str()
        .map(|code| code.chars().take(64).collect())
}

fn decode<T: for<'de> Deserialize<'de>>(body: &[u8], what: &str) -> Result<T, UpstreamError> {
    serde_json::from_slice(body).map_err(|e| UpstreamError::Unavailable(format!("{what}: unexpected answer ({e})")))
}

/// The manager-operator signature: HMAC-SHA256 over method, target, timestamp, operator id and the
/// body's SHA-256, one per line (`ManagerOperatorCanonical` in `managerOperator.ts`).
pub fn operator_signature(
    secret: &str,
    method: &str,
    target: &str,
    timestamp: &str,
    operator: &Operator,
    body: &[u8],
) -> String {
    let canonical = [method, target, timestamp, operator.id(), &sha256_hex(body)].join("\n");
    hex(&hmac_sha256(secret.as_bytes(), canonical.as_bytes()))
}

impl Upstream {
    pub fn new(backend: &str, director: &str, admin_key: &str, operator_secret: &str, clock: Clock) -> Self {
        Self {
            backend: backend.to_owned(),
            director: director.to_owned(),
            admin_key: admin_key.to_owned(),
            operator_secret: operator_secret.to_owned(),
            clock,
            last_signed: AtomicI64::new(0),
            client: Client::builder(TokioExecutor::new()).build(HttpConnector::new()),
        }
    }

    /// Now, or one past the last one taken when that is not earlier (the backend allows 5 s ahead).
    fn next_timestamp(&self) -> i64 {
        let now = (self.clock)();
        let previous = self
            .last_signed
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |last| Some(now.max(last + 1)))
            .unwrap_or(now);
        now.max(previous + 1)
    }

    async fn send(&self, request: Request<Full<Bytes>>, what: &str) -> Result<Bytes, UpstreamError> {
        let work = async {
            let response = self
                .client
                .request(request)
                .await
                .map_err(|e| UpstreamError::Unavailable(format!("{what}: {e}")))?;
            let status = response.status().as_u16();
            let body = Limited::new(response.into_body(), MAX_BODY)
                .collect()
                .await
                .map(|b| b.to_bytes())
                .map_err(|e| UpstreamError::Unavailable(format!("{what}: {e}")))?;
            if (200..300).contains(&status) {
                Ok(body)
            } else {
                Err(UpstreamError::Refused {
                    status,
                    code: error_code(&body),
                })
            }
        };
        tokio::time::timeout(TIMEOUT, work)
            .await
            .unwrap_or_else(|_| Err(UpstreamError::Unavailable(format!("{what}: timed out"))))
    }

    fn body(body: Option<&Value>) -> Vec<u8> {
        body.map(|b| serde_json::to_vec(b).unwrap_or_default())
            .unwrap_or_default()
    }

    /// A `/admin/v1` call as the manager sends it: `target` is the path and query exactly as sent.
    async fn signed(
        &self,
        operator: &Operator,
        method: Method,
        target: &str,
        body: Option<&Value>,
    ) -> Result<Bytes, UpstreamError> {
        let bytes = Self::body(body);
        let timestamp = self.next_timestamp().to_string();
        let signature = operator_signature(
            &self.operator_secret,
            method.as_str(),
            target,
            &timestamp,
            operator,
            &bytes,
        );
        let mut request = Request::builder()
            .method(method)
            .uri(format!("{}{target}", self.backend))
            .header("x-paradox-operator-id", operator.id())
            .header("x-paradox-operator-name", "admin-api")
            .header("x-paradox-operator-timestamp", timestamp)
            .header("x-paradox-operator-signature", signature);
        if !bytes.is_empty() {
            request = request.header("content-type", "application/json");
        }
        let request = request
            .body(Full::new(Bytes::from(bytes)))
            .map_err(|e| UpstreamError::Unavailable(e.to_string()))?;
        self.send(request, target).await
    }

    async fn plain(
        &self,
        method: Method,
        url: String,
        admin_key: bool,
        body: Option<&Value>,
    ) -> Result<Bytes, UpstreamError> {
        let bytes = Self::body(body);
        let mut request = Request::builder().method(method).uri(&url);
        if admin_key {
            request = request.header("x-reforged-admin-key", &self.admin_key);
        }
        if !bytes.is_empty() {
            request = request.header("content-type", "application/json");
        }
        let request = request
            .body(Full::new(Bytes::from(bytes)))
            .map_err(|e| UpstreamError::Unavailable(e.to_string()))?;
        // The URL, not the request: the admin key is in a header and never in a message
        self.send(request, &url).await
    }

    /// The account with this name, compared as the backend does (case-insensitive). The backend's
    /// search matches a part of the name, id or email, so the pages are read until the name turns up.
    pub async fn account_by_name(&self, operator: &Operator, username: &str) -> Result<Option<Account>, UpstreamError> {
        let search = percent_encoding::utf8_percent_encode(username, percent_encoding::NON_ALPHANUMERIC);
        for page in 1..=MAX_PAGES {
            let target = format!("/admin/v1/players?search={search}&page={page}&limit={PAGE}");
            let answer: AccountPage = decode(&self.signed(operator, Method::GET, &target, None).await?, &target)?;
            if let Some(found) = answer
                .accounts
                .iter()
                .find(|a| a.display_name.eq_ignore_ascii_case(username))
            {
                return Ok(Some(found.clone()));
            }
            if answer.accounts.len() < PAGE || page * PAGE >= answer.total {
                break;
            }
        }
        Ok(None)
    }

    /// The accounts with these ids that exist (ids the backend would refuse are left out).
    pub async fn accounts_by_ids(&self, operator: &Operator, ids: &[String]) -> Result<Vec<Account>, UpstreamError> {
        let ids: Vec<&String> = ids.iter().filter(|id| input::is_account_id(id)).collect();
        let mut found = Vec::new();
        for chunk in ids.chunks(PAGE) {
            let list = chunk.iter().map(|id| id.as_str()).collect::<Vec<_>>().join(",");
            let target = format!("/admin/v1/players?ids={list}");
            let answer: AccountPage = decode(&self.signed(operator, Method::GET, &target, None).await?, &target)?;
            found.extend(answer.accounts);
        }
        Ok(found)
    }

    /// Who is online: a chat connection, or a game that reported in within the last minute.
    pub async fn online(&self, operator: &Operator) -> Result<Vec<OnlinePlayer>, UpstreamError> {
        let target = "/admin/v1/online-players";
        let answer: OnlineAnswer = decode(&self.signed(operator, Method::GET, target, None).await?, target)?;
        Ok(answer.players)
    }

    /// Bans or reinstates an account. The backend audits it and, for a ban, revokes the account's
    /// refresh sessions and unused exchange codes.
    pub async fn set_access(
        &self,
        operator: &Operator,
        user_id: &str,
        status: &str,
        reason: &str,
    ) -> Result<Account, UpstreamError> {
        if !input::is_account_id(user_id) {
            return Err(UpstreamError::Refused {
                status: 404,
                code: Some("NOT_FOUND".into()),
            });
        }
        let target = format!("/admin/v1/players/{user_id}/access");
        let body = json!({"status": status, "reason": reason});
        let answer: AccountAnswer = decode(
            &self.signed(operator, Method::PATCH, &target, Some(&body)).await?,
            &target,
        )?;
        Ok(answer.account)
    }

    pub async fn kick(&self, username: &str) -> Result<Kicked, UpstreamError> {
        let url = format!("{}/reforged/api/Kick", self.backend);
        let body = json!({"Username": username});
        decode(&self.plain(Method::POST, url.clone(), true, Some(&body)).await?, &url)
    }

    pub async fn status(&self) -> Result<Status, UpstreamError> {
        let url = format!("{}/reforged/api/ServerStatus", self.backend);
        decode(&self.plain(Method::GET, url.clone(), false, None).await?, &url)
    }

    /// The Director's running game servers.
    pub async fn instances(&self) -> Result<Vec<Instance>, UpstreamError> {
        let url = format!("{}/api/instances", self.director);
        let value: Value = decode(&self.plain(Method::GET, url.clone(), false, None).await?, &url)?;
        Ok(input::parse_instances(&value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_ids_are_digits_only() {
        assert_eq!(
            Operator::parse("123456789012345678").map(|o| o.0),
            Some("123456789012345678".into())
        );
        for bad in ["", "12a", "-1", " 1", &"1".repeat(33)] {
            assert_eq!(Operator::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn signs_as_the_backend_verifies() {
        // Recorded from managerOperator.ts's ManagerOperatorSignature with the same inputs
        let operator = Operator::parse("123456789012345678").unwrap();
        let body = br#"{"status":"banned","reason":"spamming chat"}"#;
        assert_eq!(
            operator_signature(
                &format!("{}-operator-secret", "s".repeat(32)),
                "PATCH",
                "/admin/v1/players/u-1/access",
                "1800000000000",
                &operator,
                body
            ),
            "c760d18e7fb10e13f44c4c7f6300cf732b66b950a0ca5c72ee07a32f521991e4"
        );
    }

    #[test]
    fn no_two_signatures_share_a_timestamp() {
        let upstream = Upstream::new("http://127.0.0.1:1", "http://127.0.0.1:2", "k", "s", Arc::new(|| 1000));
        assert_eq!(
            [
                upstream.next_timestamp(),
                upstream.next_timestamp(),
                upstream.next_timestamp()
            ],
            [1000, 1001, 1002]
        );
    }

    #[test]
    fn reads_both_shapes_of_backend_error() {
        assert_eq!(
            error_code(br#"{"error":{"code":"ADMIN_OPERATOR_STALE","message":"x"}}"#).as_deref(),
            Some("ADMIN_OPERATOR_STALE")
        );
        assert_eq!(
            error_code(br#"{"error":"not_found","message":"x"}"#).as_deref(),
            Some("not_found")
        );
        assert_eq!(error_code(b""), None);
        assert_eq!(error_code(br#"{"error":5}"#), None);
    }
}
