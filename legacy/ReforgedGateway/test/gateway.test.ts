import assert from "node:assert/strict";
import crypto from "node:crypto";
import http from "node:http";
import https from "node:https";
import net from "node:net";
import { once } from "node:events";
import tls from "node:tls";
import { after, before, describe, it } from "node:test";
import { AllowlistFeed } from "../src/feed";
import { Gateway } from "../src/gateway";
import { CaptureLog, CapturedLog, FakeUpstream, HttpsRequest, IsPublicRoute, MakeCert, PortOpen, StartUpstream, TestCert, TestConfig, ReforgedApiRoutes } from "./helpers";

// Ports: gateway 62400, fake backend 62401, fake content 62402, fake ws 62403, fake allowlist
// helper 62404, second gateway 62405, nothing listening on 62409.
const P = { gateway: 62400, backend: 62401, content: 62402, ws: 62403, helper: 62404, gateway2: 62405, dead: 62409 };
const HELPER_SECRET = "test-helper-secret-abcdefabcdefabcdefabcdefabcdef";
const FAKE_JWT = "eyJhbGciOiJSUzI1NiJ9.eyJ1c2VySWQiOiJVSUQtdGVzdCJ9.c2lnbmF0dXJlLW5vdC1yZWFsLXNpZ25hdHVyZS1ub3QtcmVhbA";
const FAKE_KEY = "RRK_" + "fedcba9876543210".repeat(3);
const WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

type WsUpstream = { server: http.Server; seen: http.IncomingHttpHeaders[]; urls: string[]; close: () => Promise<void> };

// A minimal WebSocket server: completes the handshake (choosing the first protocol asked for, as the
// chat server does) and echoes bytes back with a prefix.
function StartWsUpstream(Port: number): Promise<WsUpstream> {
    const Seen: http.IncomingHttpHeaders[] = [];
    const Urls: string[] = [];
    const Server = http.createServer((_Req, Res) => {
        Res.writeHead(426);
        Res.end();
    });
    Server.on("upgrade", (Req: http.IncomingMessage, Socket: net.Socket) => {
        Seen.push(Req.headers);
        Urls.push(Req.url ?? "");
        // Sockets of an http.Server allow half-open connections: hang up when the peer does.
        Socket.on("end", () => Socket.end());
        if(Req.url === "/refuse"){
            Socket.end("HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nno room");
            return;
        }
        const Accept = crypto.createHash("sha1").update(String(Req.headers["sec-websocket-key"]) + WS_GUID).digest("base64");
        const Asked = Req.headers["sec-websocket-protocol"];
        const Protocol = typeof Asked === "string" ? `Sec-WebSocket-Protocol: ${Asked.split(",")[0].trim()}\r\n` : "";
        Socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n${Protocol}Sec-WebSocket-Accept: ${Accept}\r\n\r\n`);
        Socket.on("data", (Chunk: Buffer) => Socket.write(Buffer.concat([Buffer.from("echo:"), Chunk])));
        Socket.on("error", () => undefined);
    });
    return new Promise((resolve, reject) => {
        Server.once("error", reject);
        Server.listen(Port, "127.0.0.1", () => resolve({
            server: Server,
            seen: Seen,
            urls: Urls,
            close: () => new Promise<void>((done) => {
                Server.close(() => done());
                Server.closeAllConnections();
            }),
        }));
    });
}

type UpgradeResult = { status: number; headers: http.IncomingHttpHeaders; socket?: net.Socket; body?: string };

function Upgrade(Port: number, Path: string, Extra: Record<string, string> = {}): Promise<UpgradeResult & { key: string }> {
    const Key = crypto.randomBytes(16).toString("base64");
    return new Promise((resolve, reject) => {
        const Req = https.request({
            host: "127.0.0.1",
            port: Port,
            path: Path,
            agent: false,
            rejectUnauthorized: false,
            headers: { Connection: "Upgrade", Upgrade: "websocket", "Sec-WebSocket-Key": Key, "Sec-WebSocket-Version": "13", ...Extra },
        });
        Req.on("upgrade", (Res, Socket) => resolve({ status: Res.statusCode ?? 0, headers: Res.headers, socket: Socket, key: Key }));
        Req.on("response", (Res) => {
            let Body = "";
            Res.on("data", (Chunk) => Body += Chunk);
            Res.on("end", () => resolve({ status: Res.statusCode ?? 0, headers: Res.headers, body: Body, key: Key }));
        });
        Req.on("error", reject);
        Req.end();
    });
}

function ReadSome(Socket: net.Socket, Want: number): Promise<string> {
    return new Promise((resolve, reject) => {
        let Got = "";
        const OnData = (Chunk: Buffer) => {
            Got += Chunk.toString("utf8");
            if(Got.length >= Want){
                Socket.off("data", OnData);
                resolve(Got);
            }
        };
        Socket.on("data", OnData);
        Socket.once("error", reject);
        setTimeout(() => reject(new Error(`only got ${JSON.stringify(Got)}`)), 3000).unref();
    });
}

describe("gateway over TLS", () => {
    let Cert: TestCert;
    let Meta: FakeUpstream;
    let Content: FakeUpstream;
    let Helper: FakeUpstream;
    let Ws: WsUpstream;
    let Feed: AllowlistFeed;
    let TheGateway: Gateway;
    let Log: CapturedLog;
    const Config = TestConfig({ gateway: P.gateway, backend: P.backend, content: P.content, ws: P.ws });

    before(async () => {
        Log = CaptureLog();
        Cert = MakeCert(["127.0.0.1"]);
        Meta = await StartUpstream(P.backend, "backend", (Req, Res, Body) => {
            const Path = (Req.url ?? "").split("?")[0].toLowerCase();
            if(Path === "/heartbeat"){
                const Ok = Req.headers["authorization"] === "Bearer good-token" || Req.headers["x-test-force-ok"] === "1";
                Res.writeHead(Ok ? 200 : 401, { "content-type": "text/plain" });
                Res.end(Ok ? "20000" : "");
                return true;
            }
            if(Path === "/account/api/oauth/token"){
                const Ok = Body.toString("utf8").includes("exchange_code=good");
                Res.writeHead(Ok ? 200 : 400, { "content-type": "application/json" });
                Res.end(Ok ? JSON.stringify({ access_token: "x" }) : "");
                return true;
            }
            return false;
        });
        Content = await StartUpstream(P.content, "content");
        Helper = await StartUpstream(P.helper, "helper", (_Req, Res) => {
            Res.writeHead(200, { "content-type": "application/json" });
            Res.end("{}");
            return true;
        });
        Ws = await StartWsUpstream(P.ws);
        Feed = new AllowlistFeed({ url: `http://127.0.0.1:${P.helper}`, secret: HELPER_SECRET, refreshMs: 60_000, timeoutMs: 2000 });
        TheGateway = new Gateway(Config, { cert: Cert.certPem, key: Cert.keyPem }, Feed);
        await TheGateway.Listen();
    });

    after(async () => {
        await TheGateway.Close(500);
        await Promise.all([Meta.close(), Content.close(), Helper.close(), Ws.close()]);
        Log.stop();
    });

    it("serves the certificate from make-cert.js, and a client pinning its fingerprint connects", async () => {
        assert.match(Cert.fingerprint, /^[0-9a-f]{64}$/);
        assert.equal(TheGateway.Certificate?.fingerprint, Cert.fingerprint);
        const Reply = await HttpsRequest(P.gateway, "/party/invites", { pin: Cert.fingerprint });
        assert.equal(Reply.status, 200);
        assert.equal(Reply.peerFingerprint, Cert.fingerprint);
        const Before = Meta.seen.length;
        await assert.rejects(HttpsRequest(P.gateway, "/party/invites", { pin: "0".repeat(64) }), /not the pinned/);
        assert.equal(Meta.seen.length, Before, "a request went out before the pin was checked");
    });

    it("has 127.0.0.1 as an IP SAN, so ordinary validation against the cert as CA passes too", async () => {
        const Status = await new Promise<number>((resolve, reject) => {
            const Req = https.request({ host: "127.0.0.1", port: P.gateway, path: "/party/invites", agent: false, ca: Cert.certPem }, (Res) => {
                Res.resume();
                resolve(Res.statusCode ?? 0);
            });
            Req.on("error", reject);
            Req.end();
        });
        assert.equal(Status, 200);
        assert.match(Cert.names, /IP Address:127\.0\.0\.1/);
        const Days = (Date.parse(Cert.notAfter) - Date.parse(Cert.notBefore)) / 86_400_000;
        assert.ok(Days >= 3650 && Days <= 3654, `valid ${Days} days`);
    });

    it("refuses TLS older than 1.2 and plain HTTP", async () => {
        await assert.rejects(new Promise((resolve, reject) => {
            const Req = https.request({ host: "127.0.0.1", port: P.gateway, path: "/", agent: false, rejectUnauthorized: false, maxVersion: "TLSv1.1", minVersion: "TLSv1" }, resolve);
            Req.on("error", reject);
            Req.end();
        }));
        await assert.rejects(new Promise((resolve, reject) => {
            const Req = http.request({ host: "127.0.0.1", port: P.gateway, path: "/heartbeat", agent: false }, (Res) => {
                Res.resume();
                Res.statusCode === 400 ? reject(new Error("400")) : resolve(Res.statusCode);
            });
            Req.on("error", reject);
            Req.end();
        }));
    });

    it("sends /content/* to the content server and everything else to the backend, path and query untouched", async () => {
        const ContentBefore = Content.seen.length;
        const Manifest = await HttpsRequest(P.gateway, "/content/v1/manifest");
        assert.equal(Manifest.json().upstream, "content");
        assert.equal(Manifest.json().url, "/content/v1/manifest");
        assert.equal(Content.seen.length, ContentBefore + 1);

        const File = await HttpsRequest(P.gateway, "/content/v1/files/Archon/Content/Paks/x.pak", { headers: { "x-reforged-user-api-key": FAKE_KEY, range: "bytes=0-9" } });
        assert.equal(File.json().upstream, "content");
        assert.equal(File.json().headers["x-reforged-user-api-key"], FAKE_KEY);
        assert.equal(File.json().headers["range"], "bytes=0-9");

        for(const Path of ["/party/invites", "/entitlementsv2?accountId=UID-1&x=%20y", "/account127.0.0.1:61000", "/reforged/api/ServerStatus", "/QoS"]){
            const Reply = await HttpsRequest(P.gateway, Path);
            assert.equal(Reply.status, 200, Path);
            assert.equal(Reply.json().upstream, "backend", Path);
            assert.equal(Reply.json().url, Path);
        }
    });

    it("streams request bodies through unchanged and drops hop-by-hop and fingerprinting headers", async () => {
        const Body = Buffer.from(JSON.stringify({ characterId: "c", updateVersion: 3, data: "x".repeat(20_000) }));
        const Reply = await HttpsRequest(P.gateway, "/character", { method: "POST", body: Body, headers: { "content-type": "application/json", authorization: "Bearer good-token", "keep-alive": "timeout=5", "x-custom": "kept", connection: "x-custom-hop, keep-alive", "x-custom-hop": "dropped" } });
        const Echo = Reply.json();
        assert.equal(Echo.method, "POST");
        assert.equal(Echo.bodyLength, Body.length);
        assert.equal(Echo.bodySha, crypto.createHash("sha256").update(Body).digest("hex"));
        assert.equal(Echo.headers["authorization"], "Bearer good-token");
        assert.equal(Echo.headers["x-custom"], "kept");
        assert.equal(Echo.headers["x-custom-hop"], undefined);
        assert.equal(Echo.headers["keep-alive"], undefined);
        assert.equal(Reply.headers["x-powered-by"], undefined);
    });

    it("overwrites X-Forwarded-For with the real peer and adds the gateway secret, whatever the client claims", async () => {
        for(const [Local, Path] of [["127.0.0.1", "/party"], ["127.0.0.2", "/content/v1/news"], ["127.0.0.3", "/reforged/api/GetUserInfo"]] as const){
            const Reply = await HttpsRequest(P.gateway, Path, {
                localAddress: Local,
                headers: {
                    "x-forwarded-for": "6.6.6.6, 7.7.7.7",
                    "X-Real-IP": "6.6.6.6",
                    forwarded: "for=6.6.6.6",
                    "x-forwarded-proto": "http",
                    "x-forwarded-host": "evil.example",
                    "x-reforged-gateway": "forged",
                },
            });
            const Headers = Reply.json().headers;
            assert.equal(Headers["x-forwarded-for"], Local, Path);
            assert.equal(Headers["x-reforged-gateway"], Config.secret, Path);
            assert.equal(Headers["x-forwarded-proto"], "https");
            assert.equal(Headers["x-real-ip"], undefined);
            assert.equal(Headers["forwarded"], undefined);
            assert.equal(Headers["x-forwarded-host"], undefined);
            // The secret never goes back to the client.
            assert.ok(!Reply.headers["x-reforged-gateway"]);
        }
    });

    it("answers 403 to any request carrying the game-server key header, and the upstream never sees it", async () => {
        const MetaBefore = Meta.seen.length;
        const ContentBefore = Content.seen.length;
        for(const [Method, Path] of [["POST", "/heartbeat"], ["POST", "/progression/UID-1"], ["PUT", "/cooldown/batch/UID-1"], ["GET", "/content/v1/manifest"], ["POST", "/reforged/api/Register"], ["GET", "/party/invites"]]){
            const Reply = await HttpsRequest(P.gateway, Path, { method: Method, headers: { "x-mysticparadox-gameserver-apikey": "0123456789abcdef0123456789abcdef0123456789abcdef", authorization: "Bearer good-token" }, body: Method === "GET" ? undefined : "{}" });
            assert.equal(Reply.status, 403, `${Method} ${Path}`);
            assert.deepEqual(Reply.json(), { error: "forbidden" });
        }
        const Upper = await HttpsRequest(P.gateway, "/party", { headers: { "X-MysticParadox-GameServer-ApiKey": "k" } });
        assert.equal(Upper.status, 403);
        assert.equal(Meta.seen.length, MetaBefore);
        assert.equal(Content.seen.length, ContentBefore);
    });

    it("blocks every /reforged/api route of the backend except Register, GetUserInfo, ServerStatus and RegistrationStatus", async () => {
        const Routes = ReforgedApiRoutes();
        let Blocked = 0;
        for(const Route of Routes){
            const Before = Meta.seen.length;
            const Reply = await HttpsRequest(P.gateway, Route.path, { method: Route.method, headers: { "x-reforged-user-api-key": FAKE_KEY, "content-type": "application/json" }, body: Route.method === "GET" ? undefined : "{}" });
            if(IsPublicRoute(Route.method, Route.path)){
                assert.equal(Reply.status, 200, `${Route.method} ${Route.path}`);
                assert.equal(Meta.seen.length, Before + 1);
                assert.equal(Meta.seen[Meta.seen.length - 1].url, Route.path);
            }
            else{
                assert.equal(Reply.status, 403, `${Route.method} ${Route.path}`);
                assert.deepEqual(Reply.json(), { error: "forbidden" });
                assert.equal(Meta.seen.length, Before, `${Route.method} ${Route.path} reached the backend`);
                Blocked++;
            }
        }
        assert.equal(Blocked, Routes.length - 4);
        assert.ok(Blocked >= 4, `blocked ${Blocked}`);

        for(const Path of ["/REFORGED/API/CREATEINVITE", "/reforged/api/createinvite/", "/reforged//api/CreateInvite", "/reforged/api/%43reateInvite", "/reforged/api/GrantEntitlement?UserId=x"]){
            const Before = Meta.seen.length;
            const Reply = await HttpsRequest(P.gateway, Path, { method: "POST", body: "{}", headers: { "content-type": "application/json" } });
            assert.equal(Reply.status, 403, Path);
            assert.equal(Meta.seen.length, Before, Path);
        }
        for(const Path of ["/reforged/api/Register/../CreateInvite", "/reforged%2Fapi/CreateInvite", "/reforged/api\\CreateInvite"]){
            const Reply = await HttpsRequest(P.gateway, Path, { method: "POST", body: "{}" });
            assert.equal(Reply.status, 400, Path);
        }
    });

    it("reports the peer to the allowlist helper after a 2xx heartbeat with a bearer token or a 2xx token request, and only then", async () => {
        const Allowed = () => Helper.seen.filter((Seen) => Seen.url === "/allow").map((Seen) => JSON.parse(Seen.body.toString("utf8")).ip);

        // Not logged in: 401 upstream, nothing reported.
        await HttpsRequest(P.gateway, "/heartbeat", { method: "POST", body: "{}", localAddress: "127.0.0.4", headers: { authorization: "Bearer wrong" } });
        await HttpsRequest(P.gateway, "/heartbeat", { method: "POST", body: "{}", localAddress: "127.0.0.4" });
        // 200 but no bearer token (an upstream that stopped checking): not reported either.
        await HttpsRequest(P.gateway, "/heartbeat", { method: "POST", body: "{}", localAddress: "127.0.0.4", headers: { "x-test-force-ok": "1" } });
        // Other routes and other methods never report.
        await HttpsRequest(P.gateway, "/heartbeat", { method: "GET", localAddress: "127.0.0.4", headers: { authorization: "Bearer good-token" } });
        await HttpsRequest(P.gateway, "/party", { method: "POST", body: "{}", localAddress: "127.0.0.4", headers: { authorization: "Bearer good-token" } });
        await HttpsRequest(P.gateway, "/account/api/oauth/token", { method: "POST", body: "grant_type=exchange_code&exchange_code=bad", localAddress: "127.0.0.4", headers: { "content-type": "application/x-www-form-urlencoded" } });
        await Feed.Idle();
        assert.deepEqual(Allowed(), []);

        const Heartbeat = await HttpsRequest(P.gateway, "/heartbeat", { method: "POST", body: "{\"map\":\"x\"}", localAddress: "127.0.0.5", headers: { authorization: "Bearer good-token", "content-type": "application/json" } });
        assert.equal(Heartbeat.status, 200);
        assert.equal(Heartbeat.body.toString(), "20000");
        const Token = await HttpsRequest(P.gateway, "/account/api/oauth/token", { method: "POST", body: "grant_type=exchange_code&exchange_code=good", localAddress: "127.0.0.6", headers: { "content-type": "application/x-www-form-urlencoded" } });
        assert.equal(Token.status, 200);
        await Feed.Idle();
        assert.deepEqual(Allowed().sort(), ["127.0.0.5", "127.0.0.6"]);
        const Post = Helper.seen.find((Seen) => Seen.url === "/allow")!;
        assert.equal(Post.method, "POST");
        assert.equal(Post.headers["x-allowlist-secret"], HELPER_SECRET);
        assert.equal(Post.headers["content-type"], "application/json");

        // Heartbeats every 20 s do not reach the helper every time: once per refresh period.
        for(let Index = 0; Index < 3; Index++){
            await HttpsRequest(P.gateway, "/heartbeat", { method: "POST", body: "{}", localAddress: "127.0.0.5", headers: { authorization: "Bearer good-token" } });
        }
        await Feed.Idle();
        assert.equal(Allowed().filter((Ip) => Ip === "127.0.0.5").length, 1);
    });

    it("writes one access-log line per request with no token, key or secret in it", async () => {
        const Start = Log.lines.length;
        await HttpsRequest(P.gateway, `/account/api/oauth/sessions/kill/${FAKE_JWT}`, { method: "DELETE", headers: { authorization: `Bearer ${FAKE_JWT}`, "x-reforged-user-api-key": FAKE_KEY } });
        await HttpsRequest(P.gateway, `/entitlementsv2?accountId=UID-1&token=${FAKE_JWT}`, { headers: { authorization: `Bearer ${FAKE_JWT}` } });
        await HttpsRequest(P.gateway, "/reforged/api/CreateInvite", { method: "POST", body: "{\"InviteCode\":\"SECRET-CODE-1234\"}", headers: { "x-reforged-user-api-key": FAKE_KEY } });
        const Lines = Log.lines.slice(Start).filter((Line) => Line.entry.msg === "request");
        assert.equal(Lines.length, 3);
        const Text = Log.lines.map((Line) => Line.raw).join("\n");
        for(const Secret of [FAKE_JWT, FAKE_KEY, Config.secret, HELPER_SECRET, "SECRET-CODE-1234", "good-token"]){
            assert.ok(!Text.includes(Secret), `the log contains ${Secret.slice(0, 12)}...`);
        }
        const [Kill, Entitlements, Admin] = Lines.map((Line) => Line.entry);
        assert.equal(Kill.target, "/account/api/oauth/sessions/kill/<token>");
        assert.equal(Kill.route, "backend");
        assert.equal(Kill.status, 200);
        assert.equal(Kill.ip, "127.0.0.1");
        assert.equal(Entitlements.target, "/entitlementsv2?accountId=UID-1&token=<redacted>");
        assert.equal(Admin.status, 403);
        assert.equal(Admin.route, "gateway");
        assert.equal(Admin.reason, "admin_route");
        for(const Field of ["t", "ip", "method", "target", "route", "status", "bytesIn", "bytesOut", "ms"]){
            assert.ok(Field in Kill, Field);
        }
    });

    it("passes the account key on ServerStatus to the backend unchanged (registered players get the player list) and never logs it", async () => {
        const Start = Log.lines.length;
        // A key that does not look like one (no RRK_ prefix, short enough to escape the long-token
        // pattern): it stays out of the log only because headers are never logged at all.
        const OddKey = "k3y-" + crypto.randomBytes(8).toString("hex");
        const Cases: [string, string, string][] = [["GET", "/reforged/api/ServerStatus", FAKE_KEY], ["GET", "/reforged/api/serverstatus", OddKey], ["HEAD", "/reforged/api/ServerStatus", FAKE_KEY]];
        for(const [Method, Path, Key] of Cases){
            const Before = Meta.seen.length;
            const Reply = await HttpsRequest(P.gateway, Path, { method: Method, headers: { "X-Reforged-User-Api-Key": Key } });
            assert.equal(Reply.status, 200, `${Method} ${Path}`);
            assert.equal(Meta.seen.length, Before + 1, `${Method} ${Path} reached the backend`);
            const Seen = Meta.seen[Meta.seen.length - 1];
            assert.deepEqual([Seen.method, Seen.url], [Method, Path]);
            assert.equal(Seen.headers["x-reforged-user-api-key"], Key, "forwarded byte for byte");
            assert.equal(Seen.headers["x-reforged-gateway"], Config.secret);
            assert.equal(Seen.headers["x-forwarded-for"], "127.0.0.1");
        }
        // Without a key the backend gets no key header (it answers the limited status)
        const Anonymous = await HttpsRequest(P.gateway, "/reforged/api/ServerStatus");
        assert.equal(Anonymous.status, 200);
        assert.equal(Meta.seen[Meta.seen.length - 1].headers["x-reforged-user-api-key"], undefined);

        const Lines = Log.lines.slice(Start);
        assert.equal(Lines.filter((Line) => Line.entry.msg === "request").length, Cases.length + 1);
        const Text = Lines.map((Line) => Line.raw).join("\n");
        for(const Secret of [FAKE_KEY, OddKey, Config.secret]){
            assert.ok(!Text.includes(Secret), `the log contains ${Secret.slice(0, 6)}...`);
        }
        assert.doesNotMatch(Text, /x-reforged-user-api-key/i, "no header names or values in the access log");
        for(const Line of Lines.filter((Line) => Line.entry.msg === "request")){
            assert.deepEqual(Object.keys(Line.entry).filter((Key) => !["t", "level", "msg", "ip", "method", "target", "route", "status", "bytesIn", "bytesOut", "ms", "ua"].includes(Key)), [], "an access-log line grew a field");
        }
    });

    it("proxies WebSocket upgrades to the websocket upstream byte for byte", async () => {
        const Result = await Upgrade(P.gateway, "/xmpp?room=ramsgate", { "Sec-WebSocket-Protocol": "xmpp" });
        assert.equal(Result.status, 101);
        const Expected = crypto.createHash("sha1").update(Result.key + WS_GUID).digest("base64");
        assert.equal(Result.headers["sec-websocket-accept"], Expected);
        const Socket = Result.socket!;
        Socket.write("hello over tls");
        assert.equal(await ReadSome(Socket, "echo:hello over tls".length), "echo:hello over tls");
        const Seen = Ws.seen[Ws.seen.length - 1];
        assert.equal(Seen["x-forwarded-for"], "127.0.0.1");
        assert.equal(Seen["x-reforged-gateway"], Config.secret);
        assert.equal(Seen["sec-websocket-protocol"], "xmpp");
        assert.equal(TheGateway.Stats().websockets, 1);
        Socket.destroy();
        await new Promise((resolve) => setTimeout(resolve, 100));
        assert.equal(TheGateway.Stats().websockets, 0);
    });

    it("carries the 1.4.4 client's chat connection as libwebsockets 3.0 sends it: GET //, protocol xmpp", async () => {
        const Key = crypto.randomBytes(16).toString("base64");
        const Socket = tls.connect({ host: "127.0.0.1", port: P.gateway, rejectUnauthorized: false });
        await once(Socket, "secureConnect");
        Socket.write([
            "GET // HTTP/1.1", "Pragma: no-cache", "Cache-Control: no-cache", "Host: 127.0.0.1", "Origin: http://127.0.0.1",
            "Upgrade: websocket", "Connection: Upgrade", `Sec-WebSocket-Key: ${Key}`, "Sec-WebSocket-Protocol: xmpp",
            "Sec-WebSocket-Version: 13", "", ""
        ].join("\r\n"));

        let Head = "";
        while(!Head.includes("\r\n\r\n")){
            const [Chunk] = await once(Socket, "data") as [Buffer];
            Head += Chunk.toString("latin1");
        }
        const Lines = Head.slice(0, Head.indexOf("\r\n\r\n")).split("\r\n");

        try{
            assert.match(Lines[0], /^HTTP\/1\.1 101 /);
            // libwebsockets checks these: one Connection: Upgrade, and the protocol it asked for
            assert.equal(Lines.filter((Line) => /^connection:\s*upgrade$/i.test(Line)).length, 1, Head);
            assert.equal(Lines.filter((Line) => /^sec-websocket-protocol:\s*xmpp$/i.test(Line)).length, 1, Head);
            assert.equal(Ws.urls[Ws.urls.length - 1], "//", "the upstream sees the request target as sent");
            const Seen = Ws.seen[Ws.seen.length - 1];
            assert.equal(Seen["x-reforged-gateway"], Config.secret);
            assert.equal(Seen["x-forwarded-for"], "127.0.0.1");
            assert.equal(Seen["sec-websocket-protocol"], "xmpp");
        }
        finally{
            Socket.destroy();
        }
    });

    it("applies the same rules to upgrades, and passes on an upstream's refusal", async () => {
        const Before = Ws.seen.length;
        const WithKey = await Upgrade(P.gateway, "/xmpp", { "x-mysticparadox-gameserver-apikey": "k" });
        assert.equal(WithKey.status, 403);
        const Admin = await Upgrade(P.gateway, "/reforged/api/ServerStatus");
        assert.equal(Admin.status, 403);
        const Dots = await Upgrade(P.gateway, "/a/../xmpp");
        assert.equal(Dots.status, 400);
        assert.equal(Ws.seen.length, Before);

        const Refused = await Upgrade(P.gateway, "/refuse");
        assert.equal(Refused.status, 404);
        assert.equal(Refused.body, "no room");
    });

    it("answers 502 when an upstream is down (chat off, backend stopped)", async () => {
        assert.equal(await PortOpen(P.dead), false, `something listens on ${P.dead}`);
        const Second = new Gateway(TestConfig({ gateway: P.gateway2, backend: P.dead, content: P.dead, ws: P.dead }), { cert: Cert.certPem, key: Cert.keyPem });
        await Second.Listen();
        try{
            const Ws502 = await Upgrade(P.gateway2, "/xmpp");
            assert.equal(Ws502.status, 502);
            assert.deepEqual(JSON.parse(Ws502.body!), { error: "bad_gateway" });
            const Http502 = await HttpsRequest(P.gateway2, "/heartbeat", { method: "POST", body: "{}" });
            assert.equal(Http502.status, 502);
            assert.deepEqual(Http502.json(), { error: "bad_gateway" });
        }
        finally{
            await Second.Close(200);
        }
    });
});

// Ports: private-mode gateway 62440, its fake backend 62441, its fake content server 62442.
describe("gateway in private mode (plain HTTP inside the tailnet)", () => {
    const PP = { gateway: 62440, backend: 62441, content: 62442 };
    let Backend: FakeUpstream;
    let Content: FakeUpstream;
    let Private: Gateway;

    before(async () => {
        Backend = await StartUpstream(PP.backend, "backend");
        Content = await StartUpstream(PP.content, "content");
        Private = new Gateway(TestConfig({ gateway: PP.gateway, backend: PP.backend, content: PP.content, ws: PP.backend }, { tls: false }), null);
        await Private.Listen();
    });

    after(async () => {
        await Private.Close(500);
        await Backend.close();
        await Content.close();
    });

    it("serves plain HTTP with the same rules, and tells the backend the request came in over http", async () => {
        assert.equal(Private.Certificate, null);
        const Reply = await PlainRequest(PP.gateway, "/__origin/gamedev-prod.steelyard.ca/party/invites");
        assert.equal(Reply.status, 200);
        const Seen = Backend.seen[Backend.seen.length - 1];
        assert.equal(Seen.url, "/__origin/gamedev-prod.steelyard.ca/party/invites", "the target goes upstream unchanged");
        assert.equal(Seen.headers["x-forwarded-proto"], "http");
        assert.equal(Seen.headers["x-forwarded-for"], "127.0.0.1");
        assert.equal((await PlainRequest(PP.gateway, "/content/v1/runtime/manifest")).status, 200);
        assert.equal(Content.seen.length, 1);
        const Before = Backend.seen.length;
        assert.equal((await PlainRequest(PP.gateway, "/reforged/api/CreateInvite", "POST")).status, 403);
        assert.equal((await PlainRequest(PP.gateway, "/launcher/v1/auth/register", "POST")).status, 403);
        assert.equal(Backend.seen.length, Before, "nothing blocked reached the backend");
    });
});

function PlainRequest(Port: number, Target: string, Method = "GET"): Promise<{ status: number; body: string }> {
    return new Promise((resolve, reject) => {
        const Req = http.request({ host: "127.0.0.1", port: Port, path: Target, method: Method, agent: false }, (Res) => {
            let Body = "";
            Res.on("data", (Chunk) => (Body += Chunk));
            Res.on("end", () => resolve({ status: Res.statusCode ?? 0, body: Body }));
        });
        Req.on("error", reject);
        Req.end();
    });
}
