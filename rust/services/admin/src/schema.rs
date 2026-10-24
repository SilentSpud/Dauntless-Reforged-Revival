//! The GraphQL schema (written out in `schema.graphql`, checked by a test) and what each field does
//! against the backend and the Director.

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::{Context, EmptySubscription, Enum, ErrorExtensions, Object, Schema, SimpleObject};
use serde_json::json;

use crate::config;
use crate::input::{self, InstanceKind};
use crate::log;
use crate::timeouts::{Timeout, Timeouts};
use crate::upstream::{Account, Clock, Operator, Upstream, UpstreamError};

pub type AdminSchema = Schema<Query, Mutation, EmptySubscription>;

/// Deep enough for every field here, small enough that no query fans out into thousands of calls.
const MAX_DEPTH: usize = 6;
const MAX_COMPLEXITY: usize = 200;

pub fn build(admin: Arc<Admin>) -> AdminSchema {
    Schema::build(Query, Mutation, EmptySubscription)
        .data(admin)
        .limit_depth(MAX_DEPTH)
        .limit_complexity(MAX_COMPLEXITY)
        .finish()
}

/// The schema in GraphQL's own language, as `schema.graphql` holds it.
pub fn sdl() -> String {
    Schema::build(Query, Mutation, EmptySubscription).finish().sdl()
}

// ------------------------------------------------------------------ errors

/// The `code` in an error's `extensions`, which the bot switches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    BadInput,
    NotFound,
    Protected,
    AlreadyBanned,
    NotBanned,
    NotTimedOut,
    BackendUnavailable,
    BackendRefused,
    StateNotSaved,
}

impl Code {
    fn as_str(self) -> &'static str {
        match self {
            Self::BadInput => "BAD_INPUT",
            Self::NotFound => "NOT_FOUND",
            Self::Protected => "PROTECTED_ACCOUNT",
            Self::AlreadyBanned => "ALREADY_BANNED",
            Self::NotBanned => "NOT_BANNED",
            Self::NotTimedOut => "NOT_TIMED_OUT",
            Self::BackendUnavailable => "BACKEND_UNAVAILABLE",
            Self::BackendRefused => "BACKEND_REFUSED",
            Self::StateNotSaved => "STATE_NOT_SAVED",
        }
    }
}

fn fail(code: Code, message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message).extend_with(|_, e| e.set("code", code.as_str()))
}

/// An upstream failure as the bot sees it: what to do about it, without the backend's addresses.
fn upstream(service: &str, error: UpstreamError) -> async_graphql::Error {
    log::warn(&format!("{service} call failed"), json!({"error": error.to_string()}));
    match error {
        UpstreamError::Unavailable(_) => fail(
            Code::BackendUnavailable,
            format!("the {service} did not answer; is the server running?"),
        ),
        UpstreamError::Refused { status: 404, .. } => fail(Code::NotFound, "no such player"),
        UpstreamError::Refused { status, code } => {
            let code = code.unwrap_or_default();
            let hint = if code.starts_with("ADMIN_OPERATOR") || matches!(status, 401 | 403 | 503) {
                ": the admin API's credentials do not match the backend's; run reforged-host setup and restart the server"
            } else {
                ""
            };
            fail(
                Code::BackendRefused,
                format!("the {service} refused ({status} {code}){hint}"),
            )
        }
    }
}

// ------------------------------------------------------------------ types

#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum AccountStatus {
    Active,
    Banned,
    Disabled,
    /// A status this API does not know.
    Unknown,
}

#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum RestrictionKind {
    /// Banned until someone lifts it.
    Ban,
    /// Banned until `until`, then lifted by the admin API.
    Timeout,
    /// Disabled by the backend's own admin tools.
    Disabled,
}

/// Why a player cannot play right now.
#[derive(SimpleObject, Clone, Debug, PartialEq, Eq)]
pub struct Restriction {
    pub kind: RestrictionKind,
    /// The text the player sees in the launcher.
    pub reason: Option<String>,
    /// When it was set (RFC 3339).
    pub since: Option<String>,
    /// Who set it: `manager:<operator id>` for the admin API, an account id for the backend's dashboard.
    pub by: Option<String>,
    /// When a timeout ends (RFC 3339); null for the other kinds.
    pub until: Option<String>,
}

#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum LocationKind {
    /// Ramsgate
    Hub,
    Hunt,
    /// The Training Grounds
    Training,
    Tutorial,
}

/// The game server a player is on, from the Director.
#[derive(SimpleObject, Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub kind: LocationKind,
    pub instance_id: String,
    /// "Ramsgate", "Training Grounds", or the hunt's behemoth or island.
    pub title: String,
}

#[derive(SimpleObject, Clone, Debug, PartialEq, Eq)]
pub struct Player {
    pub user_id: String,
    pub username: String,
    pub status: AccountStatus,
    /// Null when the player may play.
    pub restriction: Option<Restriction>,
    /// A chat connection, or a game that reported in within the last minute.
    pub online: bool,
    /// Null when the player is in no game server the Director knows of (the menus, or offline).
    pub location: Option<Location>,
    /// Holds the backend's admin role: kicks, timeouts and bans refuse such accounts.
    pub admin: bool,
    pub created_at: Option<String>,
    pub last_login_at: Option<String>,
}

#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ServerMode {
    /// Reached over Tailscale.
    Private,
    /// Reached over the internet through the gateway's TLS port.
    Public,
}

#[derive(SimpleObject, Clone, Debug, PartialEq, Eq)]
pub struct ServerStatus {
    pub name: String,
    pub mode: ServerMode,
    /// INVITECODE, OPEN or NONE.
    pub registration: String,
    pub version: String,
    /// The server's source commit; null when unknown.
    pub commit: Option<String>,
    /// Seconds since the backend started.
    pub uptime_seconds: u32,
    pub online_count: u32,
    pub chat: bool,
    /// real, synthetic or missing.
    pub game_data: String,
}

#[derive(SimpleObject, Clone, Debug, PartialEq, Eq)]
pub struct HuntPlayer {
    pub user_id: String,
    /// Null when the backend has no account with that id.
    pub username: Option<String>,
}

#[derive(SimpleObject, Clone, Debug, PartialEq, Eq)]
pub struct Hunt {
    pub id: String,
    pub title: String,
    pub map: String,
    pub behemoth: Option<String>,
    pub player_count: u32,
    pub max_players: u32,
    pub players: Vec<HuntPlayer>,
    pub started_at: Option<String>,
}

#[derive(SimpleObject, Clone, Debug, PartialEq, Eq)]
pub struct HuntStatus {
    /// `max_hunt_servers` in host.toml.
    pub max_hunt_servers: u32,
    pub running: u32,
    /// Hunt servers that can still start.
    pub free: u32,
    pub hunts: Vec<Hunt>,
}

#[derive(SimpleObject, Clone, Debug, PartialEq, Eq)]
pub struct KickResult {
    pub user_id: String,
    pub username: String,
    /// Chat connections closed; 0 when the player held none.
    pub connections_closed: u32,
}

// ------------------------------------------------------------------ what the fields do

pub struct Admin {
    pub upstream: Upstream,
    pub timeouts: Timeouts,
    pub max_hunt_servers: u32,
    pub server_mode: config::ServerMode,
    pub clock: Clock,
}

const TIMEOUT_ENDED: &str = "Timeout ended";

fn status_of(account: &Account) -> AccountStatus {
    match account.status.as_str() {
        "active" => AccountStatus::Active,
        "banned" => AccountStatus::Banned,
        "disabled" => AccountStatus::Disabled,
        _ => AccountStatus::Unknown,
    }
}

fn location_of(kind: InstanceKind) -> LocationKind {
    match kind {
        InstanceKind::City => LocationKind::Hub,
        InstanceKind::Hunt => LocationKind::Hunt,
        InstanceKind::Dojo => LocationKind::Training,
        InstanceKind::Tutorial => LocationKind::Tutorial,
    }
}

/// The reason a timeout carries, so the player reads when it ends.
fn timeout_reason(until: i64, reason: &str) -> String {
    format!("Timed out until {}: {reason}", input::iso(until))
}

impl Admin {
    fn now(&self) -> i64 {
        (self.clock)()
    }

    async fn find(&self, operator: &Operator, name: &str) -> async_graphql::Result<Option<Account>> {
        let name = input::username(name).ok_or_else(|| {
            fail(
                Code::BadInput,
                "username must be 3 to 16 letters, digits or underscores",
            )
        })?;
        self.upstream
            .account_by_name(operator, name)
            .await
            .map_err(|e| upstream("backend", e))
    }

    async fn account(&self, operator: &Operator, name: &str) -> async_graphql::Result<Account> {
        self.find(operator, name)
            .await?
            .ok_or_else(|| fail(Code::NotFound, "no such player"))
    }

    /// The account, unless it is one moderation must not touch.
    async fn target(&self, operator: &Operator, name: &str) -> async_graphql::Result<Account> {
        let account = self.account(operator, name).await?;
        if account.roles.iter().any(|r| r == "admin") {
            return Err(fail(Code::Protected, "this player is a server admin"));
        }
        Ok(account)
    }

    async fn restriction(&self, account: &Account) -> Option<Restriction> {
        let kind = match status_of(account) {
            AccountStatus::Banned => {
                let timeout = self
                    .timeouts
                    .get(&account.user_id)
                    .await
                    .filter(|t| account.approval_reason.as_deref() == Some(t.reason.as_str()));
                if let Some(t) = timeout {
                    return Some(Restriction {
                        kind: RestrictionKind::Timeout,
                        reason: account.approval_reason.clone(),
                        since: account.approval_updated_at.clone(),
                        by: account.approval_updated_by.clone(),
                        until: Some(input::iso(t.until)),
                    });
                }
                RestrictionKind::Ban
            }
            AccountStatus::Disabled => RestrictionKind::Disabled,
            AccountStatus::Active | AccountStatus::Unknown => return None,
        };
        Some(Restriction {
            kind,
            reason: account.approval_reason.clone(),
            since: account.approval_updated_at.clone(),
            by: account.approval_updated_by.clone(),
            until: None,
        })
    }

    async fn player(&self, operator: &Operator, account: Account) -> async_graphql::Result<Player> {
        let online = self
            .upstream
            .online(operator)
            .await
            .map_err(|e| upstream("backend", e))?
            .iter()
            .any(|p| p.user_id == account.user_id);
        // Where a player is, is a nice-to-have: a Director that is down leaves it null
        let location = match self.upstream.instances().await {
            Ok(instances) => instances
                .into_iter()
                .find(|i| i.player_ids.contains(&account.user_id))
                .map(|i| Location {
                    kind: location_of(i.kind),
                    instance_id: i.id,
                    title: i.title,
                }),
            Err(error) => {
                log::warn("Director call failed", json!({"error": error.to_string()}));
                None
            }
        };
        Ok(Player {
            restriction: self.restriction(&account).await,
            status: status_of(&account),
            admin: account.roles.iter().any(|r| r == "admin"),
            user_id: account.user_id,
            username: account.display_name,
            online,
            location,
            created_at: account.created_at,
            last_login_at: account.last_login_at,
        })
    }

    async fn set_access(
        &self,
        operator: &Operator,
        account: &Account,
        status: &str,
        reason: &str,
    ) -> async_graphql::Result<Account> {
        let updated = self
            .upstream
            .set_access(operator, &account.user_id, status, reason)
            .await
            .map_err(|e| upstream("backend", e))?;
        log::info(
            &format!("player is now {status}"),
            json!({"userId": updated.user_id, "username": updated.display_name, "operator": operator.id(), "reason": reason}),
        );
        Ok(updated)
    }

    async fn forget_timeout(&self, user_id: &str) -> async_graphql::Result<()> {
        self.timeouts.remove(user_id).await.map_err(|e| {
            log::error("could not save the timeouts", json!({"error": e}));
            fail(
                Code::StateNotSaved,
                "the change was made, but the timeouts file could not be saved",
            )
        })
    }

    pub async fn kick(&self, operator: &Operator, name: &str) -> async_graphql::Result<KickResult> {
        let account = self.target(operator, name).await?;
        let kicked = self
            .upstream
            .kick(&account.display_name)
            .await
            .map_err(|e| upstream("backend", e))?;
        log::info(
            "player kicked",
            json!({"userId": kicked.user_id, "username": kicked.username, "operator": operator.id(), "closed": kicked.closed}),
        );
        Ok(KickResult {
            user_id: kicked.user_id,
            username: kicked.username,
            connections_closed: kicked.closed,
        })
    }

    pub async fn ban(&self, operator: &Operator, name: &str, reason: &str) -> async_graphql::Result<Player> {
        let reason = input::reason(reason).ok_or_else(|| fail(Code::BadInput, "reason must be 3 to 400 characters"))?;
        let account = self.target(operator, name).await?;
        let updated = self.set_access(operator, &account, "banned", &reason).await?;
        self.forget_timeout(&updated.user_id).await?;
        self.player(operator, updated).await
    }

    pub async fn timeout(
        &self,
        operator: &Operator,
        name: &str,
        minutes: Option<i64>,
        until: Option<&str>,
        reason: &str,
    ) -> async_graphql::Result<Player> {
        let reason = input::reason(reason).ok_or_else(|| fail(Code::BadInput, "reason must be 3 to 400 characters"))?;
        let now = self.now();
        let end = input::timeout_end(minutes, until, now).map_err(|why| fail(Code::BadInput, why))?;
        let account = self.target(operator, name).await?;
        let timed_out = self.timeouts.get(&account.user_id).await.is_some();
        match status_of(&account) {
            AccountStatus::Active => {}
            AccountStatus::Banned if timed_out => {}
            // Lifting the timeout would also lift the ban or the backend's own disabling
            _ => {
                return Err(fail(
                    Code::AlreadyBanned,
                    "this player is already banned or disabled; unban them first",
                ));
            }
        }
        let full_reason = timeout_reason(end, &reason);
        let updated = self.set_access(operator, &account, "banned", &full_reason).await?;
        let saved = self
            .timeouts
            .put(Timeout {
                user_id: updated.user_id.clone(),
                username: updated.display_name.clone(),
                until: end,
                reason: full_reason,
                by: operator.id().to_owned(),
                at: now,
            })
            .await;
        if let Err(error) = saved {
            // Not remembered means never lifted: undo the ban rather than leave it permanent
            log::error("could not save the timeouts", json!({"error": error}));
            self.set_access(operator, &updated, "active", "Timeout could not be recorded")
                .await?;
            return Err(fail(
                Code::StateNotSaved,
                "the timeouts file could not be saved, so the timeout was undone",
            ));
        }
        self.player(operator, updated).await
    }

    pub async fn unban(&self, operator: &Operator, name: &str, reason: Option<&str>) -> async_graphql::Result<Player> {
        let reason = match reason {
            Some(given) => {
                input::reason(given).ok_or_else(|| fail(Code::BadInput, "reason must be 3 to 400 characters"))?
            }
            None => "Ban lifted".into(),
        };
        let account = self.account(operator, name).await?;
        if status_of(&account) != AccountStatus::Banned {
            return Err(fail(Code::NotBanned, "this player is not banned"));
        }
        let updated = self.set_access(operator, &account, "active", &reason).await?;
        self.forget_timeout(&updated.user_id).await?;
        self.player(operator, updated).await
    }

    pub async fn lift_timeout(&self, operator: &Operator, name: &str) -> async_graphql::Result<Player> {
        let account = self.account(operator, name).await?;
        let player = self.restriction(&account).await;
        if player.map(|r| r.kind) != Some(RestrictionKind::Timeout) {
            return Err(fail(Code::NotTimedOut, "this player is not timed out"));
        }
        let updated = self.set_access(operator, &account, "active", "Timeout lifted").await?;
        self.forget_timeout(&updated.user_id).await?;
        self.player(operator, updated).await
    }

    /// Lifts the timeouts that have run out. One whose ban was changed since (lifted, or replaced
    /// by another ban) is only forgotten. A backend that does not answer is tried again next time.
    pub async fn sweep(&self) {
        let operator = Operator::host();
        for due in self.timeouts.due(self.now()).await {
            let accounts = match self
                .upstream
                .accounts_by_ids(&operator, std::slice::from_ref(&due.user_id))
                .await
            {
                Ok(accounts) => accounts,
                Err(error) => {
                    log::warn(
                        "timeout not lifted yet",
                        json!({"userId": due.user_id, "error": error.to_string()}),
                    );
                    continue;
                }
            };
            if let Some(account) = accounts
                .first()
                .filter(|a| a.status == "banned" && a.approval_reason.as_deref() == Some(due.reason.as_str()))
                && let Err(error) = self
                    .upstream
                    .set_access(&operator, &account.user_id, "active", TIMEOUT_ENDED)
                    .await
            {
                log::warn(
                    "timeout not lifted yet",
                    json!({"userId": due.user_id, "error": error.to_string()}),
                );
                continue;
            }
            log::info(
                "timeout ended",
                json!({"userId": due.user_id, "username": due.username}),
            );
            if let Err(error) = self.timeouts.remove(&due.user_id).await {
                log::error("could not save the timeouts", json!({"error": error}));
            }
        }
    }

    pub async fn server_status(&self, operator: &Operator) -> async_graphql::Result<ServerStatus> {
        let status = self.upstream.status().await.map_err(|e| upstream("backend", e))?;
        let online = self
            .upstream
            .online(operator)
            .await
            .map_err(|e| upstream("backend", e))?;
        Ok(ServerStatus {
            name: status.name,
            mode: match self.server_mode {
                config::ServerMode::Private => ServerMode::Private,
                config::ServerMode::Public => ServerMode::Public,
            },
            registration: status.registration,
            version: status.version,
            commit: Some(status.commit).filter(|c| !c.is_empty()),
            uptime_seconds: u32::try_from(status.uptime_seconds).unwrap_or(u32::MAX),
            online_count: u32::try_from(online.len()).unwrap_or(u32::MAX),
            chat: status.chat,
            game_data: status.game_data,
        })
    }

    pub async fn hunt_status(&self, operator: &Operator) -> async_graphql::Result<HuntStatus> {
        let hunts: Vec<_> = self
            .upstream
            .instances()
            .await
            .map_err(|e| upstream("Director", e))?
            .into_iter()
            .filter(|i| i.kind == InstanceKind::Hunt)
            .collect();
        let mut ids: Vec<String> = hunts.iter().flat_map(|h| h.player_ids.iter().cloned()).collect();
        ids.sort();
        ids.dedup();
        let names: HashMap<String, String> = self
            .upstream
            .accounts_by_ids(operator, &ids)
            .await
            .map_err(|e| upstream("backend", e))?
            .into_iter()
            .map(|a| (a.user_id, a.display_name))
            .collect();
        let running = u32::try_from(hunts.len()).unwrap_or(u32::MAX);
        Ok(HuntStatus {
            max_hunt_servers: self.max_hunt_servers,
            running,
            free: self.max_hunt_servers.saturating_sub(running),
            hunts: hunts
                .into_iter()
                .map(|h| Hunt {
                    players: h
                        .player_ids
                        .iter()
                        .map(|id| HuntPlayer {
                            user_id: id.clone(),
                            username: names.get(id).cloned(),
                        })
                        .collect(),
                    id: h.id,
                    title: h.title,
                    map: h.map,
                    behemoth: h.behemoth,
                    player_count: h.players,
                    max_players: h.max_players,
                    started_at: h.started_at,
                })
                .collect(),
        })
    }
}

// ------------------------------------------------------------------ the GraphQL roots

fn parts<'a>(ctx: &'a Context<'_>) -> async_graphql::Result<(&'a Arc<Admin>, &'a Operator)> {
    Ok((ctx.data::<Arc<Admin>>()?, ctx.data::<Operator>()?))
}

pub struct Query;

#[Object]
impl Query {
    /// A player by username (case-insensitive); null when there is no such account.
    async fn player(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<Option<Player>> {
        let (admin, operator) = parts(ctx)?;
        match admin.find(operator, &username).await? {
            Some(account) => admin.player(operator, account).await.map(Some),
            None => Ok(None),
        }
    }

    async fn server_status(&self, ctx: &Context<'_>) -> async_graphql::Result<ServerStatus> {
        let (admin, operator) = parts(ctx)?;
        admin.server_status(operator).await
    }

    /// The running hunt servers, who is in each, and how many more can start.
    async fn hunt_status(&self, ctx: &Context<'_>) -> async_graphql::Result<HuntStatus> {
        let (admin, operator) = parts(ctx)?;
        admin.hunt_status(operator).await
    }
}

pub struct Mutation;

#[Object]
impl Mutation {
    /// Drops a connected player now: their chat connections close and their sign-in tokens are
    /// revoked. They are not banned, and may log in again.
    async fn kick(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<KickResult> {
        let (admin, operator) = parts(ctx)?;
        admin.kick(operator, &username).await
    }

    /// Bans a player until a given time, then lifts the ban. Give `minutes` or `until` (RFC 3339),
    /// within a year. Replaces an earlier timeout; refused for a player who is banned.
    async fn timeout(
        &self,
        ctx: &Context<'_>,
        username: String,
        minutes: Option<i64>,
        until: Option<String>,
        reason: String,
    ) -> async_graphql::Result<Player> {
        let (admin, operator) = parts(ctx)?;
        admin
            .timeout(operator, &username, minutes, until.as_deref(), &reason)
            .await
    }

    /// Lifts a timeout before it runs out.
    async fn lift_timeout(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<Player> {
        let (admin, operator) = parts(ctx)?;
        admin.lift_timeout(operator, &username).await
    }

    /// Bans a player until someone unbans them. Replaces a timeout. The player reads `reason` in the
    /// launcher.
    async fn ban(&self, ctx: &Context<'_>, username: String, reason: String) -> async_graphql::Result<Player> {
        let (admin, operator) = parts(ctx)?;
        admin.ban(operator, &username, &reason).await
    }

    /// Lifts a ban or a timeout.
    async fn unban(
        &self,
        ctx: &Context<'_>,
        username: String,
        reason: Option<String>,
    ) -> async_graphql::Result<Player> {
        let (admin, operator) = parts(ctx)?;
        admin.unban(operator, &username, reason.as_deref()).await
    }
}
