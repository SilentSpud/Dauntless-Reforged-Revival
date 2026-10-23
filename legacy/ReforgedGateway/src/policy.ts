// What the gateway does with one request, decided from the method, the request target and the
// headers only (before any body is read). Pure functions, so every rule has a unit test.
//
// From Dauntless Revived's gateway (UndauntedGateway/src/policy.ts), with the routes of the Mystic
// Paradox backend: the 1.14.7 runtime sends the game's calls as /__origin/<official host>/<path>,
// which the backend unwraps before routing, so every rule here looks at that inner path.

export type Upstream = "backend" | "content" | "ws";

// Which per-IP rate limit a request counts against. WebSocket upgrades (the game's chat connection)
// have their own bucket, so chat reconnects can never use up the one the game's HTTP traffic needs.
export type LimitClass = "general" | "content" | "register" | "token" | "ws";

// Requests whose 2xx answer proves the peer is a logged-in player: its address goes to the
// allowlist helper, which opens the UDP game ports for it.
export type FeedKind = "heartbeat" | "token";

export type Decision =
    | { action: "reject"; status: 400 | 403 | 405; error: string; reason: string; limitClass: LimitClass }
    | { action: "proxy"; upstream: Upstream; limitClass: LimitClass; feed?: FeedKind };

// The only /reforged/api routes reachable from outside, and with which methods. Everything else
// under /reforged/api is an admin route (invites, account status) and answers 403 here, even with
// a valid admin key: the backend accepts those only from its own machine anyway.
export const PUBLIC_REFORGED_API: ReadonlyMap<string, readonly string[]> = new Map([
    ["register", ["POST"]],
    ["getuserinfo", ["GET", "HEAD"]],
    ["serverstatus", ["GET", "HEAD"]],
    ["registrationstatus", ["GET", "HEAD"]],
]);

// Path prefixes of the backend that are for the host only, never for players:
//   /admin       Mystic Paradox's admin dashboard API
//   /launcher    Mystic Paradox's own launcher API (email and Discord accounts, game sessions, its
//                update channel and guard): a Reforged server registers players through invites only
//   /internal    the metrics endpoint
//   /health      the readiness probes
export const HOST_ONLY_PREFIXES = ["/admin", "/launcher", "/internal", "/health"];

// Headers only the host's own processes send: the dedicated game servers (they run on the server
// and talk to the backend directly), the admin tools and the metrics scraper.
export const GAMESERVER_KEY_HEADER = "x-mysticparadox-gameserver-apikey";
export const HOST_ONLY_HEADERS = [GAMESERVER_KEY_HEADER, "x-reforged-admin-key", "x-mysticparadox-metrics-token"];

const ALLOWED_METHODS = new Set(["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"]);

// The request target must be origin-form ("/path?query"). Absolute-form ("http://host/...") and
// "*" are refused so the routing below always sees the path the upstream will see.
export function SplitTarget(RawUrl: string | undefined): { path: string; query: string } | undefined {
    if(RawUrl === undefined || !RawUrl.startsWith("/")){
        return undefined;
    }
    const Question = RawUrl.indexOf("?");
    return Question === -1
        ? { path: RawUrl, query: "" }
        : { path: RawUrl.slice(0, Question), query: RawUrl.slice(Question + 1) };
}

// Paths the real client never sends and that could make the gateway and the upstream disagree
// about which route is meant: control characters, whitespace, backslashes, encoded separators and
// dot segments.
export function CheckPath(Path: string): "ok" | "bad_path" {
    if(/[\\\x00-\x20\x7f]/.test(Path)){
        return "bad_path";
    }
    if(/%(2f|5c|00|2e)/i.test(Path)){
        return "bad_path";
    }
    for(const Segment of Path.split("/")){
        if(Segment === "." || Segment === ".."){
            return "bad_path";
        }
    }
    return "ok";
}

// The path as a lenient router would match it: percent-decoded, lower case, repeated slashes
// folded, trailing slashes dropped. Express matches routes case-insensitively and with an
// optional trailing slash, so the host-only checks must be at least that lenient.
export function CanonicalPath(Path: string): string | undefined {
    let Decoded: string;
    try{
        Decoded = decodeURIComponent(Path);
    }
    catch{
        return undefined;
    }
    let Out = Decoded.toLowerCase().replace(/\/{2,}/g, "/");
    while(Out.length > 1 && Out.endsWith("/")){
        Out = Out.slice(0, -1);
    }
    return Out;
}

// The backend unwraps /__origin/<host><rest> before routing (once, and only for a bare host name),
// the same way as here. The rules below apply to <rest>. A second /__origin inside is never valid.
export function UnwrapOrigin(Path: string): { path: string; origin: string | null } | "bad_origin" {
    const Match = /^\/+__origin\/([^/?#]+)(.*)$/i.exec(Path);
    if(Match === null){
        return /^\/+__origin(\/|$)/i.test(Path) ? "bad_origin" : { path: Path, origin: null };
    }
    let Host: string;
    try{
        Host = decodeURIComponent(Match[1]).toLowerCase();
    }
    catch{
        return "bad_origin";
    }
    if(!/^[a-z0-9.-]{1,253}$/.test(Host) || Host.startsWith(".") || Host.endsWith(".") || Host.includes("..")){
        return "bad_origin";
    }
    const Rest = Match[2].length > 0 ? Match[2] : "/";
    const Inner = Rest.startsWith("/") ? Rest : `/${Rest}`;
    if(/^\/+__origin(\/|$)/i.test(Inner)){
        return "bad_origin";
    }
    return { path: Inner, origin: Host };
}

function Reject(status: 400 | 403 | 405, error: string, reason: string, limitClass: LimitClass): Decision {
    return { action: "reject", status, error, reason, limitClass };
}

function Under(Canonical: string, Prefix: string): boolean {
    return Canonical === Prefix || Canonical.startsWith(`${Prefix}/`);
}

export function Classify(Method: string | undefined, RawUrl: string | undefined, Headers: Record<string, string | string[] | undefined>, IsUpgrade: boolean): Decision {
    const Verb = (Method ?? "").toUpperCase();

    const Target = SplitTarget(RawUrl);
    if(Target === undefined){
        return Reject(400, "bad_request", "target_not_origin_form", "general");
    }
    if(CheckPath(Target.path) !== "ok"){
        return Reject(400, "bad_path", "bad_path", "general");
    }
    const Unwrapped = UnwrapOrigin(Target.path);
    if(Unwrapped === "bad_origin"){
        return Reject(400, "bad_path", "bad_origin", "general");
    }
    const Canonical = CanonicalPath(Unwrapped.path);
    if(Canonical === undefined){
        return Reject(400, "bad_path", "bad_percent_encoding", "general");
    }
    // The content server is the launcher's, never behind an /__origin prefix.
    const IsContent = Unwrapped.origin === null && Under(Canonical, "/content");

    const LimitClass: LimitClass = IsUpgrade ? "ws"
        : Canonical === "/reforged/api/register" ? "register"
        : Canonical === "/account/api/oauth/token" ? "token"
        : IsContent ? "content"
        : "general";

    if(!ALLOWED_METHODS.has(Verb)){
        return Reject(405, "method_not_allowed", "method", LimitClass);
    }

    for(const Name of HOST_ONLY_HEADERS){
        if(Headers[Name] !== undefined){
            return Reject(403, "forbidden", Name === GAMESERVER_KEY_HEADER ? "gameserver_key_header" : "host_only_header", LimitClass);
        }
    }

    if(Under(Canonical, "/reforged")){
        const Match = /^\/reforged\/api\/([^/]+)$/.exec(Canonical);
        const Methods = Match === null ? undefined : PUBLIC_REFORGED_API.get(Match[1]);
        if(IsUpgrade || Methods === undefined || !Methods.includes(Verb)){
            return Reject(403, "forbidden", "admin_route", LimitClass);
        }
        return { action: "proxy", upstream: "backend", limitClass: LimitClass };
    }

    if(HOST_ONLY_PREFIXES.some((Prefix) => Under(Canonical, Prefix))){
        return Reject(403, "forbidden", "host_only_route", LimitClass);
    }

    if(IsUpgrade){
        const Upgrade = String(Headers["upgrade"] ?? "").toLowerCase();
        if(Verb !== "GET" || Upgrade !== "websocket"){
            return Reject(400, "bad_request", "unsupported_upgrade", LimitClass);
        }
        return { action: "proxy", upstream: "ws", limitClass: LimitClass };
    }

    if(IsContent){
        return { action: "proxy", upstream: "content", limitClass: LimitClass };
    }

    if(Verb === "POST" && Canonical === "/heartbeat"){
        return { action: "proxy", upstream: "backend", limitClass: LimitClass, feed: "heartbeat" };
    }
    if(Verb === "POST" && Canonical === "/account/api/oauth/token"){
        return { action: "proxy", upstream: "backend", limitClass: LimitClass, feed: "token" };
    }

    return { action: "proxy", upstream: "backend", limitClass: LimitClass };
}

// A heartbeat only proves a login when it carried the player's bearer token (the upstream
// answers 401 without one; this is a second check in case that ever changes).
export function HasBearer(Headers: Record<string, string | string[] | undefined>): boolean {
    const Value = Headers["authorization"];
    return typeof Value === "string" && /^bearer\s+\S+/i.test(Value);
}
