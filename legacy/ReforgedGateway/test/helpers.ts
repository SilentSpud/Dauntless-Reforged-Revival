import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import https from "node:https";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import tls from "node:tls";
import { DEFAULT_LIMITS, DEFAULT_TIMEOUTS, GatewayConfig } from "../src/config";
import { LogLevel, SetLogSink } from "../src/log";
import { PUBLIC_REFORGED_API } from "../src/policy";

// Tests only ever listen on these spare loopback ports (62400-62499), never on the live stack's.
export const ROOT = path.resolve(__dirname, "..", "..");
export const TOOL = path.join(ROOT, "tools", "make-cert.js");

// Temp folders (throwaway certificates and keys, audit logs) are removed when the test process ends.
const TEMP_DIRS: string[] = [];
process.once("exit", () => {
    for(const Dir of TEMP_DIRS){
        try{
            fs.rmSync(Dir, { recursive: true, force: true });
        }
        catch{
            // A child process may still hold a file open; the OS temp cleanup gets it later.
        }
    }
});

export function TempDir(Name: string): string {
    const Dir = fs.mkdtempSync(path.join(os.tmpdir(), `rr-gateway-${Name}-`));
    TEMP_DIRS.push(Dir);
    return Dir;
}

export type TestCert = { cert: string; key: string; certPem: string; keyPem: string; fingerprint: string; notBefore: string; notAfter: string; names: string };

// A throwaway certificate made by the real tool, in a temp folder.
export function MakeCert(Hosts: string[] = ["127.0.0.1"]): TestCert {
    const Dir = TempDir("cert");
    const Args = [TOOL, ...Hosts.flatMap((Host) => ["--host", Host]), "--out", Dir, "--json"];
    const Result = spawnSync(process.execPath, Args, { encoding: "utf8" });
    if(Result.status !== 0){
        throw new Error(`make-cert failed: ${Result.stderr}`);
    }
    const Out = JSON.parse(Result.stdout);
    return { ...Out, certPem: fs.readFileSync(Out.cert, "utf8"), keyPem: fs.readFileSync(Out.key, "utf8") };
}

export type Seen = { method: string; url: string; headers: http.IncomingHttpHeaders; body: Buffer; aborted: boolean };

export type FakeUpstream = {
    server: http.Server;
    seen: Seen[];
    close: () => Promise<void>;
};

export type FakeHandler = (Req: http.IncomingMessage, Res: http.ServerResponse, Body: Buffer) => boolean | void;

// A plain HTTP upstream on 127.0.0.1 that records every request and answers with a JSON echo,
// unless Handler takes the request (returns true).
export function StartUpstream(Port: number, Name: string, Handler?: FakeHandler): Promise<FakeUpstream> {
    const Seen: Seen[] = [];
    const Server = http.createServer((Req, Res) => {
        const Chunks: Buffer[] = [];
        const Record: Seen = { method: Req.method ?? "", url: Req.url ?? "", headers: Req.headers, body: Buffer.alloc(0), aborted: false };
        Seen.push(Record);
        Req.on("data", (Chunk: Buffer) => Chunks.push(Chunk));
        Req.on("aborted", () => {
            Record.aborted = true;
        });
        Req.on("error", () => {
            Record.aborted = true;
        });
        Req.on("end", () => {
            Record.body = Buffer.concat(Chunks);
            if(Handler !== undefined && Handler(Req, Res, Record.body) === true){
                return;
            }
            const Text = JSON.stringify({ upstream: Name, method: Req.method, url: Req.url, headers: Req.headers, bodyLength: Record.body.length, bodySha: crypto.createHash("sha256").update(Record.body).digest("hex") });
            Res.writeHead(200, { "content-type": "application/json", "x-powered-by": "Express", "content-length": Buffer.byteLength(Text) });
            Res.end(Text);
        });
    });
    return new Promise((resolve, reject) => {
        Server.once("error", reject);
        Server.listen(Port, "127.0.0.1", () => resolve({
            server: Server,
            seen: Seen,
            close: () => new Promise<void>((done) => {
                Server.close(() => done());
                Server.closeAllConnections();
            }),
        }));
    });
}

export function TestConfig(Ports: { gateway: number; backend: number; content: number; ws: number }, Overrides: Partial<GatewayConfig> = {}): GatewayConfig {
    return {
        bindHost: "127.0.0.1",
        port: Ports.gateway,
        tls: true,
        certFile: "",
        keyFile: "",
        secret: "test-gateway-secret-0123456789abcdef0123456789abcdef",
        backend: { host: "127.0.0.1", port: Ports.backend },
        content: { host: "127.0.0.1", port: Ports.content },
        ws: { host: "127.0.0.1", port: Ports.ws },
        allowlist: undefined,
        limits: structuredClone(DEFAULT_LIMITS),
        timeouts: { ...DEFAULT_TIMEOUTS, checkIntervalMs: 200, shutdownGraceMs: 2000 },
        ...Overrides,
    };
}

export type Reply = { status: number; headers: http.IncomingHttpHeaders; body: Buffer; json: () => any; peerFingerprint: string };

export type RequestOptions = {
    method?: string;
    headers?: Record<string, string | string[]>;
    body?: string | Buffer;
    localAddress?: string;
    // The fingerprint the client pins (like the launcher's relay). Default: accept any.
    pin?: string;
};

function Sha256Hex(Data: Buffer): string {
    return crypto.createHash("sha256").update(Data).digest("hex");
}

// A TLS connection that is only handed to the request after the server's certificate matched the
// pin. (With rejectUnauthorized: false Node never calls checkServerIdentity for a self-signed
// certificate, so the pin must be checked on secureConnect, before any request byte is sent.
// The launcher's relay does the same in legacy/ReforgedLauncher/src/main/pinned.ts.)
function PinnedConnection(Pin: string | undefined, LocalAddress: string | undefined, OnPeer: (Fingerprint: string) => void) {
    return (Options: any, Callback: (Error: Error | null, Socket?: tls.TLSSocket) => void) => {
        const Socket = tls.connect({ host: Options.host, port: Options.port, rejectUnauthorized: false, ...(LocalAddress === undefined ? {} : { localAddress: LocalAddress }) } as tls.ConnectionOptions);
        let Done = false;
        Socket.once("secureConnect", () => {
            const Fingerprint = Sha256Hex(Socket.getPeerCertificate(true).raw);
            OnPeer(Fingerprint);
            Done = true;
            if(Pin !== undefined && Fingerprint !== Pin){
                const Error_ = new Error(`certificate fingerprint ${Fingerprint} is not the pinned ${Pin}`);
                Socket.destroy();
                Callback(Error_);
                return;
            }
            Callback(null, Socket);
        });
        Socket.once("error", (Error_) => {
            if(!Done){
                Done = true;
                Callback(Error_);
            }
        });
        return undefined;
    };
}

// One HTTPS request on a fresh connection; the path goes on the wire exactly as given.
export function HttpsRequest(Port: number, RawPath: string, Options: RequestOptions = {}): Promise<Reply> {
    return new Promise((resolve, reject) => {
        let Peer = "";
        const Req = https.request({
            host: "127.0.0.1",
            port: Port,
            method: Options.method ?? "GET",
            path: RawPath,
            headers: Options.headers,
            createConnection: PinnedConnection(Options.pin, Options.localAddress, (Fingerprint) => {
                Peer = Fingerprint;
            }) as never,
        }, (Res) => {
            const Chunks: Buffer[] = [];
            Res.on("data", (Chunk: Buffer) => Chunks.push(Chunk));
            Res.on("end", () => {
                const Body = Buffer.concat(Chunks);
                resolve({ status: Res.statusCode ?? 0, headers: Res.headers, body: Body, json: () => JSON.parse(Body.toString("utf8")), peerFingerprint: Peer });
            });
            Res.on("error", reject);
        });
        Req.on("error", reject);
        if(Options.body !== undefined){
            Req.end(Options.body);
        }
        else{
            Req.end();
        }
    });
}

// A raw TLS connection (for slow clients, handshakes that never finish, and hand-written requests).
export function TlsConnect(Port: number, LocalAddress?: string): Promise<tls.TLSSocket> {
    return new Promise((resolve, reject) => {
        const Raw = net.connect({ host: "127.0.0.1", port: Port, localAddress: LocalAddress });
        Raw.once("error", reject);
        const Socket = tls.connect({ socket: Raw, rejectUnauthorized: false }, () => resolve(Socket));
        Socket.once("error", reject);
    });
}

export function TcpConnect(Port: number, LocalAddress?: string): Promise<net.Socket> {
    return new Promise((resolve, reject) => {
        const Socket = net.connect({ host: "127.0.0.1", port: Port, localAddress: LocalAddress }, () => resolve(Socket));
        Socket.once("error", reject);
    });
}

export function WaitClose(Socket: { once: (Event: "close", Listener: () => void) => unknown }, TimeoutMs: number): Promise<number> {
    const Started = Date.now();
    return new Promise((resolve, reject) => {
        const Timer = setTimeout(() => reject(new Error(`socket still open after ${TimeoutMs} ms`)), TimeoutMs);
        Socket.once("close", () => {
            clearTimeout(Timer);
            resolve(Date.now() - Started);
        });
    });
}

export async function WaitFor(Condition: () => boolean | Promise<boolean>, TimeoutMs = 5000, What = "condition"){
    const Until = Date.now() + TimeoutMs;
    while(Date.now() < Until){
        if(await Condition()){
            return;
        }
        await Sleep(20);
    }
    throw new Error(`timed out waiting for ${What}`);
}

export function Sleep(Ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, Ms));
}

export type CapturedLog = { lines: { level: LogLevel; entry: any; raw: string }[]; stop: () => void };

export function CaptureLog(): CapturedLog {
    const Lines: CapturedLog["lines"] = [];
    SetLogSink((Raw, Level) => Lines.push({ level: Level, entry: JSON.parse(Raw), raw: Raw }));
    return { lines: Lines, stop: () => SetLogSink(undefined) };
}

// Is anything listening on this loopback port? (Used to prove a "dead" upstream port is dead.)
export function PortOpen(Port: number): Promise<boolean> {
    return new Promise((resolve) => {
        const Socket = net.connect({ host: "127.0.0.1", port: Port });
        Socket.once("connect", () => {
            Socket.destroy();
            resolve(true);
        });
        Socket.once("error", () => resolve(false));
    });
}

// Every route the backend registers under /reforged/api, read from the backend patch
// (patch 0002 in docs/paradox-patches.md, now in paradox/backend), so a route added there later is
// covered without touching this test.
export function ReforgedApiRoutes(): { method: string; path: string }[] {
    const Source = fs.readFileSync(path.join(ROOT, "..", "..", "upstream", "patches", "0002-backend-reforged-launcher-api.patch"), "utf8");
    const Routes: { method: string; path: string }[] = [];
    for(const Match of Source.matchAll(/^\+\s*reforgedApiRouter\.(get|post|put|delete|patch)\(\s*"([^"]+)"/gm)){
        if(Match[2].startsWith("/reforged/api/")) Routes.push({ method: Match[1].toUpperCase(), path: Match[2].replace(/:[A-Za-z]+/g, "ABCD-EFGH-JKMN") });
    }
    return Routes;
}

export function IsPublicRoute(Method: string, Path: string): boolean {
    const Name = Path.slice("/reforged/api/".length).toLowerCase();
    return PUBLIC_REFORGED_API.get(Name)?.includes(Method) === true;
}
