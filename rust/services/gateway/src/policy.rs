//! What the gateway does with one request (`src/policy.ts`), decided from the method, the request
//! target and the headers only, before any body is read. Pure functions, so every rule has a test.
//!
//! The routes are the Mystic Paradox backend's: the 1.14.7 runtime sends the game's calls as
//! `/__origin/<official host>/<path>`, which the backend unwraps before routing, so every rule here
//! looks at that inner path.

use http::HeaderMap;
use service_kit::js;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upstream {
    Backend,
    Content,
    Ws,
}

impl Upstream {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Backend => "backend",
            Self::Content => "content",
            Self::Ws => "ws",
        }
    }
}

/// Which per-IP rate limit a request counts against. WebSocket upgrades (the game's chat connection)
/// have their own bucket, so chat reconnects can never use up the one the game's HTTP traffic needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitClass {
    General,
    Content,
    Register,
    Token,
    Ws,
}

impl LimitClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Content => "content",
            Self::Register => "register",
            Self::Token => "token",
            Self::Ws => "ws",
        }
    }
}

/// Requests whose 2xx answer proves the peer is a logged-in player: its address goes to the
/// allowlist helper, which opens the UDP game ports for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedKind {
    Heartbeat,
    Token,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Reject {
        status: u16,
        error: &'static str,
        reason: &'static str,
        limit_class: LimitClass,
    },
    Proxy {
        upstream: Upstream,
        limit_class: LimitClass,
        feed: Option<FeedKind>,
    },
}

impl Decision {
    pub fn limit_class(&self) -> LimitClass {
        match self {
            Self::Reject { limit_class, .. } | Self::Proxy { limit_class, .. } => *limit_class,
        }
    }
}

/// The only /reforged/api routes reachable from outside, and with which methods. Everything else
/// under /reforged/api is an admin route (invites, account status) and answers 403 here, even with
/// a valid admin key: the backend accepts those only from its own machine anyway.
pub const PUBLIC_REFORGED_API: [(&str, &[&str]); 4] = [
    ("register", &["POST"]),
    ("getuserinfo", &["GET", "HEAD"]),
    ("serverstatus", &["GET", "HEAD"]),
    ("registrationstatus", &["GET", "HEAD"]),
];

/// Path prefixes of the backend that are for the host only, never for players:
/// - `/admin`: Mystic Paradox's admin dashboard API
/// - `/launcher`: Mystic Paradox's own launcher API (email and Discord accounts, game sessions, its
///   update channel and guard); a Reforged server registers players through invites only
/// - `/internal`: the metrics endpoint
/// - `/health`: the readiness probes
pub const HOST_ONLY_PREFIXES: [&str; 4] = ["/admin", "/launcher", "/internal", "/health"];

/// Headers only the host's own processes send: the dedicated game servers (they run on the server
/// and talk to the backend directly), the admin tools and the metrics scraper.
pub const GAMESERVER_KEY_HEADER: &str = "x-mysticparadox-gameserver-apikey";
pub const HOST_ONLY_HEADERS: [&str; 3] = [
    GAMESERVER_KEY_HEADER,
    "x-reforged-admin-key",
    "x-mysticparadox-metrics-token",
];

const ALLOWED_METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"];

/// The request target must be origin-form ("/path?query"). Absolute-form ("http://host/...") and
/// "*" are refused so the routing below always sees the path the upstream will see.
pub fn split_target(raw: &str) -> Option<(&str, &str)> {
    if !raw.starts_with('/') {
        return None;
    }
    Some(raw.split_once('?').unwrap_or((raw, "")))
}

/// Paths the real client never sends and that could make the gateway and the upstream disagree
/// about which route is meant. The client's own paths contain letters, digits, "-", "_", ".", ":"
/// and "/" only. Stricter than `CheckPath` on Unicode spaces such as U+00A0 (Paradox paths are
/// ASCII).
pub fn check_path(path: &str) -> bool {
    if path
        .chars()
        .any(|c| c == '\\' || c <= '\u{1f}' || c == '\u{7f}' || js::is_space(c))
    {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    if ["%2f", "%5c", "%00", "%2e"].iter().any(|bad| lower.contains(bad)) {
        return false;
    }
    !path.split('/').any(|segment| segment == "." || segment == "..")
}

/// The path as a lenient router would match it: percent-decoded, lower case, repeated slashes
/// folded, trailing slashes dropped. Express matches routes case-insensitively and with an optional
/// trailing slash, so the host-only checks must be at least that lenient.
pub fn canonical_path(path: &str) -> Option<String> {
    let decoded = js::decode_uri_component(path)?.to_lowercase();
    let mut out = String::with_capacity(decoded.len());
    for c in decoded.chars() {
        if c == '/' && out.ends_with('/') {
            continue;
        }
        out.push(c);
    }
    while out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    Some(out)
}

/// `/^\/+__origin(\/|$)/i`: the path starts with an `/__origin` segment.
fn starts_with_origin(path: &str) -> bool {
    let rest = path.trim_start_matches('/');
    if rest.len() == path.len() {
        return false;
    }
    rest.get(..8).is_some_and(|word| word.eq_ignore_ascii_case("__origin"))
        && rest[8..].chars().next().is_none_or(|c| c == '/')
}

/// The inner path of `/__origin/<host><rest>` and the host, or the path itself with no host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unwrapped<'a> {
    pub path: std::borrow::Cow<'a, str>,
    pub origin: Option<String>,
}

/// `UnwrapOrigin`: the backend unwraps `/__origin/<host><rest>` before routing (once, and only for
/// a bare host name), the same way as here. The rules then apply to `<rest>`. A second
/// `/__origin` inside is never valid. `None` is `bad_origin`.
pub fn unwrap_origin(path: &str) -> Option<Unwrapped<'_>> {
    if !starts_with_origin(path) {
        return Some(Unwrapped {
            path: path.into(),
            origin: None,
        });
    }
    // `/^\/+__origin\/([^/?#]+)(.*)$/i`
    let after = &path.trim_start_matches('/')[8..];
    let after = after.strip_prefix('/')?;
    let host_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    if host_end == 0 {
        return None;
    }
    let host = js::decode_uri_component(&after[..host_end])?.to_lowercase();
    if !(1..=253).contains(&host.len())
        || !host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        || host.starts_with('.')
        || host.ends_with('.')
        || host.contains("..")
    {
        return None;
    }
    let rest = &after[host_end..];
    let inner = if rest.is_empty() {
        "/".to_owned()
    } else if rest.starts_with('/') {
        rest.to_owned()
    } else {
        format!("/{rest}")
    };
    if starts_with_origin(&inner) {
        return None;
    }
    Some(Unwrapped {
        path: inner.into(),
        origin: Some(host),
    })
}

fn reject(status: u16, error: &'static str, reason: &'static str, limit_class: LimitClass) -> Decision {
    Decision::Reject {
        status,
        error,
        reason,
        limit_class,
    }
}

fn proxy(upstream: Upstream, limit_class: LimitClass, feed: Option<FeedKind>) -> Decision {
    Decision::Proxy {
        upstream,
        limit_class,
        feed,
    }
}

/// `Under`: the path is the prefix or below it.
fn under(canonical: &str, prefix: &str) -> bool {
    canonical
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// `Classify`.
pub fn classify(method: &str, raw_url: &str, headers: &HeaderMap, is_upgrade: bool) -> Decision {
    let verb = method.to_uppercase();
    let Some((path, _)) = split_target(raw_url) else {
        return reject(400, "bad_request", "target_not_origin_form", LimitClass::General);
    };
    if !check_path(path) {
        return reject(400, "bad_path", "bad_path", LimitClass::General);
    }
    let Some(unwrapped) = unwrap_origin(path) else {
        return reject(400, "bad_path", "bad_origin", LimitClass::General);
    };
    let Some(canonical) = canonical_path(&unwrapped.path) else {
        return reject(400, "bad_path", "bad_percent_encoding", LimitClass::General);
    };
    // The content server is the launcher's, never behind an /__origin prefix
    let is_content = unwrapped.origin.is_none() && under(&canonical, "/content");

    let limit_class = if is_upgrade {
        LimitClass::Ws
    } else if canonical == "/reforged/api/register" {
        LimitClass::Register
    } else if canonical == "/account/api/oauth/token" {
        LimitClass::Token
    } else if is_content {
        LimitClass::Content
    } else {
        LimitClass::General
    };

    if !ALLOWED_METHODS.contains(&verb.as_str()) {
        return reject(405, "method_not_allowed", "method", limit_class);
    }
    for name in HOST_ONLY_HEADERS {
        if headers.contains_key(name) {
            let reason = if name == GAMESERVER_KEY_HEADER {
                "gameserver_key_header"
            } else {
                "host_only_header"
            };
            return reject(403, "forbidden", reason, limit_class);
        }
    }

    if under(&canonical, "/reforged") {
        let methods = canonical
            .strip_prefix("/reforged/api/")
            .filter(|name| !name.is_empty() && !name.contains('/'))
            .and_then(|name| PUBLIC_REFORGED_API.iter().find(|(route, _)| *route == name))
            .map(|(_, methods)| *methods);
        return match methods {
            Some(methods) if !is_upgrade && methods.contains(&verb.as_str()) => {
                proxy(Upstream::Backend, limit_class, None)
            }
            _ => reject(403, "forbidden", "admin_route", limit_class),
        };
    }

    if HOST_ONLY_PREFIXES.iter().any(|prefix| under(&canonical, prefix)) {
        return reject(403, "forbidden", "host_only_route", limit_class);
    }

    if is_upgrade {
        let upgrade = headers
            .get(http::header::UPGRADE)
            .map(|v| String::from_utf8_lossy(v.as_bytes()).to_lowercase())
            .unwrap_or_default();
        if verb != "GET" || upgrade != "websocket" {
            return reject(400, "bad_request", "unsupported_upgrade", limit_class);
        }
        return proxy(Upstream::Ws, limit_class, None);
    }

    if is_content {
        return proxy(Upstream::Content, limit_class, None);
    }

    // Paradox has no /game/login (DR's metagame fed the allowlist from it): its login is POST
    // /login, so only the heartbeat and the token route feed, as in the Node gateway (RD20)
    if verb == "POST" && canonical == "/heartbeat" {
        return proxy(Upstream::Backend, limit_class, Some(FeedKind::Heartbeat));
    }
    if verb == "POST" && canonical == "/account/api/oauth/token" {
        return proxy(Upstream::Backend, limit_class, Some(FeedKind::Token));
    }
    proxy(Upstream::Backend, limit_class, None)
}

/// A heartbeat only proves a login when it carried the player's bearer token. Paradox answers
/// `POST /heartbeat` without looking at it, so this check is the only one: it keeps a peer with no
/// token from opening the game ports, but any bearer text passes (`/^bearer\s+\S+/i`).
pub fn has_bearer(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(prefix) = value.get(..6) else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case("bearer") {
        return false;
    }
    let rest = &value[6..];
    let after = rest.trim_start_matches(js::is_space);
    after.len() < rest.len() && after.chars().next().is_some_and(|c| !js::is_space(c))
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    fn none() -> HeaderMap {
        HeaderMap::new()
    }

    fn status(decision: &Decision) -> Option<u16> {
        match decision {
            Decision::Reject { status, .. } => Some(*status),
            Decision::Proxy { .. } => None,
        }
    }

    fn reason(decision: &Decision) -> Option<&'static str> {
        match decision {
            Decision::Reject { reason, .. } => Some(reason),
            Decision::Proxy { .. } => None,
        }
    }

    fn feed(decision: &Decision) -> Option<FeedKind> {
        match decision {
            Decision::Proxy { feed, .. } => *feed,
            Decision::Reject { .. } => None,
        }
    }

    fn proxied(upstream: Upstream, limit_class: LimitClass, feed: Option<FeedKind>) -> Decision {
        Decision::Proxy {
            upstream,
            limit_class,
            feed,
        }
    }

    fn is_public(method: &str, path: &str) -> bool {
        let name = path["/reforged/api/".len()..].to_lowercase();
        PUBLIC_REFORGED_API
            .iter()
            .any(|(route, methods)| *route == name && methods.contains(&method))
    }

    /// `ReforgedApiRoutes`: every route the backend registers under /reforged/api, read from its
    /// source (patch 0002 in docs/paradox-patches.md), so a route added there later is covered.
    fn reforged_api_routes() -> Vec<(String, String)> {
        let file =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../paradox/backend/src/routes/reforgedApi.ts");
        let source = std::fs::read_to_string(&file).unwrap();
        let pattern =
            regex::Regex::new(r#"(?m)^\s*reforgedApiRouter\.(get|post|put|delete|patch)\(\s*"([^"]+)""#).unwrap();
        let parameter = regex::Regex::new(r":[A-Za-z]+").unwrap();
        pattern
            .captures_iter(&source)
            .filter(|found| found[2].starts_with("/reforged/api/"))
            .map(|found| {
                (
                    found[1].to_uppercase(),
                    parameter.replace_all(&found[2], "ABCD-EFGH-JKMN").into_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn the_backend_still_has_the_routes_this_gateway_knows_about() {
        let routes = reforged_api_routes();
        assert!(routes.len() >= 8, "found only {} routes", routes.len());
        for name in [
            "Register",
            "GetUserInfo",
            "ServerStatus",
            "RegistrationStatus",
            "CreateInvite",
            "InviteCodes",
            "SetAccountStatus",
        ] {
            assert!(
                routes.iter().any(|(_, path)| *path == format!("/reforged/api/{name}")),
                "{name}"
            );
        }
    }

    #[test]
    fn only_the_public_reforged_routes_pass() {
        let mut passed = 0;
        for (method, path) in &reforged_api_routes() {
            let choice = classify(method, path, &none(), false);
            if is_public(method, path) {
                assert_eq!(status(&choice), None, "{method} {path}");
                passed += 1;
            } else {
                assert!(
                    matches!(
                        choice,
                        Decision::Reject {
                            status: 403,
                            reason: "admin_route",
                            ..
                        }
                    ),
                    "{method} {path}"
                );
            }
        }
        assert_eq!(passed, 4);
        // POST RegistrationStatus changes the registration mode: admin only, blocked
        for (method, path) in [
            ("POST", "/reforged/api/RegistrationStatus"),
            ("HEAD", "/reforged/api/Register"),
            ("GET", "/reforged/api/UsernameAvailable?Username=x"),
        ] {
            assert_eq!(
                status(&classify(method, path, &none(), false)),
                Some(403),
                "{method} {path}"
            );
        }
    }

    #[test]
    fn spelling_tricks_cannot_reach_an_admin_route() {
        for variant in [
            "/REFORGED/API/CREATEINVITE",
            "/reforged/api/createinvite",
            "/Reforged/Api/CreateInvite/",
            "/reforged/api/CreateInvite//",
            "/reforged//api/CreateInvite",
            "//reforged/api/CreateInvite",
            "/reforged/api/%43reateInvite",
            "/%72eforged/api/CreateInvite",
            "/reforged/api/CreateInvite?x=/reforged/api/Register",
            "/reforged/api/Register/../CreateInvite",
            "/reforged/api/./CreateInvite",
            "/reforged%2fapi/CreateInvite",
            "/reforged/api%2FCreateInvite",
            "/reforged/api/CreateInvite%00",
            "/reforged/api\\CreateInvite",
            "/reforged/api/%2e%2e/api/CreateInvite",
            "/reforged/api/Register;/CreateInvite",
            "/reforged/api/Register/",
            "/reforged",
            "/reforged/api",
            "/reforged/api/",
            "/reforged/api/Register/x",
            "/reforged/api/%zz",
        ] {
            let choice = classify("POST", variant, &none(), false);
            if variant == "/reforged/api/Register/" {
                // Express treats the trailing slash as the same route: this is Register itself
                assert_eq!(status(&choice), None, "{variant}");
                continue;
            }
            assert!(matches!(status(&choice), Some(403 | 400)), "{variant}");
        }
    }

    #[test]
    fn routes_content_upgrades_and_the_rest() {
        use LimitClass as L;
        use Upstream as U;
        let cases = [
            ("/content/v1/manifest", U::Content, L::Content),
            ("/Content/v1/files/Archon/x.pak", U::Content, L::Content),
            ("/party/invites", U::Backend, L::General),
            ("/contentx", U::Backend, L::General),
            // The client's own odd path from the logs goes through untouched
            ("/account127.0.0.1:61000", U::Backend, L::General),
        ];
        for (path, upstream, limit_class) in cases {
            assert_eq!(
                classify("GET", path, &none(), false),
                proxied(upstream, limit_class, None),
                "{path}"
            );
        }
        assert_eq!(
            classify("GET", "/xmpp", &headers(&[("upgrade", "websocket")]), true),
            proxied(U::Ws, L::Ws, None)
        );
        assert_eq!(
            classify("GET", "/", &headers(&[("upgrade", "WebSocket")]), true),
            proxied(U::Ws, L::Ws, None)
        );
        let inventory = "/inventory/7d1c9d8e-2b43-4c55-9d0e-6f3a0c1e2b44/dauntlessrel-1.4.4:239827";
        assert_eq!(status(&classify("POST", inventory, &none(), false)), None);
    }

    #[test]
    fn the_runtimes_origin_requests_are_judged_by_the_path_inside() {
        use LimitClass as L;
        use Upstream as U;
        let host = "/__origin/gamedev-prod.steelyard.ca";
        assert_eq!(
            classify("GET", &format!("{host}/party/invites"), &none(), false),
            proxied(U::Backend, L::General, None)
        );
        assert_eq!(
            classify(
                "POST",
                "/__origin/account-public-service-prod.ol.epicgames.com/account/api/oauth/token",
                &none(),
                false
            ),
            proxied(U::Backend, L::Token, Some(FeedKind::Token))
        );
        assert_eq!(
            classify("POST", &format!("{host}/heartbeat"), &none(), false),
            proxied(U::Backend, L::General, Some(FeedKind::Heartbeat))
        );
        for inner in [
            "/reforged/api/CreateInvite",
            "/admin/v1/players",
            "/launcher/v1/auth/register",
            "/internal/v1/metrics",
            "/health/ready",
            "/REFORGED/API/SetAccountStatus/",
        ] {
            assert_eq!(
                status(&classify("POST", &format!("{host}{inner}"), &none(), false)),
                Some(403),
                "{inner}"
            );
        }
        assert_eq!(
            status(&classify(
                "POST",
                &format!("{host}/reforged/api/Register"),
                &none(),
                false
            )),
            None
        );
        // Content is never reached through the prefix; the backend answers that path itself (404)
        assert_eq!(
            classify("GET", &format!("{host}/content/v1/runtime/manifest"), &none(), false),
            proxied(U::Backend, L::General, None)
        );
        for bad in [
            "/__origin/".to_owned(),
            "/__origin".to_owned(),
            "/__origin/a..b/x".to_owned(),
            "/__origin/.x/x".to_owned(),
            "/__origin/x%/y".to_owned(),
            "/__origin/user@host/x".to_owned(),
            "/__origin/host:443/x".to_owned(),
            format!("{host}/__origin/other/admin/v1"),
            format!("{host}//__origin/other/x"),
        ] {
            let choice = classify("GET", &bad, &none(), false);
            assert_eq!(status(&choice), Some(400), "{bad}");
        }
        assert_eq!(
            reason(&classify("GET", "/__ORIGIN", &none(), false)),
            Some("bad_origin")
        );
        assert_eq!(
            classify("GET", "/__origin", &none(), false).limit_class(),
            LimitClass::General
        );
    }

    #[test]
    fn blocks_the_host_only_parts_of_the_backend() {
        for (method, path) in [
            ("GET", "/admin/v1/players"),
            ("POST", "/admin/v1/auth/login"),
            ("PATCH", "/Admin/V1/players/x/access"),
            ("POST", "/launcher/v1/auth/register"),
            ("POST", "/launcher/v1/game-sessions"),
            ("GET", "/launcher/v1/runtime/stable/windows-x86_64/download"),
            ("GET", "/internal/v1/metrics"),
            ("GET", "/health/live"),
            ("GET", "/admin"),
            ("GET", "//launcher//v1"),
        ] {
            assert_eq!(
                reason(&classify(method, path, &none(), false)),
                Some("host_only_route"),
                "{method} {path}"
            );
        }
        assert_eq!(status(&classify("GET", "/administrator", &none(), false)), None);
        assert_eq!(status(&classify("GET", "/launchers", &none(), false)), None);
        // An upgrade to a host-only path is refused before the upgrade rules
        let ws = headers(&[("upgrade", "websocket")]);
        assert_eq!(
            reason(&classify("GET", "/health/live", &ws, true)),
            Some("host_only_route")
        );
    }

    #[test]
    fn refuses_the_admin_key_and_the_metrics_token_headers() {
        for name in ["x-reforged-admin-key", "x-mysticparadox-metrics-token"] {
            let choice = classify("GET", "/reforged/api/ServerStatus", &headers(&[(name, "x")]), false);
            assert_eq!(reason(&choice), Some("host_only_header"), "{name}");
        }
    }

    #[test]
    fn feeds_and_limit_classes() {
        use LimitClass as L;
        use Upstream as U;
        assert_eq!(
            classify("POST", "/heartbeat", &none(), false),
            proxied(U::Backend, L::General, Some(FeedKind::Heartbeat))
        );
        assert_eq!(
            classify("POST", "/HEARTBEAT/", &none(), false),
            proxied(U::Backend, L::General, Some(FeedKind::Heartbeat))
        );
        assert_eq!(
            classify("POST", "/account/api/oauth/token", &none(), false),
            proxied(U::Backend, L::Token, Some(FeedKind::Token))
        );
        for (method, path) in [
            ("GET", "/heartbeat"),
            ("POST", "/heartbeatx"),
            ("POST", "/account/api/oauth/verify"),
            // DR fed on these; Paradox has no /game/login, and /login is not a feed
            ("POST", "/game/login"),
            ("POST", "/GAME/LOGIN/"),
            ("POST", "/login"),
        ] {
            assert_eq!(feed(&classify(method, path, &none(), false)), None, "{method} {path}");
        }
        assert_eq!(
            classify("POST", "/reforged/api/Register", &none(), false).limit_class(),
            L::Register
        );
        assert_eq!(
            classify("POST", "/reforged/api/register/", &none(), false).limit_class(),
            L::Register
        );
        assert_eq!(
            classify("POST", "/account/api/oauth/token?x=1", &none(), false).limit_class(),
            L::Token
        );
        assert_eq!(
            classify("GET", "/reforged/api/ServerStatus", &none(), false).limit_class(),
            L::General
        );
    }

    #[test]
    fn the_game_server_key_header_is_always_refused() {
        let long = "a".repeat(48);
        for path in [
            "/heartbeat",
            "/character",
            "/content/v1/manifest",
            "/reforged/api/Register",
            "/progression/UID-1",
        ] {
            for value in ["", "x", long.as_str()] {
                let choice = classify("POST", path, &headers(&[(GAMESERVER_KEY_HEADER, value)]), false);
                assert!(matches!(
                    choice,
                    Decision::Reject {
                        status: 403,
                        reason: "gameserver_key_header",
                        ..
                    }
                ));
            }
        }
        let upgrade = headers(&[(GAMESERVER_KEY_HEADER, "k"), ("upgrade", "websocket")]);
        assert_eq!(status(&classify("GET", "/xmpp", &upgrade, true)), Some(403));
    }

    #[test]
    fn odd_targets_methods_and_upgrades() {
        let ws = headers(&[("upgrade", "websocket")]);
        assert_eq!(
            status(&classify(
                "GET",
                "http://evil.example/reforged/api/CreateInvite",
                &none(),
                false
            )),
            Some(400)
        );
        assert_eq!(status(&classify("OPTIONS", "*", &none(), false)), Some(400));
        assert_eq!(status(&classify("GET", "", &none(), false)), Some(400));
        assert_eq!(status(&classify("TRACE", "/heartbeat", &none(), false)), Some(405));
        assert_eq!(status(&classify("CONNECT", "/heartbeat", &none(), false)), Some(405));
        assert_eq!(
            status(&classify("GET", "/xmpp", &headers(&[("upgrade", "h2c")]), true)),
            Some(400)
        );
        assert_eq!(status(&classify("POST", "/xmpp", &ws, true)), Some(400));
        assert_eq!(
            status(&classify("GET", "/reforged/api/ServerStatus", &ws, true)),
            Some(403)
        );
    }

    #[test]
    fn the_clients_chat_connection_asks_for_a_double_slash() {
        // The request target of the game's XMPP WebSocket (Dauntless Revived's docs/findings/chat.md);
        // the backend's chat listener accepts //, / and /__ws/xmpp
        let chat = headers(&[
            ("upgrade", "websocket"),
            ("sec-websocket-protocol", "xmpp"),
            ("origin", "http://127.0.0.1"),
        ]);
        assert_eq!(
            classify("GET", "//", &chat, true),
            proxied(Upstream::Ws, LimitClass::Ws, None)
        );
        assert_eq!(
            status(&classify(
                "GET",
                "//reforged/api/x",
                &headers(&[("upgrade", "websocket")]),
                true
            )),
            Some(403)
        );
        assert_eq!(status(&classify("GET", "//reforged/api/x", &none(), false)), Some(403));
    }

    #[test]
    fn path_helpers() {
        for good in [
            "/",
            "/heartbeat",
            "/progression/UID-00000000-0000-4000-8000-00000000f609",
            "/inventory/x/dauntlessrel-1.4.4:239827",
            "/motd/",
            "/a.b/c..d/.e",
        ] {
            assert!(check_path(good), "{good}");
        }
        for bad in [
            "/a/../b",
            "/a/./b",
            "/..",
            "/a/..",
            "/a\\b",
            "/a b",
            "/a\u{0}b",
            "/a\tb",
            "/a%2Fb",
            "/a%5cb",
            "/a%00",
            "/a/%2e%2e/b",
            "/a/%2E/b",
            "/a\u{a0}b",
            "/a\u{feff}",
        ] {
            assert!(!check_path(bad), "{bad:?}");
        }
        assert_eq!(
            canonical_path("/Reforged/API/Register/").as_deref(),
            Some("/reforged/api/register")
        );
        assert_eq!(canonical_path("//a///b//").as_deref(), Some("/a/b"));
        assert_eq!(canonical_path("/").as_deref(), Some("/"));
        assert_eq!(canonical_path("/%41").as_deref(), Some("/a"));
        assert_eq!(canonical_path("/%zz"), None);
        assert_eq!(canonical_path("/%C3"), None);
    }

    #[test]
    fn unwrap_origin_unwraps_exactly_one_bare_host_prefix() {
        let unwrapped = |path: &str| unwrap_origin(path).map(|u| (u.path.into_owned(), u.origin));
        assert_eq!(
            unwrapped("/__origin/gamedev-prod.steelyard.ca/party"),
            Some(("/party".into(), Some("gamedev-prod.steelyard.ca".into())))
        );
        assert_eq!(
            unwrapped("/__origin/API.EpicGames.dev"),
            Some(("/".into(), Some("api.epicgames.dev".into())))
        );
        assert_eq!(
            unwrapped("//__Origin/a%2Db.example/x"),
            Some(("/x".into(), Some("a-b.example".into())))
        );
        assert_eq!(unwrapped("/party"), Some(("/party".into(), None)));
        assert_eq!(unwrapped("/__originx/y"), Some(("/__originx/y".into(), None)));
        assert_eq!(unwrapped("/__origin/x/__origin/y/z"), None);
        assert_eq!(unwrapped("/__origin"), None);
        assert_eq!(unwrapped("/__origin/"), None);
        assert_eq!(unwrapped(&format!("/__origin/{}/x", "a".repeat(254))), None);
    }

    #[test]
    fn bearer_tokens() {
        assert!(has_bearer(&headers(&[("authorization", "Bearer abc")])));
        assert!(has_bearer(&headers(&[("authorization", "bearer abc")])));
        assert!(has_bearer(&headers(&[("authorization", "BEARER\t x")])));
        assert!(!has_bearer(&headers(&[("authorization", "Bearer ")])));
        assert!(!has_bearer(&headers(&[("authorization", "Bearerabc")])));
        assert!(!has_bearer(&headers(&[("authorization", "Basic abc")])));
        assert!(!has_bearer(&none()));
    }
}
