import assert from "node:assert/strict";
import https from "node:https";
import { after, before, describe, it } from "node:test";
import { GatewayConfig } from "../src/config";
import { Gateway } from "../src/gateway";
import { CaptureLog, CapturedLog, FakeUpstream, HttpsRequest, MakeCert, Sleep, StartUpstream, TcpConnect, TestCert, TestConfig, TlsConnect, WaitClose } from "./helpers";

// Ports: gateways 62410 (body limits), 62417 (rate limits), 62414 (connections), 62415 (timeouts),
// 62416 (shutdown); fake backend 62411, fake content 62412; 62413 is the (unused) ws upstream.
const P = { limits: 62410, rate: 62417, backend: 62411, content: 62412, ws: 62413, connections: 62414, timeouts: 62415, shutdown: 62416 };

function Ports(Gateway: number){
    return { gateway: Gateway, backend: P.backend, content: P.content, ws: P.ws };
}

// Sends a chunked body in pieces and resolves with the status, even if the gateway hangs up early.
function ChunkedPost(Port: number, Path: string, Total: number, ChunkSize: number): Promise<{ status: number; body: string }> {
    return new Promise((resolve, reject) => {
        let Status = 0;
        let Body = "";
        const Req = https.request({ host: "127.0.0.1", port: Port, method: "POST", path: Path, agent: false, rejectUnauthorized: false, headers: { "content-type": "application/octet-stream" } }, (Res) => {
            Status = Res.statusCode ?? 0;
            Res.on("data", (Chunk) => Body += Chunk);
            Res.on("end", () => resolve({ status: Status, body: Body }));
            Res.on("error", () => resolve({ status: Status, body: Body }));
        });
        // The gateway may hang up while the rest of the body is still being written.
        Req.on("error", (error) => Status !== 0 ? setTimeout(() => resolve({ status: Status, body: Body }), 50) : reject(error));
        let Sent = 0;
        const Pump = () => {
            while(Sent < Total){
                Sent += ChunkSize;
                if(!Req.write(Buffer.alloc(ChunkSize, 0x61))){
                    Req.once("drain", Pump);
                    return;
                }
            }
            Req.end();
        };
        Pump();
    });
}

describe("gateway limits", () => {
    let Cert: TestCert;
    let Meta: FakeUpstream;
    let Content: FakeUpstream;
    let Log: CapturedLog;
    const Gateways: Gateway[] = [];

    async function Start(Port: number, Change: (Config: GatewayConfig) => void): Promise<Gateway> {
        const Config = TestConfig(Ports(Port));
        Change(Config);
        const Instance = new Gateway(Config, { cert: Cert.certPem, key: Cert.keyPem });
        await Instance.Listen();
        Gateways.push(Instance);
        return Instance;
    }

    before(async () => {
        Log = CaptureLog();
        Cert = MakeCert(["127.0.0.1"]);
        Meta = await StartUpstream(P.backend, "backend", (Req, Res) => {
            if(Req.url === "/slow"){
                setTimeout(() => {
                    Res.writeHead(200, { "content-type": "text/plain" });
                    Res.end("slow done");
                }, 800);
                return true;
            }
            if(Req.url === "/hang"){
                return true;
            }
            return false;
        });
        Content = await StartUpstream(P.content, "content");
    });

    after(async () => {
        for(const Instance of Gateways){
            await Instance.Close(200);
        }
        await Promise.all([Meta.close(), Content.close()]);
        Log.stop();
    });

    describe("request bodies", () => {
        before(async () => {
            await Start(P.limits, (Config) => {
                Config.limits.maxBodyBytes = 4096;
                Config.limits.rate.general = { burst: 1000, perMinute: 6000 };
            });
        });

        it("refuses a Content-Length over the cap with 413 before reading it, and the upstream never sees it", async () => {
            const Before = Meta.seen.length;
            const Reply = await HttpsRequest(P.limits, "/character", { method: "POST", body: Buffer.alloc(4097, 0x61), headers: { "content-type": "application/json" } });
            assert.equal(Reply.status, 413);
            assert.deepEqual(Reply.json(), { error: "body_too_large" });
            assert.equal(Reply.headers["connection"], "close");
            assert.equal(Meta.seen.length, Before);
        });

        it("passes a body of exactly the cap", async () => {
            const Reply = await HttpsRequest(P.limits, "/character", { method: "POST", body: Buffer.alloc(4096, 0x61) });
            assert.equal(Reply.status, 200);
            assert.equal(Reply.json().bodyLength, 4096);
        });

        it("cuts a chunked body at the cap with 413 and aborts the upstream request", async () => {
            const Before = Meta.seen.length;
            const Reply = await ChunkedPost(P.limits, "/character", 64 * 1024, 1024);
            assert.equal(Reply.status, 413);
            assert.deepEqual(JSON.parse(Reply.body), { error: "body_too_large" });
            await Sleep(100);
            const Forwarded = Meta.seen.slice(Before);
            // Either nothing reached the upstream yet, or what did was aborted short of the cap.
            for(const Seen of Forwarded){
                assert.ok(Seen.aborted || Seen.body.length <= 4096, "the upstream got more than the cap");
            }
            const Line = Log.lines.find((Entry) => Entry.entry.msg === "request" && Entry.entry.reason === "body_too_large" && Entry.entry.bytesIn > 4096);
            assert.ok(Line, "no access-log line for the cut body");
        });

        it("answers Expect: 100-continue itself: 413 without 100 for a body over the cap, 100 then the answer otherwise", async () => {
            const Send = (Length: number) => new Promise<{ status: number; continued: boolean; body: string }>((resolve, reject) => {
                let Continued = false;
                const Req = https.request({ host: "127.0.0.1", port: P.limits, method: "POST", path: "/character", agent: false, rejectUnauthorized: false, headers: { expect: "100-continue", "content-length": String(Length) } }, (Res) => {
                    let Body = "";
                    Res.on("data", (Chunk) => Body += Chunk);
                    Res.on("end", () => resolve({ status: Res.statusCode ?? 0, continued: Continued, body: Body }));
                });
                Req.on("continue", () => {
                    Continued = true;
                    Req.end(Buffer.alloc(Length, 0x62));
                });
                Req.on("error", reject);
                Req.flushHeaders();
            });
            const Big = await Send(5000);
            assert.equal(Big.status, 413);
            assert.equal(Big.continued, false);
            const Small = await Send(100);
            assert.equal(Small.status, 200);
            assert.equal(Small.continued, true);
            const Echo = JSON.parse(Small.body);
            assert.equal(Echo.bodyLength, 100);
            assert.equal(Echo.headers["expect"], undefined);
        });

        it("a chunked body under the cap goes through", async () => {
            const Reply = await ChunkedPost(P.limits, "/character", 3000, 1000);
            assert.equal(Reply.status, 200);
            assert.equal(JSON.parse(Reply.body).bodyLength, 3000);
        });
    });

    describe("rate limits", () => {
        before(async () => {
            await Start(P.rate, (Config) => {
                Config.limits.rate.general = { burst: 5, perMinute: 60 };
                Config.limits.rate.content = { burst: 3, perMinute: 60 };
                Config.limits.rate.register = { burst: 2, perMinute: 0.2 };
                Config.limits.rate.token = { burst: 2, perMinute: 1 };
                Config.limits.rate.ws = { burst: 3, perMinute: 1 };
            });
        });
        const Port = P.rate;

        it("allows a burst per address, then answers 429 with Retry-After without asking the upstream", async () => {
            for(let Index = 0; Index < 5; Index++){
                assert.equal((await HttpsRequest(Port, "/party/invites", { localAddress: "127.0.0.20" })).status, 200);
            }
            const Before = Meta.seen.length;
            const Limited = await HttpsRequest(Port, "/party/invites", { localAddress: "127.0.0.20" });
            assert.equal(Limited.status, 429);
            assert.deepEqual(Limited.json(), { error: "rate_limited" });
            assert.equal(Limited.headers["retry-after"], "1");
            assert.equal(Meta.seen.length, Before);
            // Another address is not affected.
            assert.equal((await HttpsRequest(Port, "/party/invites", { localAddress: "127.0.0.21" })).status, 200);
            // A refill of one per second lets one more through.
            await Sleep(1100);
            assert.equal((await HttpsRequest(Port, "/party/invites", { localAddress: "127.0.0.20" })).status, 200);
            assert.equal((await HttpsRequest(Port, "/party/invites", { localAddress: "127.0.0.20" })).status, 429);
        });

        it("counts downloads in their own bucket", async () => {
            for(let Index = 0; Index < 3; Index++){
                assert.equal((await HttpsRequest(Port, "/content/v1/manifest", { localAddress: "127.0.0.20" })).status, 200);
            }
            assert.equal((await HttpsRequest(Port, "/content/v1/manifest", { localAddress: "127.0.0.20" })).status, 429);
        });

        it("limits Register strictly", async () => {
            const Body = JSON.stringify({ Username: "Someone", InviteCode: "ABCD-EFGH-JKMN" });
            assert.equal((await HttpsRequest(Port, "/reforged/api/Register", { method: "POST", body: Body, localAddress: "127.0.0.22" })).status, 200);
            assert.equal((await HttpsRequest(Port, "/reforged/api/register", { method: "POST", body: Body, localAddress: "127.0.0.22" })).status, 200);
            const Limited = await HttpsRequest(Port, "/reforged/api/Register", { method: "POST", body: Body, localAddress: "127.0.0.22" });
            assert.equal(Limited.status, 429);
            assert.equal(Limited.headers["retry-after"], "300");
            // Ordinary requests from that address still work: separate bucket.
            assert.equal((await HttpsRequest(Port, "/reforged/api/ServerStatus", { localAddress: "127.0.0.22" })).status, 200);
        });

        it("limits the login token route strictly", async () => {
            const Form = { method: "POST", body: "grant_type=exchange_code&exchange_code=x", headers: { "content-type": "application/x-www-form-urlencoded" }, localAddress: "127.0.0.23" };
            assert.equal((await HttpsRequest(Port, "/account/api/oauth/token", Form)).status, 200);
            assert.equal((await HttpsRequest(Port, "/account/api/oauth/token", Form)).status, 200);
            const Limited = await HttpsRequest(Port, "/account/api/oauth/token", Form);
            assert.equal(Limited.status, 429);
            assert.equal(Limited.headers["retry-after"], "60");
            const Line = Log.lines.find((Entry) => Entry.entry.status === 429 && Entry.entry.reason === "rate_limited:token");
            assert.ok(Line);
            assert.equal(Line!.entry.ip, "127.0.0.23");
        });

        it("counts WebSocket upgrades in their own bucket: a burst of chat reconnects never 429s the game's HTTP traffic", async () => {
            const UpgradeStatus = () => new Promise<number>((resolve, reject) => {
                const Req = https.request({ host: "127.0.0.1", port: Port, path: "//", agent: false, rejectUnauthorized: false, localAddress: "127.0.0.24", headers: { Connection: "Upgrade", Upgrade: "websocket", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==", "Sec-WebSocket-Version": "13", "Sec-WebSocket-Protocol": "xmpp" } });
                Req.on("response", (Res) => {
                    Res.resume();
                    resolve(Res.statusCode ?? 0);
                });
                Req.on("upgrade", (_Res, Socket) => {
                    Socket.destroy();
                    resolve(101);
                });
                Req.on("error", reject);
                Req.end();
            });

            for(let Index = 0; Index < 3; Index++){
                assert.notEqual(await UpgradeStatus(), 429);
            }
            assert.equal(await UpgradeStatus(), 429, "the fourth upgrade is over the ws bucket");
            assert.equal((await HttpsRequest(Port, "/heartbeat", { method: "POST", body: "{}", localAddress: "127.0.0.24" })).status, 200, "the general bucket is untouched");
            const Line = Log.lines.find((Entry) => Entry.entry.status === 429 && Entry.entry.reason === "rate_limited:ws");
            assert.ok(Line);
        });

        it("counts the game's HTTP traffic apart from upgrades: an empty general bucket still lets chat connect", async () => {
            for(let Index = 0; Index < 5; Index++){
                assert.equal((await HttpsRequest(Port, "/party/invites", { localAddress: "127.0.0.25" })).status, 200);
            }
            assert.equal((await HttpsRequest(Port, "/party/invites", { localAddress: "127.0.0.25" })).status, 429);
            const Status = await new Promise<number>((resolve, reject) => {
                const Req = https.request({ host: "127.0.0.1", port: Port, path: "/xmpp", agent: false, rejectUnauthorized: false, localAddress: "127.0.0.25", headers: { Connection: "Upgrade", Upgrade: "websocket", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==", "Sec-WebSocket-Version": "13" } });
                Req.on("response", (Res) => {
                    Res.resume();
                    resolve(Res.statusCode ?? 0);
                });
                Req.on("upgrade", (_Res, Socket) => {
                    Socket.destroy();
                    resolve(101);
                });
                Req.on("error", reject);
                Req.end();
            });
            assert.notEqual(Status, 429);
        });
    });

    describe("connections", () => {
        before(async () => {
            await Start(P.connections, (Config) => {
                Config.limits.maxConnectionsPerIp = 3;
                Config.limits.rate.connect = { burst: 8, perMinute: 60 };
            });
        });

        it("holds at most maxConnectionsPerIp open connections per address", async () => {
            const Open = [await TlsConnect(P.connections, "127.0.0.30"), await TlsConnect(P.connections, "127.0.0.30"), await TlsConnect(P.connections, "127.0.0.30")];
            await assert.rejects(TlsConnect(P.connections, "127.0.0.30"));
            // Someone else still gets in.
            const Other = await TlsConnect(P.connections, "127.0.0.31");
            Other.destroy();
            // Closing one frees a slot.
            Open[0].destroy();
            await Sleep(100);
            const Again = await TlsConnect(P.connections, "127.0.0.30");
            for(const Socket of [...Open, Again]){
                Socket.destroy();
            }
            const Line = Log.lines.find((Entry) => Entry.entry.msg === "connection refused: too many open connections from one address");
            assert.ok(Line);
        });

        it("limits how fast one address opens new connections", async () => {
            await Sleep(100);
            let Opened = 0;
            let Refused = 0;
            for(let Index = 0; Index < 10; Index++){
                try{
                    const Socket = await TlsConnect(P.connections, "127.0.0.32");
                    Opened++;
                    Socket.destroy();
                    await Sleep(20);
                }
                catch{
                    Refused++;
                }
            }
            assert.equal(Opened, 8);
            assert.equal(Refused, 2);
        });
    });

    describe("timeouts", () => {
        before(async () => {
            await Start(P.timeouts, (Config) => {
                Config.timeouts.headersMs = 1000;
                Config.timeouts.requestMs = 2000;
                Config.timeouts.handshakeMs = 1000;
                Config.timeouts.keepAliveMs = 1000;
                Config.timeouts.idleMs = 3000;
                Config.timeouts.checkIntervalMs = 200;
            });
        });

        it("drops a client that sends its headers too slowly (slowloris)", async () => {
            const Socket = await TlsConnect(P.timeouts);
            let Received = "";
            Socket.on("data", (Chunk) => Received += Chunk);
            Socket.write("GET /party/invites HTTP/1.1\r\nHost: 127.0.0.1\r\n");
            const Drip = setInterval(() => {
                if(!Socket.destroyed){
                    Socket.write("X-Slow: 1\r\n");
                }
            }, 250);
            const Ms = await WaitClose(Socket, 5000);
            clearInterval(Drip);
            assert.ok(Ms >= 800 && Ms < 3000, `closed after ${Ms} ms`);
            assert.match(Received, /^HTTP\/1\.1 408/);
        });

        it("drops a client that sends its body too slowly, and the upstream request with it", async () => {
            const Before = Meta.seen.length;
            const Socket = await TlsConnect(P.timeouts);
            Socket.on("data", () => undefined);
            Socket.write("POST /character HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n");
            const Drip = setInterval(() => {
                if(!Socket.destroyed){
                    Socket.write("aaaaa");
                }
            }, 300);
            const Ms = await WaitClose(Socket, 6000);
            clearInterval(Drip);
            assert.ok(Ms >= 1800 && Ms < 4500, `closed after ${Ms} ms`);
            await Sleep(100);
            const Forwarded = Meta.seen.slice(Before);
            assert.ok(Forwarded.every((Seen) => Seen.aborted), "the upstream request was not aborted");
        });

        it("drops a connection that never finishes the TLS handshake", async () => {
            const Socket = await TcpConnect(P.timeouts);
            Socket.on("data", () => undefined);
            Socket.on("error", () => undefined);
            const Ms = await WaitClose(Socket, 5000);
            assert.ok(Ms < 3500, `closed after ${Ms} ms`);
        });

        it("closes idle keep-alive connections", async () => {
            const Socket = await TlsConnect(P.timeouts);
            let Received = "";
            Socket.on("data", (Chunk) => Received += Chunk);
            Socket.write("GET /party/invites HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
            const Ms = await WaitClose(Socket, 5000);
            assert.match(Received, /^HTTP\/1\.1 200/);
            assert.ok(Ms >= 900 && Ms < 3500, `closed after ${Ms} ms`);
        });
    });

    describe("graceful shutdown", () => {
        it("lets a request in flight finish with Connection: close, refuses new connections, then stops", async () => {
            const Instance = await Start(P.shutdown, () => undefined);
            const InFlight = HttpsRequest(P.shutdown, "/slow");
            await Sleep(200);
            const Started = Date.now();
            const Closing = Instance.Close(5000);
            await Sleep(50);
            await assert.rejects(HttpsRequest(P.shutdown, "/party/invites"));
            const Reply = await InFlight;
            assert.equal(Reply.status, 200);
            assert.equal(Reply.body.toString(), "slow done");
            assert.equal(Reply.headers["connection"], "close");
            await Closing;
            assert.ok(Date.now() - Started < 3000, "close waited for the full grace period");
        });

        it("cuts what is still running when the grace period ends", async () => {
            const Instance = await Start(P.shutdown, () => undefined);
            const Hanging = HttpsRequest(P.shutdown, "/hang").then(() => "answered", () => "cut");
            await Sleep(200);
            const Started = Date.now();
            await Instance.Close(500);
            const Ms = Date.now() - Started;
            assert.ok(Ms >= 450 && Ms < 2500, `closed after ${Ms} ms`);
            assert.equal(await Hanging, "cut");
        });
    });
});
