import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { CreateContentHandler } from "../src/app";
import { AuthCache } from "../src/auth";
import { Branding } from "../src/branding";
import { StreamLimiter } from "../src/limits";
import { SetLogSink } from "../src/log";
import { News } from "../src/news";
import { RuntimeStore } from "../src/runtime";
import { Publish, Request } from "./helpers";

// Ports: content handler 62050.
const PORT = 62050;
const KEY = "RRK_" + "c".repeat(48);
const Lines: string[] = [];

describe("the content server", () => {
    let Server: http.Server;
    let Dir: string;
    let Files: { dll: Buffer; loader: Buffer };
    let Lookups = 0;

    before(async () => {
        SetLogSink((Line) => Lines.push(Line));
        Dir = fs.mkdtempSync(path.join(os.tmpdir(), "rr-content-app-"));
        Files = Publish(Dir);
        const News_ = path.join(Dir, "news.json");
        fs.writeFileSync(News_, JSON.stringify({ items: [{ date: "2026-10-14", title: "Hello", body: "First hunt on Friday." }] }));
        const Handler = CreateContentHandler({
            runtime: new RuntimeStore(Dir),
            auth: new AuthCache({
                lookup: async (Key) => {
                    Lookups++;
                    return Key === KEY ? { status: "ok", account: { UserId: "uid-1", Username: "Aurora", IsAdmin: false } } : { status: "invalid" };
                },
            }),
            limiter: new StreamLimiter(4, 48),
            branding: new Branding(undefined),
            news: new News(News_),
        });
        Server = http.createServer(Handler);
        await new Promise<void>((resolve) => Server.listen(PORT, "127.0.0.1", resolve));
    });

    after(async () => {
        Server.closeAllConnections();
        await new Promise<void>((resolve) => Server.close(() => resolve()));
        fs.rmSync(Dir, { recursive: true, force: true });
        SetLogSink(undefined);
    });

    it("serves the runtime manifest without a key, with an ETag", async () => {
        const Reply = await Request(PORT, "GET", "/content/v1/runtime/manifest");
        assert.equal(Reply.status, 200);
        const Body = JSON.parse(Reply.body.toString());
        assert.equal(Body.version, "20261014-1a2b3c4d");
        assert.deepEqual(Body.files.map((F: { name: string }) => F.name), ["MysticParadox.dll", "winmm.dll"]);
        assert.equal(Body.files[0].sha256, createHash("sha256").update(Files.dll).digest("hex"));
        const Again = await Request(PORT, "GET", "/content/v1/runtime/manifest", { "if-none-match": String(Reply.headers.etag) });
        assert.equal(Again.status, 304);
    });

    it("serves a runtime file only with a known account key", async () => {
        assert.equal((await Request(PORT, "GET", "/content/v1/runtime/files/MysticParadox.dll")).status, 401);
        assert.equal((await Request(PORT, "GET", "/content/v1/runtime/files/MysticParadox.dll", { "x-reforged-user-api-key": "RRK_" + "0".repeat(48) })).status, 401);
        const Reply = await Request(PORT, "GET", "/content/v1/runtime/files/MysticParadox.dll", { "x-reforged-user-api-key": KEY });
        assert.equal(Reply.status, 200);
        assert.ok(Reply.body.equals(Files.dll));
        assert.equal(Reply.headers["content-type"], "application/octet-stream");
        const Loader = await Request(PORT, "GET", "/content/v1/runtime/files/winmm.dll", { "x-reforged-user-api-key": KEY });
        assert.ok(Loader.body.equals(Files.loader));
        assert.ok(!Lines.some((Line) => Line.includes(KEY)), "the key is never logged");
    });

    it("serves nothing but the two runtime files: no traversal, no lookalikes, no listing", async () => {
        for(const Target of ["/content/v1/runtime/files/runtime.json", "/content/v1/runtime/files/../runtime.json", "/content/v1/runtime/files/%2e%2e/runtime.json", "/content/v1/runtime/files/winmm.DLL", "/content/v1/runtime/files/", "/content/v1/runtime/files/MysticParadox.dll/x", "/content/v1/runtime/", "/content/v1/files/x", "/", "/content"]){
            const Reply = await Request(PORT, "GET", Target, { "x-reforged-user-api-key": KEY });
            assert.equal(Reply.status, 404, Target);
        }
        assert.equal((await Request(PORT, "POST", "/content/v1/runtime/manifest")).status, 405);
    });

    it("serves the host's news and an empty art pack", async () => {
        const News_ = JSON.parse((await Request(PORT, "GET", "/content/v1/news")).body.toString());
        assert.equal(News_.items[0].title, "Hello");
        const Art = JSON.parse((await Request(PORT, "GET", "/content/v1/branding")).body.toString());
        assert.deepEqual(Art, { backgrounds: [], accent: null });
    });

    it("answers 404 no_runtime before the host publishes", async () => {
        const Empty = fs.mkdtempSync(path.join(os.tmpdir(), "rr-content-empty-"));
        const Handler = CreateContentHandler({ runtime: new RuntimeStore(Empty), auth: new AuthCache({ lookup: async () => ({ status: "invalid" }) }), limiter: new StreamLimiter(4, 48), branding: new Branding(undefined), news: new News(undefined) });
        const Other = http.createServer(Handler);
        await new Promise<void>((resolve) => Other.listen(PORT + 1, "127.0.0.1", resolve));
        try{
            const Reply = await Request(PORT + 1, "GET", "/content/v1/runtime/manifest");
            assert.equal(Reply.status, 404);
            assert.deepEqual(JSON.parse(Reply.body.toString()), { error: "no_runtime" });
        }
        finally{
            Other.closeAllConnections();
            await new Promise<void>((resolve) => Other.close(() => resolve()));
            fs.rmSync(Empty, { recursive: true, force: true });
        }
        assert.ok(Lookups >= 1);
    });
});
