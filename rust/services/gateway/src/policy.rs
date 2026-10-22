//! What the gateway does with one request (`src/policy.ts`), decided from the method, the request
//! target and the headers only, before any body is read. Pure functions, so every rule has a test.

use http::HeaderMap;
use service_kit::js;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upstream {
    Metagame,
    Content,
    Ws,
}

impl Upstream {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metagame => "metagame",
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

/// The only /undaunted/api routes reachable from outside, and with which methods. Everything else
/// under /undaunted/api is an admin route (invites, renames, rollbacks, entitlements, JWTs for any
/// account) and answers 403 here, even with a valid admin key.
pub const PUBLIC_UNDAUNTED_API: [(&str, &[&str]); 6] = [
    ("register", &["POST"]),
    ("getuserinfo", &["GET", "HEAD"]),
    ("serverstatus", &["GET", "HEAD"]),
    ("huntregion", &["GET", "HEAD", "POST"]),
    ("devicesignals", &["POST"]),
    ("registrationstatus", &["GET", "HEAD"]),
];

pub const GAMESERVER_KEY_HEADER: &str = "x-undaunted-gameserver-apikey";

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
/// and "/" only (checked against every path in the metagame logs).
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
/// trailing slash, so the admin-route check must be at least that lenient.
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

/// `Classify`.
pub fn classify(method: &str, raw_url: &str, headers: &HeaderMap, is_upgrade: bool) -> Decision {
    let verb = method.to_uppercase();
    let Some((path, _)) = split_target(raw_url) else {
        return reject(400, "bad_request", "target_not_origin_form", LimitClass::General);
    };
    if !check_path(path) {
        return reject(400, "bad_path", "bad_path", LimitClass::General);
    }
    let Some(canonical) = canonical_path(path) else {
        return reject(400, "bad_path", "bad_percent_encoding", LimitClass::General);
    };

    let limit_class = if is_upgrade {
        LimitClass::Ws
    } else if canonical == "/undaunted/api/register" {
        LimitClass::Register
    } else if canonical == "/account/api/oauth/token" {
        LimitClass::Token
    } else if canonical == "/content" || canonical.starts_with("/content/") {
        LimitClass::Content
    } else {
        LimitClass::General
    };

    if !ALLOWED_METHODS.contains(&verb.as_str()) {
        return reject(405, "method_not_allowed", "method", limit_class);
    }
    if headers.contains_key(GAMESERVER_KEY_HEADER) {
        return reject(403, "forbidden", "gameserver_key_header", limit_class);
    }

    if canonical == "/undaunted" || canonical.starts_with("/undaunted/") {
        let methods = canonical
            .strip_prefix("/undaunted/api/")
            .filter(|name| !name.is_empty() && !name.contains('/'))
            .and_then(|name| PUBLIC_UNDAUNTED_API.iter().find(|(route, _)| *route == name))
            .map(|(_, methods)| *methods);
        return match methods {
            Some(methods) if !is_upgrade && methods.contains(&verb.as_str()) => {
                proxy(Upstream::Metagame, limit_class, None)
            }
            _ => reject(403, "forbidden", "admin_route", limit_class),
        };
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

    if limit_class == LimitClass::Content {
        return proxy(Upstream::Content, limit_class, None);
    }

    // /game/login returns 2xx only after bearer validation and account lookup. Reuse the
    // bearer-required feed so travel need not wait for a heartbeat.
    if verb == "POST" && (canonical == "/heartbeat" || canonical == "/game/login") {
        return proxy(Upstream::Metagame, limit_class, Some(FeedKind::Heartbeat));
    }
    if verb == "POST" && canonical == "/account/api/oauth/token" {
        return proxy(Upstream::Metagame, limit_class, Some(FeedKind::Token));
    }
    proxy(Upstream::Metagame, limit_class, None)
}

/// A heartbeat only proves a login when it carried the player's bearer token (the upstream answers
/// 401 without one; this is a second check in case that ever changes). `/^bearer\s+\S+/i`.
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

    fn feed(decision: &Decision) -> Option<FeedKind> {
        match decision {
            Decision::Proxy { feed, .. } => *feed,
            Decision::Reject { .. } => None,
        }
    }

    fn is_public(method: &str, path: &str) -> bool {
        let name = path["/undaunted/api/".len()..].to_lowercase();
        PUBLIC_UNDAUNTED_API
            .iter()
            .any(|(route, methods)| *route == name && methods.contains(&method))
    }

    /// The /undaunted/api routes the Rust metagame serves, as the Node test read them from the
    /// Node metagame's source.
    fn undaunted_api_routes() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../metagame/src/routes");
        let pattern = regex::Regex::new(r#"\.(get|post|put|delete|patch)\(\s*"(/undaunted/api/[^"]+)""#).unwrap();
        let parameter = regex::Regex::new(r":[A-Za-z]+").unwrap();
        let mut routes = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let source = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            for found in pattern.captures_iter(&source) {
                routes.push((
                    found[1].to_uppercase(),
                    parameter.replace_all(&found[2], "ABCD-EFGH-JKMN").into_owned(),
                ));
            }
        }
        routes
    }

    #[test]
    fn device_checks_are_post_only() {
        assert_eq!(
            status(&classify("POST", "/undaunted/api/DeviceSignals", &none(), false)),
            None
        );
        for method in ["GET", "HEAD", "PUT", "DELETE"] {
            assert_eq!(
                status(&classify(method, "/undaunted/api/DeviceSignals", &none(), false)),
                Some(403)
            );
        }
    }

    #[test]
    fn only_the_public_undaunted_routes_pass() {
        let routes = undaunted_api_routes();
        assert!(routes.len() >= 20, "found only {} routes", routes.len());
        for name in [
            "Register",
            "GetUserInfo",
            "ServerStatus",
            "RegistrationStatus",
            "CreateInvite",
            "RenameUser",
            "GenerateJWTForUserId",
            "GrantEntitlement",
            "RollbackCharacter",
        ] {
            assert!(
                routes.iter().any(|(_, path)| *path == format!("/undaunted/api/{name}")),
                "{name}"
            );
        }
        for (method, path) in &routes {
            let choice = classify(method, path, &none(), false);
            if is_public(method, path) {
                assert_eq!(status(&choice), None, "{method} {path}");
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
        // POST RegistrationStatus changes the registration mode: admin only, blocked
        assert_eq!(
            status(&classify("POST", "/undaunted/api/RegistrationStatus", &none(), false)),
            Some(403)
        );
        assert_eq!(
            status(&classify("HEAD", "/undaunted/api/Register", &none(), false)),
            Some(403)
        );
        assert_eq!(
            status(&classify(
                "GET",
                "/undaunted/api/UsernameAvailable?Username=x",
                &none(),
                false
            )),
            Some(403)
        );
    }

    #[test]
    fn spelling_tricks_cannot_reach_an_admin_route() {
        for variant in [
            "/UNDAUNTED/API/CREATEINVITE",
            "/undaunted/api/createinvite",
            "/Undaunted/Api/CreateInvite/",
            "/undaunted/api/CreateInvite//",
            "/undaunted//api/CreateInvite",
            "//undaunted/api/CreateInvite",
            "/undaunted/api/%43reateInvite",
            "/%75ndaunted/api/CreateInvite",
            "/undaunted/api/CreateInvite?x=/undaunted/api/Register",
            "/undaunted/api/Register/../CreateInvite",
            "/undaunted/api/./CreateInvite",
            "/undaunted%2fapi/CreateInvite",
            "/undaunted/api%2FCreateInvite",
            "/undaunted/api/CreateInvite%00",
            "/undaunted/api\\CreateInvite",
            "/undaunted/api/%2e%2e/api/CreateInvite",
            "/undaunted/api/Register;/CreateInvite",
            "/undaunted/api/Register/",
            "/undaunted",
            "/undaunted/api",
            "/undaunted/api/",
            "/undaunted/api/Register/x",
            "/undaunted/api/%zz",
        ] {
            let choice = classify("POST", variant, &none(), false);
            if variant == "/undaunted/api/Register/" {
                // Express treats the trailing slash as the same route: this is Register itself
                assert_eq!(status(&choice), None, "{variant}");
                continue;
            }
            assert!(matches!(status(&choice), Some(403 | 400)), "{variant}");
        }
    }

    #[test]
    fn routes_content_upgrades_and_the_rest() {
        let proxied = |upstream, limit_class| Decision::Proxy {
            upstream,
            limit_class,
            feed: None,
        };
        use LimitClass as L;
        use Upstream as U;
        assert_eq!(
            classify("GET", "/content/v1/manifest", &none(), false),
            proxied(U::Content, L::Content)
        );
        assert_eq!(
            classify("GET", "/Content/v1/files/Archon/x.pak", &none(), false),
            proxied(U::Content, L::Content)
        );
        assert_eq!(
            classify("GET", "/party/invites", &none(), false),
            proxied(U::Metagame, L::General)
        );
        assert_eq!(
            classify("GET", "/contentx", &none(), false),
            proxied(U::Metagame, L::General)
        );
        assert_eq!(
            classify("GET", "/xmpp", &headers(&[("upgrade", "websocket")]), true),
            proxied(U::Ws, L::Ws)
        );
        assert_eq!(
            classify("GET", "/", &headers(&[("upgrade", "WebSocket")]), true),
            proxied(U::Ws, L::Ws)
        );
        // The client's own odd path from the logs goes through untouched
        assert_eq!(
            classify("GET", "/account127.0.0.1:61000", &none(), false),
            proxied(U::Metagame, L::General)
        );
        let inventory = "/inventory/7d1c9d8e-2b43-4c55-9d0e-6f3a0c1e2b44/dauntlessrel-1.4.4:239827";
        assert_eq!(status(&classify("POST", inventory, &none(), false)), None);
    }

    #[test]
    fn feeds_and_limit_classes() {
        assert_eq!(
            feed(&classify("POST", "/heartbeat", &none(), false)),
            Some(FeedKind::Heartbeat)
        );
        assert_eq!(
            feed(&classify("POST", "/HEARTBEAT/", &none(), false)),
            Some(FeedKind::Heartbeat)
        );
        let token = classify("POST", "/account/api/oauth/token", &none(), false);
        assert_eq!(
            (feed(&token), token.limit_class()),
            (Some(FeedKind::Token), LimitClass::Token)
        );
        for (method, path) in [
            ("GET", "/heartbeat"),
            ("POST", "/heartbeatx"),
            ("POST", "/account/api/oauth/verify"),
            ("GET", "/game/login"),
            ("POST", "/game/loginx"),
            ("POST", "/game/login/extra"),
        ] {
            assert_eq!(feed(&classify(method, path, &none(), false)), None, "{method} {path}");
        }
        for path in ["/game/login", "/GAME/LOGIN/", "/game/login?x=1"] {
            assert_eq!(
                feed(&classify("POST", path, &none(), false)),
                Some(FeedKind::Heartbeat),
                "{path}"
            );
        }
        assert_eq!(
            classify("POST", "/undaunted/api/Register", &none(), false).limit_class(),
            LimitClass::Register
        );
        assert_eq!(
            classify("POST", "/undaunted/api/register/", &none(), false).limit_class(),
            LimitClass::Register
        );
        assert_eq!(
            classify("POST", "/account/api/oauth/token?x=1", &none(), false).limit_class(),
            LimitClass::Token
        );
        assert_eq!(
            classify("GET", "/undaunted/api/ServerStatus", &none(), false).limit_class(),
            LimitClass::General
        );
    }

    #[test]
    fn the_game_server_key_header_is_always_refused() {
        let long = "a".repeat(48);
        for path in [
            "/heartbeat",
            "/character",
            "/content/v1/manifest",
            "/undaunted/api/Register",
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
                "http://evil.example/undaunted/api/CreateInvite",
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
            status(&classify("GET", "/undaunted/api/ServerStatus", &ws, true)),
            Some(403)
        );
    }

    #[test]
    fn the_clients_chat_connection_asks_for_a_double_slash() {
        // The request target of the game's XMPP WebSocket (exe 0x1441f761f, docs/findings/chat.md)
        let chat = headers(&[
            ("upgrade", "websocket"),
            ("sec-websocket-protocol", "xmpp"),
            ("origin", "http://127.0.0.1"),
        ]);
        assert_eq!(
            classify("GET", "//", &chat, true),
            Decision::Proxy {
                upstream: Upstream::Ws,
                limit_class: LimitClass::Ws,
                feed: None
            }
        );
        assert_eq!(
            status(&classify(
                "GET",
                "//undaunted/api/x",
                &headers(&[("upgrade", "websocket")]),
                true
            )),
            Some(403)
        );
        assert_eq!(status(&classify("GET", "//undaunted/api/x", &none(), false)), Some(403));
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
            canonical_path("/Undaunted/API/Register/").as_deref(),
            Some("/undaunted/api/register")
        );
        assert_eq!(canonical_path("//a///b//").as_deref(), Some("/a/b"));
        assert_eq!(canonical_path("/").as_deref(), Some("/"));
        assert_eq!(canonical_path("/%41").as_deref(), Some("/a"));
        assert_eq!(canonical_path("/%zz"), None);
        assert_eq!(canonical_path("/%C3"), None);
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
