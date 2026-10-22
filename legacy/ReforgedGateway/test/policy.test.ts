import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { CanonicalPath, CheckPath, Classify, HasBearer, UnwrapOrigin } from "../src/policy";
import { IsPublicRoute, ReforgedApiRoutes } from "./helpers";

describe("the /reforged/api routes", () => {
    const Routes = ReforgedApiRoutes();

    it("the backend patch still has the routes this gateway knows about", () => {
        assert.ok(Routes.length >= 8, `found only ${Routes.length} routes`);
        for(const Name of ["Register", "GetUserInfo", "ServerStatus", "RegistrationStatus", "CreateInvite", "InviteCodes", "SetAccountStatus"]){
            assert.ok(Routes.some((Route) => Route.path === `/reforged/api/${Name}`), Name);
        }
    });

    it("only Register (POST), GetUserInfo, ServerStatus and RegistrationStatus (GET) pass; every other route is 403", () => {
        let Passed = 0;
        for(const Route of Routes){
            const Choice = Classify(Route.method, Route.path, {}, false);
            if(IsPublicRoute(Route.method, Route.path)){
                assert.equal(Choice.action, "proxy", `${Route.method} ${Route.path}`);
                Passed++;
            }
            else{
                assert.equal(Choice.action, "reject", `${Route.method} ${Route.path}`);
                assert.equal(Choice.action === "reject" && Choice.status, 403);
                assert.equal(Choice.action === "reject" && Choice.reason, "admin_route");
            }
        }
        assert.equal(Passed, 4);
        // POST RegistrationStatus changes the registration mode: admin only, blocked.
        assert.equal(Classify("POST", "/reforged/api/RegistrationStatus", {}, false).action, "reject");
        assert.equal(Classify("HEAD", "/reforged/api/Register", {}, false).action, "reject");
        assert.equal(Classify("GET", "/reforged/api/UsernameAvailable?Username=x", {}, false).action, "reject");
    });

    it("spelling tricks cannot reach an admin route", () => {
        const Variants = [
            "/REFORGED/API/CREATEINVITE", "/reforged/api/createinvite", "/Reforged/Api/CreateInvite/", "/reforged/api/CreateInvite//",
            "/reforged//api/CreateInvite", "//reforged/api/CreateInvite", "/reforged/api/%43reateInvite", "/%72eforged/api/CreateInvite",
            "/reforged/api/CreateInvite?x=/reforged/api/Register", "/reforged/api/Register/../CreateInvite", "/reforged/api/./CreateInvite",
            "/reforged%2fapi/CreateInvite", "/reforged/api%2FCreateInvite", "/reforged/api/CreateInvite%00", "/reforged/api\\CreateInvite",
            "/reforged/api/%2e%2e/api/CreateInvite", "/reforged/api/Register;/CreateInvite", "/reforged/api/Register/", "/reforged",
            "/reforged/api", "/reforged/api/", "/reforged/api/Register/x", "/reforged/api/%zz",
        ];
        for(const Variant of Variants){
            const Choice = Classify("POST", Variant, {}, false);
            if(Variant === "/reforged/api/Register/"){
                // Express treats the trailing slash as the same route: this is Register itself.
                assert.equal(Choice.action, "proxy", Variant);
                continue;
            }
            assert.equal(Choice.action, "reject", Variant);
            assert.ok(Choice.action === "reject" && (Choice.status === 403 || Choice.status === 400), Variant);
        }
    });
});

describe("Classify", () => {
    it("routes /content to the content server, upgrades to the websocket upstream, the rest to the backend", () => {
        assert.deepEqual(Classify("GET", "/content/v1/manifest", {}, false), { action: "proxy", upstream: "content", limitClass: "content" });
        assert.deepEqual(Classify("GET", "/Content/v1/files/Archon/x.pak", {}, false), { action: "proxy", upstream: "content", limitClass: "content" });
        assert.deepEqual(Classify("GET", "/party/invites", {}, false), { action: "proxy", upstream: "backend", limitClass: "general" });
        assert.deepEqual(Classify("GET", "/contentx", {}, false), { action: "proxy", upstream: "backend", limitClass: "general" });
        assert.deepEqual(Classify("GET", "/xmpp", { upgrade: "websocket" }, true), { action: "proxy", upstream: "ws", limitClass: "ws" });
        assert.deepEqual(Classify("GET", "/", { upgrade: "WebSocket" }, true), { action: "proxy", upstream: "ws", limitClass: "ws" });
        // The client's own odd path from the logs goes through untouched.
        assert.deepEqual(Classify("GET", "/account127.0.0.1:61000", {}, false), { action: "proxy", upstream: "backend", limitClass: "general" });
        assert.deepEqual(Classify("POST", "/inventory/7d1c9d8e-2b43-4c55-9d0e-6f3a0c1e2b44/dauntlessrel-1.4.4:239827", {}, false).action, "proxy");
    });

    it("the 1.14.7 runtime's /__origin/<host>/ requests are judged by the path inside", () => {
        const Host = "/__origin/gamedev-prod.steelyard.ca";
        assert.deepEqual(Classify("GET", `${Host}/party/invites`, {}, false), { action: "proxy", upstream: "backend", limitClass: "general" });
        assert.deepEqual(Classify("POST", "/__origin/account-public-service-prod.ol.epicgames.com/account/api/oauth/token", {}, false), { action: "proxy", upstream: "backend", limitClass: "token", feed: "token" });
        assert.deepEqual(Classify("POST", `${Host}/heartbeat`, {}, false), { action: "proxy", upstream: "backend", limitClass: "general", feed: "heartbeat" });
        for(const Inner of ["/reforged/api/CreateInvite", "/admin/v1/players", "/launcher/v1/auth/register", "/internal/v1/metrics", "/health/ready", "/REFORGED/API/SetAccountStatus/"]){
            const Choice = Classify("POST", `${Host}${Inner}`, {}, false);
            assert.equal(Choice.action === "reject" && Choice.status, 403, Inner);
        }
        assert.equal(Classify("POST", `${Host}/reforged/api/Register`, {}, false).action, "proxy");
        // Content is never reached through the prefix; the backend answers that path itself (404).
        assert.deepEqual(Classify("GET", `${Host}/content/v1/runtime/manifest`, {}, false), { action: "proxy", upstream: "backend", limitClass: "general" });
        for(const Bad of ["/__origin/", "/__origin", "/__origin/a..b/x", "/__origin/.x/x", "/__origin/x%/y", "/__origin/user@host/x", "/__origin/host:443/x", `${Host}/__origin/other/admin/v1`, `${Host}//__origin/other/x`]){
            const Choice = Classify("GET", Bad, {}, false);
            assert.equal(Choice.action === "reject" && Choice.status, 400, Bad);
        }
    });

    it("blocks the host-only parts of the backend: admin, Mystic Paradox's own launcher API, metrics, health", () => {
        for(const [Method, Path] of [["GET", "/admin/v1/players"], ["POST", "/admin/v1/auth/login"], ["PATCH", "/Admin/V1/players/x/access"], ["POST", "/launcher/v1/auth/register"], ["POST", "/launcher/v1/game-sessions"], ["GET", "/launcher/v1/runtime/stable/windows-x86_64/download"], ["GET", "/internal/v1/metrics"], ["GET", "/health/live"], ["GET", "/admin"], ["GET", "//launcher//v1"]]){
            const Choice = Classify(Method, Path, {}, false);
            assert.equal(Choice.action === "reject" && Choice.reason, "host_only_route", `${Method} ${Path}`);
        }
        assert.equal(Classify("GET", "/administrator", {}, false).action, "proxy");
        assert.equal(Classify("GET", "/launchers", {}, false).action, "proxy");
    });

    it("refuses the admin key and the metrics token headers from outside", () => {
        for(const Name of ["x-reforged-admin-key", "x-mysticparadox-metrics-token"]){
            const Choice = Classify("GET", "/reforged/api/ServerStatus", { [Name]: "x" }, false);
            assert.equal(Choice.action === "reject" && Choice.reason, "host_only_header", Name);
        }
    });

    it("marks heartbeats and token requests for the allowlist feed", () => {
        assert.deepEqual(Classify("POST", "/heartbeat", {}, false), { action: "proxy", upstream: "backend", limitClass: "general", feed: "heartbeat" });
        assert.deepEqual(Classify("POST", "/HEARTBEAT/", {}, false), { action: "proxy", upstream: "backend", limitClass: "general", feed: "heartbeat" });
        assert.deepEqual(Classify("POST", "/account/api/oauth/token", {}, false), { action: "proxy", upstream: "backend", limitClass: "token", feed: "token" });
        assert.equal((Classify("GET", "/heartbeat", {}, false) as any).feed, undefined);
        assert.equal((Classify("POST", "/heartbeatx", {}, false) as any).feed, undefined);
        assert.equal((Classify("POST", "/account/api/oauth/verify", {}, false) as any).feed, undefined);
    });

    it("puts Register and the token route in their own rate-limit classes", () => {
        assert.equal(Classify("POST", "/reforged/api/Register", {}, false).limitClass, "register");
        assert.equal(Classify("POST", "/reforged/api/register/", {}, false).limitClass, "register");
        assert.equal(Classify("POST", "/account/api/oauth/token?x=1", {}, false).limitClass, "token");
        assert.equal(Classify("GET", "/reforged/api/ServerStatus", {}, false).limitClass, "general");
    });

    it("refuses any request with the game-server key header, whatever its value or route", () => {
        for(const Path of ["/heartbeat", "/character", "/content/v1/manifest", "/reforged/api/Register", "/progression/UID-1"]){
            for(const Value of ["", "x", "a".repeat(48)]){
                const Choice = Classify("POST", Path, { "x-mysticparadox-gameserver-apikey": Value }, false);
                assert.equal(Choice.action, "reject");
                assert.equal(Choice.action === "reject" && Choice.status, 403);
                assert.equal(Choice.action === "reject" && Choice.reason, "gameserver_key_header");
            }
        }
        const Upgrade = Classify("GET", "/xmpp", { "x-mysticparadox-gameserver-apikey": "k", upgrade: "websocket" }, true);
        assert.equal(Upgrade.action === "reject" && Upgrade.status, 403);
    });

    it("refuses targets that are not a plain path, odd methods and non-websocket upgrades", () => {
        assert.equal((Classify("GET", "http://evil.example/reforged/api/CreateInvite", {}, false) as any).status, 400);
        assert.equal((Classify("OPTIONS", "*", {}, false) as any).status, 400);
        assert.equal((Classify("GET", "", {}, false) as any).status, 400);
        assert.equal((Classify("TRACE", "/heartbeat", {}, false) as any).status, 405);
        assert.equal((Classify("CONNECT", "/heartbeat", {}, false) as any).status, 405);
        assert.equal((Classify("GET", "/xmpp", { upgrade: "h2c" }, true) as any).status, 400);
        assert.equal((Classify("POST", "/xmpp", { upgrade: "websocket" }, true) as any).status, 400);
        assert.equal((Classify("GET", "/reforged/api/ServerStatus", { upgrade: "websocket" }, true) as any).status, 403);
    });

    it("the game's chat connection: libwebsockets asks for //, which goes to the websocket upstream in its own bucket", () => {
        // The request target of the game's XMPP WebSocket (Dauntless Revived's docs/findings/chat.md);
        // the backend's chat listener accepts //, / and /__ws/xmpp.
        assert.deepEqual(Classify("GET", "//", { upgrade: "websocket", "sec-websocket-protocol": "xmpp", origin: "http://127.0.0.1" }, true), { action: "proxy", upstream: "ws", limitClass: "ws" });
        assert.equal((Classify("GET", "//reforged/api/x", { upgrade: "websocket" }, true) as any).status, 403);
        assert.equal((Classify("GET", "//reforged/api/x", {}, false) as any).status, 403);
    });
});

describe("path helpers", () => {
    it("CheckPath refuses dot segments, backslashes, controls and encoded separators", () => {
        for(const Good of ["/", "/heartbeat", "/progression/UID-00000000-0000-4000-8000-00000000f609", "/inventory/x/dauntlessrel-1.4.4:239827", "/motd/", "/a.b/c..d/.e"]){
            assert.equal(CheckPath(Good), "ok", Good);
        }
        for(const Bad of ["/a/../b", "/a/./b", "/..", "/a/..", "/a\\b", "/a b", "/a\tb", "/a%2Fb", "/a%5cb", "/a%00", "/a/%2e%2e/b", "/a/%2E/b"]){
            assert.equal(CheckPath(Bad), "bad_path", Bad);
        }
    });

    it("CanonicalPath matches the way Express matches routes, and then some", () => {
        assert.equal(CanonicalPath("/Reforged/API/Register/"), "/reforged/api/register");
        assert.equal(CanonicalPath("//a///b//"), "/a/b");
        assert.equal(CanonicalPath("/"), "/");
        assert.equal(CanonicalPath("/%41"), "/a");
        assert.equal(CanonicalPath("/%zz"), undefined);
    });

    it("UnwrapOrigin unwraps exactly one /__origin/<bare host> prefix", () => {
        assert.deepEqual(UnwrapOrigin("/__origin/gamedev-prod.steelyard.ca/party"), { path: "/party", origin: "gamedev-prod.steelyard.ca" });
        assert.deepEqual(UnwrapOrigin("/__origin/API.EpicGames.dev"), { path: "/", origin: "api.epicgames.dev" });
        assert.deepEqual(UnwrapOrigin("/party"), { path: "/party", origin: null });
        assert.equal(UnwrapOrigin("/__origin/x/__origin/y/z"), "bad_origin");
        assert.equal(UnwrapOrigin("/__origin"), "bad_origin");
    });

    it("HasBearer wants a bearer token", () => {
        assert.equal(HasBearer({ authorization: "Bearer abc" }), true);
        assert.equal(HasBearer({ authorization: "bearer abc" }), true);
        assert.equal(HasBearer({ authorization: "Bearer " }), false);
        assert.equal(HasBearer({ authorization: "Basic abc" }), false);
        assert.equal(HasBearer({}), false);
    });
});
