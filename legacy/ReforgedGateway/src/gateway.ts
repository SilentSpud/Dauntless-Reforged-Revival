import crypto from "node:crypto";
import http from "node:http";
import https from "node:https";
import net from "node:net";
import tls from "node:tls";
import { Endpoint, GatewayConfig } from "./config";
import { AllowlistFeed } from "./feed";
import { PeerAddress, RateLimitKey } from "./ip";
import { logger, LogThrottle } from "./log";
import { Classify, Decision, HasBearer, LimitClass, Upstream } from "./policy";
import { RateLimiter } from "./ratelimit";
import { RedactTarget, RedactUserAgent } from "./redact";

// Hop-by-hop headers (RFC 9110 7.6.1) never cross the gateway in either direction.
const HOP_BY_HOP = new Set([
    "connection", "keep-alive", "proxy-connection", "proxy-authenticate", "proxy-authorization",
    "te", "trailer", "trailers", "transfer-encoding", "upgrade",
]);

// Client-supplied address and origin claims are dropped; the gateway sets its own.
const CLIENT_CLAIMS = new Set([
    "x-forwarded-for", "x-forwarded-proto", "x-forwarded-host", "x-forwarded-port", "x-forwarded-prefix",
    "forwarded", "x-real-ip", "x-client-ip", "true-client-ip", "cf-connecting-ip", "x-reforged-gateway",
    // The gateway already answered any 100-continue itself.
    "expect",
]);

const RESPONSE_DROP = new Set(["x-powered-by", "x-reforged-gateway"]);

export type CertInfo = {
    fingerprint: string;
    notBefore: Date;
    notAfter: Date;
    subjectAltName: string;
};

// SHA-256 of the certificate's DER encoding, 64 lowercase hex: the "fp" of a v2 invite.
export function DescribeCertificate(Pem: string | Buffer): CertInfo {
    const Cert = new crypto.X509Certificate(Pem);
    return {
        fingerprint: crypto.createHash("sha256").update(Cert.raw).digest("hex"),
        notBefore: new Date(Cert.validFrom),
        notAfter: new Date(Cert.validTo),
        subjectAltName: Cert.subjectAltName ?? "",
    };
}

function ConnectionTokens(Value: string | string[] | undefined): Set<string> {
    const Out = new Set<string>();
    const Joined = Array.isArray(Value) ? Value.join(",") : Value ?? "";
    for(const Token of Joined.split(",")){
        const Name = Token.trim().toLowerCase();
        if(Name !== ""){
            Out.add(Name);
        }
    }
    return Out;
}

export function ForwardHeaders(Incoming: http.IncomingHttpHeaders, Ip: string, Secret: string, Upgrade: boolean, Proto: "https" | "http" = "https"): http.OutgoingHttpHeaders {
    const Named = ConnectionTokens(Incoming["connection"]);
    const Out: http.OutgoingHttpHeaders = {};
    for(const [Name, Value] of Object.entries(Incoming)){
        if(Value === undefined || HOP_BY_HOP.has(Name) || Named.has(Name) || CLIENT_CLAIMS.has(Name)){
            continue;
        }
        Out[Name] = Value;
    }
    // Overwritten, never appended: the backend trusts exactly one address, the real peer.
    Out["x-forwarded-for"] = Ip;
    Out["x-forwarded-proto"] = Proto;
    Out["x-reforged-gateway"] = Secret;
    if(Upgrade){
        Out["connection"] = "Upgrade";
        Out["upgrade"] = "websocket";
    }
    return Out;
}

function ResponseHeaders(Incoming: http.IncomingHttpHeaders, Close: boolean): http.OutgoingHttpHeaders {
    const Named = ConnectionTokens(Incoming["connection"]);
    const Out: http.OutgoingHttpHeaders = {};
    for(const [Name, Value] of Object.entries(Incoming)){
        if(Value === undefined || HOP_BY_HOP.has(Name) || Named.has(Name) || RESPONSE_DROP.has(Name)){
            continue;
        }
        Out[Name] = Value;
    }
    if(Close){
        Out["connection"] = "close";
    }
    return Out;
}

type AccessEntry = {
    ip: string;
    method: string;
    target: string;
    route: Upstream | "gateway";
    status: number;
    bytesIn: number;
    bytesOut: number;
    reason?: string;
    ua?: string;
};

export type GatewayStats = {
    connections: number;
    trackedPeers: number;
    websockets: number;
};

export class Gateway {
    readonly Server: http.Server | https.Server;
    // Null in private mode (plain HTTP inside the tailnet).
    readonly Certificate: CertInfo | null;
    private readonly Agent: http.Agent;
    private readonly Limiters: Record<LimitClass, RateLimiter>;
    private readonly ConnectLimiter: RateLimiter;
    private readonly PerIp = new Map<string, number>();
    private readonly Upgraded = new Set<net.Socket>();
    private readonly Throttle = new LogThrottle(60_000);
    private readonly SweepTimer: NodeJS.Timeout;
    private Closing = false;

    constructor(private readonly Config: GatewayConfig, Tls: { cert: string | Buffer; key: string | Buffer } | null, private readonly Feed?: AllowlistFeed){
        if(Config.tls && Tls === null){
            throw new Error("the TLS gateway needs its certificate and key");
        }
        this.Certificate = Config.tls && Tls !== null ? DescribeCertificate(Tls.cert) : null;

        const Rates = Config.limits.rate;
        this.Limiters = {
            general: new RateLimiter(Rates.general),
            content: new RateLimiter(Rates.content),
            register: new RateLimiter(Rates.register),
            token: new RateLimiter(Rates.token),
            ws: new RateLimiter(Rates.ws),
        };
        this.ConnectLimiter = new RateLimiter(Rates.connect);

        this.Agent = new http.Agent({ keepAlive: true, maxSockets: 512, maxFreeSockets: 64, scheduling: "lifo" });

        const Timeouts = Config.timeouts;
        const HttpOptions: http.ServerOptions = {
            headersTimeout: Timeouts.headersMs,
            requestTimeout: Timeouts.requestMs,
            keepAliveTimeout: Timeouts.keepAliveMs,
            connectionsCheckingInterval: Timeouts.checkIntervalMs,
            maxHeaderSize: 16 * 1024,
        };
        this.Server = Config.tls && Tls !== null
            ? https.createServer({
                ...HttpOptions,
                cert: Tls.cert,
                key: Tls.key,
                minVersion: "TLSv1.2",
                ALPNProtocols: ["http/1.1"],
                handshakeTimeout: Timeouts.handshakeMs,
            })
            : http.createServer(HttpOptions);
        this.Server.maxConnections = Config.limits.maxConnections;
        this.Server.maxHeadersCount = Config.limits.maxHeadersCount;
        // A connection that moves no bytes for this long is dropped (a stalled download included).
        this.Server.setTimeout(Timeouts.idleMs);

        this.Server.on("connection", (Socket: net.Socket) => this.OnConnection(Socket));
        this.Server.on("request", (Req: http.IncomingMessage, Res: http.ServerResponse) => this.OnRequest(Req, Res, false));
        this.Server.on("checkContinue", (Req: http.IncomingMessage, Res: http.ServerResponse) => this.OnRequest(Req, Res, true));
        this.Server.on("upgrade", (Req: http.IncomingMessage, Socket: net.Socket, Head: Buffer) => this.OnUpgrade(Req, Socket, Head));
        this.Server.on("clientError", (Error: NodeJS.ErrnoException, Socket: net.Socket) => this.OnClientError(Error, Socket));
        this.Server.on("tlsClientError", (Error: Error, Socket: tls.TLSSocket) => {
            if(this.Throttle.Allow(`tls:${PeerAddress(Socket.remoteAddress)}`) !== undefined){
                logger.debug("tls handshake failed", { ip: PeerAddress(Socket.remoteAddress), error: Error.message });
            }
        });
        this.Server.on("drop", (Data?: { remoteAddress?: string }) => {
            if(this.Throttle.Allow("drop") !== undefined){
                logger.warn("connection refused: the gateway is at its connection limit", { ip: PeerAddress(Data?.remoteAddress), max: Config.limits.maxConnections });
            }
        });

        this.SweepTimer = setInterval(() => {
            const Now = Date.now();
            for(const Limiter of [...Object.values(this.Limiters), this.ConnectLimiter]){
                Limiter.Sweep(Now);
            }
        }, 60_000);
        this.SweepTimer.unref();
    }

    Listen(): Promise<void> {
        return new Promise((resolve, reject) => {
            const OnError = (Error: Error) => reject(Error);
            this.Server.once("error", OnError);
            this.Server.listen(this.Config.port, this.Config.bindHost, () => {
                this.Server.off("error", OnError);
                resolve();
            });
        });
    }

    Stats(): GatewayStats {
        let Connections = 0;
        for(const Count of this.PerIp.values()){
            Connections += Count;
        }
        return { connections: Connections, trackedPeers: this.PerIp.size, websockets: this.Upgraded.size };
    }

    // Stops accepting, lets requests in flight finish (their answers carry Connection: close),
    // then cuts whatever is left after the grace period. WebSockets have no natural end and are
    // closed at once; the chat client reconnects.
    async Close(GraceMs = this.Config.timeouts.shutdownGraceMs): Promise<void> {
        this.Closing = true;
        clearInterval(this.SweepTimer);
        const Closed = new Promise<void>((resolve) => this.Server.close(() => resolve()));
        this.Server.closeIdleConnections();
        for(const Socket of this.Upgraded){
            Socket.destroy();
        }
        const Deadline = setTimeout(() => this.Server.closeAllConnections(), GraceMs);
        await Closed;
        clearTimeout(Deadline);
        this.Agent.destroy();
        if(this.Feed !== undefined){
            await this.Feed.Idle();
            this.Feed.Close();
        }
    }

    private OnConnection(Socket: net.Socket){
        Socket.on("error", () => undefined);
        if(this.Closing){
            Socket.destroy();
            return;
        }

        const Ip = PeerAddress(Socket.remoteAddress);
        const Key = RateLimitKey(Ip);
        const Count = this.PerIp.get(Key) ?? 0;
        if(Count >= this.Config.limits.maxConnectionsPerIp){
            if(this.Throttle.Allow(`perip:${Key}`) !== undefined){
                logger.warn("connection refused: too many open connections from one address", { ip: Ip, open: Count });
            }
            Socket.destroy();
            return;
        }
        const Rate = this.ConnectLimiter.Take(Key);
        if(!Rate.ok){
            if(this.Throttle.Allow(`connrate:${Key}`) !== undefined){
                logger.warn("connection refused: too many new connections from one address", { ip: Ip });
            }
            Socket.destroy();
            return;
        }

        this.PerIp.set(Key, Count + 1);
        Socket.once("close", () => {
            const Left = (this.PerIp.get(Key) ?? 1) - 1;
            if(Left <= 0){
                this.PerIp.delete(Key);
            }
            else{
                this.PerIp.set(Key, Left);
            }
        });
    }

    // Malformed requests, oversized headers and the header/body timeouts (slow clients) end here.
    private OnClientError(Error: NodeJS.ErrnoException, Socket: net.Socket){
        if(Error.code === "ECONNRESET" || Error.code === "EPIPE"){
            Socket.destroy();
            return;
        }
        const Status = Error.code === "HPE_HEADER_OVERFLOW" ? 431
            : Error.code === "ERR_HTTP_REQUEST_TIMEOUT" ? 408
            : 400;
        if(this.Throttle.Allow(`clienterror:${PeerAddress(Socket.remoteAddress)}`) !== undefined){
            logger.info("request", { ip: PeerAddress(Socket.remoteAddress), route: "gateway", status: Status, reason: Error.code ?? "client_error" });
        }
        const Message = (Socket as any)._httpMessage;
        if(Socket.writable && (Message === undefined || Message === null || !Message._headerSent)){
            const Body = JSON.stringify({ error: Status === 408 ? "request_timeout" : Status === 431 ? "headers_too_large" : "bad_request" });
            Socket.end(`HTTP/1.1 ${Status} ${http.STATUS_CODES[Status]}\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(Body)}\r\nConnection: close\r\n\r\n${Body}`);
        }
        setTimeout(() => Socket.destroy(), 1000).unref();
    }

    private Log(Entry: AccessEntry, Started: number, Extra: Record<string, unknown> = {}){
        logger.info("request", {
            ip: Entry.ip,
            method: Entry.method,
            target: Entry.target,
            route: Entry.route,
            status: Entry.status,
            bytesIn: Entry.bytesIn,
            bytesOut: Entry.bytesOut,
            ms: Math.round(performance.now() - Started),
            ...(Entry.reason === undefined ? {} : { reason: Entry.reason }),
            ...(Entry.ua === undefined ? {} : { ua: Entry.ua }),
            ...Extra,
        });
    }

    private Refuse(Res: http.ServerResponse, Req: http.IncomingMessage, Entry: AccessEntry, Status: number, Error: string, Reason: string, Extra: http.OutgoingHttpHeaders = {}){
        Entry.route = "gateway";
        Entry.reason = Reason;
        const Body = JSON.stringify({ error: Error });
        // A refused request's body is never read: close the connection instead of draining it.
        const HasBody = Req.headers["transfer-encoding"] !== undefined || Number(Req.headers["content-length"] ?? 0) > 0;
        Res.writeHead(Status, {
            "content-type": "application/json",
            "content-length": Buffer.byteLength(Body),
            "cache-control": "no-store",
            ...(HasBody || this.Closing ? { connection: "close" } : {}),
            ...Extra,
        });
        Res.end(Body);
        if(HasBody){
            Res.once("finish", () => {
                Req.socket.end();
                setTimeout(() => Req.socket.destroy(), 1000).unref();
            });
        }
    }

    private Upstream(Name: Upstream): Endpoint {
        return Name === "backend" ? this.Config.backend : Name === "content" ? this.Config.content : this.Config.ws;
    }

    private OnRequest(Req: http.IncomingMessage, Res: http.ServerResponse, ExpectsContinue: boolean){
        const Started = performance.now();
        const Ip = PeerAddress(Req.socket.remoteAddress);
        const Entry: AccessEntry = {
            ip: Ip,
            method: Req.method ?? "",
            target: RedactTarget(Req.url ?? ""),
            route: "gateway",
            status: 0,
            bytesIn: 0,
            bytesOut: 0,
            ua: RedactUserAgent(Req.headers["user-agent"]),
        };
        Res.once("close", () => {
            Entry.status = Res.headersSent ? Res.statusCode : 0;
            this.Log(Entry, Started, Res.writableFinished ? {} : { aborted: true });
        });

        const Choice = Classify(Req.method, Req.url, Req.headers, false);

        const Rate = this.Limiters[Choice.limitClass].Take(RateLimitKey(Ip));
        if(!Rate.ok){
            this.Refuse(Res, Req, Entry, 429, "rate_limited", `rate_limited:${Choice.limitClass}`, { "retry-after": String(Rate.retryAfterSeconds) });
            return;
        }

        if(Choice.action === "reject"){
            this.Refuse(Res, Req, Entry, Choice.status, Choice.error, Choice.reason);
            return;
        }

        const Length = Req.headers["content-length"];
        if(Length !== undefined && Number(Length) > this.Config.limits.maxBodyBytes){
            this.Refuse(Res, Req, Entry, 413, "body_too_large", "body_too_large");
            return;
        }

        if(ExpectsContinue){
            Res.writeContinue();
        }

        this.Proxy(Req, Res, Choice, Ip, Entry);
    }

    private Proxy(Req: http.IncomingMessage, Res: http.ServerResponse, Choice: Extract<Decision, { action: "proxy" }>, Ip: string, Entry: AccessEntry){
        Entry.route = Choice.upstream;
        const Target = this.Upstream(Choice.upstream);
        let Finished = false;

        const UpReq = http.request({
            host: Target.host,
            port: Target.port,
            method: Req.method,
            path: Req.url,
            headers: ForwardHeaders(Req.headers, Ip, this.Config.secret, false, this.Config.tls ? "https" : "http"),
            agent: this.Agent,
        });

        UpReq.setTimeout(this.Config.timeouts.upstreamIdleMs, () => UpReq.destroy(Object.assign(new Error("upstream timed out"), { code: "UPSTREAM_TIMEOUT" })));

        UpReq.on("response", (UpRes) => {
            const Status = UpRes.statusCode ?? 502;
            if(Choice.feed !== undefined && this.Feed !== undefined && Status >= 200 && Status < 300
                && (Choice.feed === "token" || HasBearer(Req.headers))){
                this.Feed.Report(Ip);
            }
            if(Res.headersSent || Res.destroyed){
                UpRes.resume();
                return;
            }
            Res.writeHead(Status, UpRes.statusMessage, ResponseHeaders(UpRes.headers, this.Closing));
            UpRes.on("data", (Chunk: Buffer) => {
                Entry.bytesOut += Chunk.length;
            });
            UpRes.on("error", () => Res.destroy());
            UpRes.on("aborted", () => Res.destroy());
            UpRes.pipe(Res);
        });

        UpReq.on("error", (Error: NodeJS.ErrnoException) => {
            if(Finished){
                return;
            }
            Finished = true;
            if(!Res.headersSent && !Res.destroyed){
                const TimedOut = Error.code === "UPSTREAM_TIMEOUT";
                this.Refuse(Res, Req, Entry, TimedOut ? 504 : 502, TimedOut ? "upstream_timeout" : "bad_gateway", TimedOut ? "upstream_timeout" : `upstream_${Error.code ?? "error"}`);
                Entry.route = Choice.upstream;
            }
            else{
                Res.destroy();
            }
        });

        // The client went away (or the answer is complete): nothing more to do upstream.
        Res.once("close", () => {
            if(!Res.writableFinished){
                Finished = true;
                UpReq.destroy();
            }
        });

        // Request body: streamed with backpressure, counted, cut at the cap. A Content-Length
        // over the cap was refused before this point; this catches chunked bodies.
        const Cap = this.Config.limits.maxBodyBytes;
        Req.on("data", (Chunk: Buffer) => {
            if(Finished){
                return;
            }
            Entry.bytesIn += Chunk.length;
            if(Entry.bytesIn > Cap){
                Finished = true;
                UpReq.destroy();
                if(!Res.headersSent){
                    this.Refuse(Res, Req, Entry, 413, "body_too_large", "body_too_large");
                }
                else{
                    Res.destroy();
                }
                return;
            }
            if(!UpReq.write(Chunk)){
                Req.pause();
                UpReq.once("drain", () => Req.resume());
            }
        });
        Req.on("end", () => {
            if(!Finished){
                UpReq.end();
            }
        });
        Req.on("error", () => {
            Finished = true;
            UpReq.destroy();
        });
    }

    private WriteRaw(Socket: net.Socket, Status: number, Error: string, Extra: Record<string, string> = {}){
        const Body = JSON.stringify({ error: Error });
        let Head = `HTTP/1.1 ${Status} ${http.STATUS_CODES[Status] ?? ""}\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(Body)}\r\nCache-Control: no-store\r\nConnection: close\r\n`;
        for(const [Name, Value] of Object.entries(Extra)){
            Head += `${Name}: ${Value}\r\n`;
        }
        if(Socket.writable){
            Socket.end(Head + "\r\n" + Body);
        }
        setTimeout(() => Socket.destroy(), 1000).unref();
    }

    // WebSocket upgrades (the chat service later): the same checks as any request, then the
    // handshake goes to the ws upstream and the two sockets are joined byte for byte.
    private OnUpgrade(Req: http.IncomingMessage, Socket: net.Socket, Head: Buffer){
        const Started = performance.now();
        Socket.on("error", () => undefined);
        const Ip = PeerAddress(Req.socket.remoteAddress);
        const Entry: AccessEntry = {
            ip: Ip,
            method: Req.method ?? "",
            target: RedactTarget(Req.url ?? ""),
            route: "gateway",
            status: 0,
            bytesIn: 0,
            bytesOut: 0,
            ua: RedactUserAgent(Req.headers["user-agent"]),
        };
        const Refuse = (Status: number, Error: string, Reason: string, Extra: Record<string, string> = {}) => {
            Entry.status = Status;
            Entry.reason = Reason;
            this.WriteRaw(Socket, Status, Error, Extra);
            this.Log(Entry, Started);
        };

        if(this.Closing){
            Refuse(503, "shutting_down", "shutting_down");
            return;
        }

        const Choice = Classify(Req.method, Req.url, Req.headers, true);
        const Rate = this.Limiters[Choice.limitClass].Take(RateLimitKey(Ip));
        if(!Rate.ok){
            Refuse(429, "rate_limited", `rate_limited:${Choice.limitClass}`, { "Retry-After": String(Rate.retryAfterSeconds) });
            return;
        }
        if(Choice.action === "reject"){
            Refuse(Choice.status, Choice.error, Choice.reason);
            return;
        }

        const Target = this.Upstream(Choice.upstream);
        Entry.route = Choice.upstream;
        const UpReq = http.request({
            host: Target.host,
            port: Target.port,
            method: "GET",
            path: Req.url,
            headers: ForwardHeaders(Req.headers, Ip, this.Config.secret, true, this.Config.tls ? "https" : "http"),
            agent: false,
        });
        UpReq.setTimeout(this.Config.timeouts.headersMs, () => UpReq.destroy(Object.assign(new Error("upstream timed out"), { code: "UPSTREAM_TIMEOUT" })));

        let Settled = false;
        Socket.once("close", () => {
            if(!Settled){
                Settled = true;
                UpReq.destroy();
            }
        });

        UpReq.on("upgrade", (UpRes: http.IncomingMessage, UpSocket: net.Socket, UpHead: Buffer) => {
            UpReq.setTimeout(0);
            UpSocket.on("error", () => undefined);
            if(Settled || Socket.destroyed){
                UpSocket.destroy();
                return;
            }
            Settled = true;

            let Reply = `HTTP/1.1 101 ${UpRes.statusMessage || "Switching Protocols"}\r\n`;
            for(let Index = 0; Index + 1 < UpRes.rawHeaders.length; Index += 2){
                const Name = UpRes.rawHeaders[Index];
                if(RESPONSE_DROP.has(Name.toLowerCase())){
                    continue;
                }
                Reply += `${Name}: ${UpRes.rawHeaders[Index + 1]}\r\n`;
            }
            Socket.write(Reply + "\r\n");
            if(UpHead.length > 0){
                Socket.write(UpHead);
            }
            if(Head.length > 0){
                UpSocket.write(Head);
            }

            Entry.status = 101;
            this.Log(Entry, Started);

            this.Upgraded.add(Socket);
            const Idle = this.Config.timeouts.wsIdleMs;
            Socket.setTimeout(Idle, () => Socket.destroy());
            UpSocket.setTimeout(Idle, () => UpSocket.destroy());
            Socket.on("data", (Chunk: Buffer) => {
                Entry.bytesIn += Chunk.length;
            });
            UpSocket.on("data", (Chunk: Buffer) => {
                Entry.bytesOut += Chunk.length;
            });
            let Ended = false;
            const End = () => {
                if(Ended){
                    return;
                }
                Ended = true;
                this.Upgraded.delete(Socket);
                Socket.destroy();
                UpSocket.destroy();
                logger.info("websocket closed", { ip: Ip, target: Entry.target, bytesIn: Entry.bytesIn, bytesOut: Entry.bytesOut, ms: Math.round(performance.now() - Started) });
            };
            Socket.once("close", End);
            UpSocket.once("close", End);
            Socket.pipe(UpSocket);
            UpSocket.pipe(Socket);
        });

        // The upstream answered without switching protocols: pass that answer on and hang up.
        UpReq.on("response", (UpRes: http.IncomingMessage) => {
            if(Settled){
                UpRes.resume();
                return;
            }
            Settled = true;
            const Chunks: Buffer[] = [];
            let Size = 0;
            UpRes.on("data", (Chunk: Buffer) => {
                if(Size < 64 * 1024){
                    Chunks.push(Chunk);
                    Size += Chunk.length;
                }
            });
            UpRes.on("end", () => {
                const Body = Buffer.concat(Chunks);
                const Headers = ResponseHeaders(UpRes.headers, true);
                delete Headers["content-length"];
                let Reply = `HTTP/1.1 ${UpRes.statusCode} ${UpRes.statusMessage ?? ""}\r\n`;
                for(const [Name, Value] of Object.entries(Headers)){
                    for(const One of Array.isArray(Value) ? Value : [Value]){
                        Reply += `${Name}: ${One}\r\n`;
                    }
                }
                Reply += `content-length: ${Body.length}\r\n\r\n`;
                if(Socket.writable){
                    Socket.end(Buffer.concat([Buffer.from(Reply), Body]));
                }
                setTimeout(() => Socket.destroy(), 1000).unref();
                Entry.status = UpRes.statusCode ?? 0;
                Entry.bytesOut = Body.length;
                this.Log(Entry, Started);
            });
        });

        UpReq.on("error", (Error: NodeJS.ErrnoException) => {
            if(Settled){
                return;
            }
            Settled = true;
            const TimedOut = Error.code === "UPSTREAM_TIMEOUT";
            Refuse(TimedOut ? 504 : 502, TimedOut ? "upstream_timeout" : "bad_gateway", TimedOut ? "upstream_timeout" : `upstream_${Error.code ?? "error"}`);
        });

        UpReq.end();
    }
}
