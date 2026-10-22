import fs from "node:fs";
import http from "node:http";
import { pipeline } from "node:stream";
import { AuthCache } from "./auth";
import { Branding, BRANDING_PREFIX, IsSafeImageName } from "./branding";
import { StreamLimiter } from "./limits";
import { logger } from "./log";
import { News } from "./news";
import { RUNTIME_FILE_NAMES, RuntimeStore } from "./runtime";

// Routes (all GET or HEAD), behind the gateway:
//   /content/v1/runtime/manifest      the game runtime the host published: version and files with
//                                     their sizes and SHA-256 (no key needed: the launcher shows it
//                                     to the player before asking them to accept it)
//   /content/v1/runtime/files/<name>  MysticParadox.dll or winmm.dll; needs x-reforged-user-api-key
//   /content/v1/branding              the host's art pack: background image URLs and an accent colour
//   /content/v1/branding/<file>       one of those images
//   /content/v1/news                  the host's news items
// Anything else is 404; there is no directory listing anywhere. Modelled on Dauntless Revived's
// content server, which serves the 1.4.4 game files the same way.

export const RUNTIME_MANIFEST_PATH = "/content/v1/runtime/manifest";
export const RUNTIME_FILES_PREFIX = "/content/v1/runtime/files/";

export type ContentDeps = {
    runtime: RuntimeStore;
    auth: AuthCache;
    limiter: StreamLimiter;
    branding: Branding;
    news: News;
};

const MAX_BRANDING_BYTES = 25 * 1024 * 1024;

type RequestLog = {
    route: string;
    path?: string;
    reason?: string;
    account?: string;
    user?: string;
    bytes?: number;
    expected?: number;
};

function Base(res: http.ServerResponse){
    res.setHeader("X-Content-Type-Options", "nosniff");
}

function SendJson(req: http.IncomingMessage, res: http.ServerResponse, Status: number, Body: unknown, Headers: Record<string, string> = {}){
    const Text = typeof Body === "string" ? Body : JSON.stringify(Body);
    res.statusCode = Status;
    res.setHeader("Content-Type", "application/json; charset=utf-8");
    res.setHeader("Content-Length", Buffer.byteLength(Text));
    res.setHeader("Cache-Control", "no-cache");
    for(const [Name, Value] of Object.entries(Headers)){
        res.setHeader(Name, Value);
    }
    res.end(req.method === "HEAD" ? undefined : Text);
}

function SendError(req: http.IncomingMessage, res: http.ServerResponse, Status: number, Code: string, Headers: Record<string, string> = {}){
    SendJson(req, res, Status, { error: Code }, Headers);
}

function StripQuery(Target: string): string {
    const At = Target.indexOf("?");
    return At === -1 ? Target : Target.slice(0, At);
}

// If-None-Match: "*" or a list of entity tags, weak ones included (RFC 9110 13.1.2).
export function IfNoneMatchHits(Header: string | string[] | undefined, ETag: string): boolean {
    if(typeof Header !== "string"){
        return false;
    }
    if(Header.trim() === "*"){
        return true;
    }
    return Header.split(",").some((Tag) => Tag.trim().replace(/^W\//, "") === ETag);
}

export function CreateContentHandler(Deps: ContentDeps): http.RequestListener {
    return (req, res) => {
        const Started = Date.now();
        const Target = req.url ?? "/";
        const PathOnly = StripQuery(Target);
        const Info: RequestLog = { route: "other" };
        let ClientGone = false;

        Base(res);

        res.on("close", () => {
            ClientGone = true;
            const Fields: Record<string, unknown> = {
                method: req.method,
                route: Info.route,
                path: Info.path ?? PathOnly.slice(0, 200),
                status: res.statusCode,
                ms: Date.now() - Started,
                ip: req.headers["x-forwarded-for"] ?? req.socket.remoteAddress,
            };
            if(Info.reason !== undefined) Fields.reason = Info.reason;
            if(Info.account !== undefined) Fields.account = Info.account;
            if(Info.user !== undefined) Fields.user = Info.user;
            if(Info.bytes !== undefined){
                Fields.bytes = Info.bytes;
                Fields.expected = Info.expected;
                Fields.complete = Info.bytes === Info.expected && res.writableFinished;
            }
            const Level = res.statusCode >= 500 || res.statusCode === 400 || res.statusCode === 401 ? "warn" : "info";
            logger[Level](Info.route === "runtime-file" && req.method === "GET" && Info.bytes !== undefined ? "download" : "request", Fields);
        });

        const Method = req.method ?? "GET";
        const IsKnownRoute =
            PathOnly === RUNTIME_MANIFEST_PATH || PathOnly.startsWith(RUNTIME_FILES_PREFIX) ||
            PathOnly === "/content/v1/branding" || PathOnly.startsWith(BRANDING_PREFIX) ||
            PathOnly === "/content/v1/news";

        if(!IsKnownRoute){
            SendError(req, res, 404, "not_found");
            return;
        }

        if(Method !== "GET" && Method !== "HEAD"){
            SendError(req, res, 405, "method_not_allowed", { "Allow": "GET, HEAD" });
            return;
        }

        if(PathOnly === RUNTIME_MANIFEST_PATH){
            Info.route = "runtime-manifest";
            const Status = Deps.runtime.get();
            if(Status.status !== "ok"){
                Info.reason = Status.reason;
                // 404: nothing published yet (the launcher shows "waiting for the host").
                // 503: something is published but does not check out; the host has to fix it.
                SendError(req, res, Status.status === "none" ? 404 : 503, Status.status === "none" ? "no_runtime" : "runtime_invalid");
                return;
            }
            if(IfNoneMatchHits(req.headers["if-none-match"], Status.etag)){
                res.statusCode = 304;
                res.setHeader("ETag", Status.etag);
                res.end();
                return;
            }
            SendJson(req, res, 200, Status.body, { "ETag": Status.etag });
            return;
        }

        if(PathOnly === "/content/v1/branding"){
            Info.route = "branding";
            SendJson(req, res, 200, Deps.branding.body());
            return;
        }

        if(PathOnly === "/content/v1/news"){
            Info.route = "news";
            SendJson(req, res, 200, Deps.news.body());
            return;
        }

        if(PathOnly.startsWith(BRANDING_PREFIX)){
            Info.route = "branding-image";
            ServeBrandingImage(Deps, req, res, PathOnly.slice(BRANDING_PREFIX.length), Info);
            return;
        }

        Info.route = "runtime-file";
        ServeRuntimeFile(Deps, req, res, PathOnly.slice(RUNTIME_FILES_PREFIX.length), Info, () => ClientGone).catch((error: unknown) => {
            logger.error("runtime file request failed", { error: error instanceof Error ? error.message : String(error) });
            if(!res.headersSent){
                SendError(req, res, 500, "internal_error");
            }
            else{
                res.destroy();
            }
        });
    };
}

function ServeBrandingImage(Deps: ContentDeps, req: http.IncomingMessage, res: http.ServerResponse, Name: string, Info: RequestLog){
    if(!IsSafeImageName(Name)){
        SendError(req, res, 404, "not_found");
        return;
    }

    const Found = Deps.branding.image(Name);
    if(Found === undefined){
        SendError(req, res, 404, "not_found");
        return;
    }

    Info.path = Name;

    fs.promises.open(Found.fullPath, "r").then(async (Handle) => {
        let Stat: fs.Stats;
        try{
            Stat = await Handle.stat();
        }
        catch{
            await Handle.close().catch(() => undefined);
            SendError(req, res, 404, "not_found");
            return;
        }

        if(!Stat.isFile() || Stat.size === 0 || Stat.size > MAX_BRANDING_BYTES){
            await Handle.close().catch(() => undefined);
            SendError(req, res, 404, "not_found");
            return;
        }

        const ETag = `W/"${Stat.size.toString(16)}-${Math.floor(Stat.mtimeMs).toString(16)}"`;
        res.setHeader("ETag", ETag);
        res.setHeader("Cache-Control", "public, max-age=300");

        if(IfNoneMatchHits(req.headers["if-none-match"], ETag.replace(/^W\//, ""))){
            await Handle.close().catch(() => undefined);
            res.statusCode = 304;
            res.end();
            return;
        }

        res.statusCode = 200;
        res.setHeader("Content-Type", Found.image.contentType);
        res.setHeader("Content-Length", Stat.size);

        if(req.method === "HEAD"){
            await Handle.close().catch(() => undefined);
            res.end();
            return;
        }

        const Stream = Handle.createReadStream({ start: 0, end: Stat.size - 1, highWaterMark: 256 * 1024 });
        pipeline(Stream, res, () => undefined);
    }, () => {
        SendError(req, res, 404, "not_found");
    }).catch((error: unknown) => {
        logger.warn("branding image failed", { file: Name, error: error instanceof Error ? error.message : String(error) });
        res.destroy();
    });
}

async function ServeRuntimeFile(Deps: ContentDeps, req: http.IncomingMessage, res: http.ServerResponse, Name: string, Info: RequestLog, IsClientGone: () => boolean){
    // Exactly one of the two names, letter case included: no decoding, no folders, no lookalikes.
    if(!(RUNTIME_FILE_NAMES as readonly string[]).includes(Name)){
        Info.reason = "not_a_runtime_file";
        SendError(req, res, 404, "not_found");
        return;
    }
    Info.path = Name;

    const Key = req.headers["x-reforged-user-api-key"];
    if(typeof Key !== "string" || Key.length === 0){
        Info.reason = "no_key";
        SendError(req, res, 401, "unauthorized");
        return;
    }

    const Auth = await Deps.auth.check(Key);
    if(Auth.status === "unavailable"){
        Info.reason = Auth.reason;
        SendError(req, res, 503, "auth_unavailable", { "Retry-After": "5" });
        return;
    }
    if(Auth.status === "invalid"){
        Info.reason = "bad_key";
        SendError(req, res, 401, "unauthorized");
        return;
    }
    Info.account = Auth.account.UserId;
    Info.user = Auth.account.Username;

    const Found = Deps.runtime.file(Name);
    if(Found === undefined){
        Info.reason = "no_runtime";
        SendError(req, res, 404, "not_found");
        return;
    }

    const ETag = `"${Found.file.sha256}"`;
    if(IfNoneMatchHits(req.headers["if-none-match"], ETag)){
        res.statusCode = 304;
        res.setHeader("ETag", ETag);
        res.end();
        return;
    }

    if(req.method === "HEAD"){
        res.statusCode = 200;
        res.setHeader("ETag", ETag);
        res.setHeader("Content-Type", "application/octet-stream");
        res.setHeader("Content-Length", Found.file.size);
        res.end();
        return;
    }

    const Slot = Deps.limiter.tryAcquire(Auth.account.UserId);
    if(!Slot.ok){
        Info.reason = Slot.reason;
        if(Slot.reason === "account_limit"){
            SendError(req, res, 429, "too_many_streams", { "Retry-After": "2" });
        }
        else{
            SendError(req, res, 503, "server_busy", { "Retry-After": "10" });
        }
        return;
    }
    if(IsClientGone()){
        Slot.release();
        return;
    }
    res.on("close", Slot.release);

    let Handle: fs.promises.FileHandle;
    try{
        Handle = await fs.promises.open(Found.fullPath, "r");
    }
    catch{
        Deps.runtime.invalidate();
        Info.reason = "open_failed";
        SendError(req, res, 503, "file_unavailable");
        return;
    }
    let Stat: fs.Stats;
    try{
        Stat = await Handle.stat();
    }
    catch{
        await Handle.close().catch(() => undefined);
        SendError(req, res, 503, "file_unavailable");
        return;
    }
    // Replaced since the last check (a new publish in progress): never send a size the list does not name.
    if(!Stat.isFile() || Stat.size !== Found.file.size){
        await Handle.close().catch(() => undefined);
        Deps.runtime.invalidate();
        Info.reason = "changed_on_disk";
        SendError(req, res, 503, "file_unavailable", { "Retry-After": "10" });
        return;
    }

    res.statusCode = 200;
    res.setHeader("ETag", ETag);
    res.setHeader("Content-Type", "application/octet-stream");
    res.setHeader("Content-Length", Found.file.size);
    res.setHeader("Cache-Control", "private, no-transform");
    Info.bytes = 0;
    Info.expected = Found.file.size;

    const Stream = Handle.createReadStream({ start: 0, end: Found.file.size - 1, highWaterMark: 256 * 1024 });
    Stream.on("data", (Chunk: Buffer | string) => {
        Info.bytes = (Info.bytes ?? 0) + Chunk.length;
    });
    pipeline(Stream, res, (error) => {
        if(error && (error as NodeJS.ErrnoException).code !== "ERR_STREAM_PREMATURE_CLOSE"){
            logger.warn("runtime download stream error", { file: Name, error: error.message });
        }
    });
}
