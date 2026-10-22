import { test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { AddressInfo } from "node:net";
import { AuthCache, HashKey, IsPlausibleKey, LookupResult, BackendLookup } from "../src/auth";

const GOOD = "RRK_" + "ab".repeat(24);
const ACCOUNT = { UserId: "UID-1", Username: "tester", IsAdmin: false };

function Counting(Answer: (Key: string) => LookupResult | Promise<LookupResult>){
    const Calls: string[] = [];
    return {
        calls: Calls,
        lookup: async (Key: string) => {
            Calls.push(HashKey(Key));
            return Answer(Key);
        },
    };
}

test("a good key is looked up once and then served from the cache", async () => {
    const L = Counting((Key) => Key === GOOD ? { status: "ok", account: ACCOUNT } : { status: "invalid" });
    const Cache = new AuthCache({ lookup: L.lookup });

    assert.deepEqual(await Cache.check(GOOD), { status: "ok", account: ACCOUNT });
    assert.deepEqual(await Cache.check(GOOD), { status: "ok", account: ACCOUNT });
    assert.equal(L.calls.length, 1);
});

test("the cache is keyed by the key's hash and never holds the key", async () => {
    const L = Counting(() => ({ status: "ok", account: ACCOUNT }));
    const Cache = new AuthCache({ lookup: L.lookup });
    await Cache.check(GOOD);

    assert.equal(Cache.hasHash(HashKey(GOOD)), true);
    const Internals = JSON.stringify([...(Cache as unknown as { Entries: Map<string, unknown> }).Entries.entries()]);
    assert.equal(Internals.includes(GOOD), false);
    assert.equal(HashKey(GOOD).length, 64);
    assert.notEqual(HashKey(GOOD), GOOD);
});

test("entries expire after the TTL", async () => {
    let Now = 1_000_000;
    const L = Counting(() => ({ status: "ok", account: ACCOUNT }));
    const Cache = new AuthCache({ lookup: L.lookup, ttlMs: 5 * 60 * 1000, now: () => Now });

    await Cache.check(GOOD);
    Now += 5 * 60 * 1000 - 1;
    await Cache.check(GOOD);
    assert.equal(L.calls.length, 1);
    Now += 2;
    await Cache.check(GOOD);
    assert.equal(L.calls.length, 2);
});

test("refused keys are cached briefly, backend outages not at all", async () => {
    let Now = 0;
    let Down = true;
    const L = Counting((Key) => Down ? { status: "unavailable", reason: "down" } : (Key === GOOD ? { status: "ok", account: ACCOUNT } : { status: "invalid" }));
    const Cache = new AuthCache({ lookup: L.lookup, negativeTtlMs: 30_000, now: () => Now });

    assert.equal((await Cache.check(GOOD)).status, "unavailable");
    Down = false;
    assert.equal((await Cache.check(GOOD)).status, "ok");
    assert.equal(L.calls.length, 2);

    assert.equal((await Cache.check("RRK_wrong")).status, "invalid");
    assert.equal((await Cache.check("RRK_wrong")).status, "invalid");
    assert.equal(L.calls.length, 3);
    Now += 30_001;
    assert.equal((await Cache.check("RRK_wrong")).status, "invalid");
    assert.equal(L.calls.length, 4);
});

test("implausible keys are refused without a lookup", async () => {
    const L = Counting(() => ({ status: "ok", account: ACCOUNT }));
    const Cache = new AuthCache({ lookup: L.lookup });

    for(const Bad of [undefined, "", "has space", "line\nbreak", "x".repeat(257), "ä", 42]){
        assert.equal((await Cache.check(Bad)).status, "invalid");
        assert.equal(IsPlausibleKey(Bad), false);
    }
    assert.equal(L.calls.length, 0);
});

test("concurrent checks of one key share a single lookup", async () => {
    let Release!: () => void;
    const Gate = new Promise<void>((resolve) => { Release = resolve; });
    const L = Counting(async () => {
        await Gate;
        return { status: "ok", account: ACCOUNT };
    });
    const Cache = new AuthCache({ lookup: L.lookup });

    const All = Promise.all([Cache.check(GOOD), Cache.check(GOOD), Cache.check(GOOD)]);
    Release();
    const Results = await All;
    assert.equal(L.calls.length, 1);
    for(const Result of Results){
        assert.equal(Result.status, "ok");
    }
});

test("a lookup that throws counts as unavailable", async () => {
    const Cache = new AuthCache({ lookup: async () => { throw new Error("boom"); } });
    const Result = await Cache.check(GOOD);
    assert.equal(Result.status, "unavailable");
    assert.equal(Cache.size, 0);
});

test("the cache stays bounded", async () => {
    const Cache = new AuthCache({ lookup: async () => ({ status: "invalid" }), maxEntries: 10 });
    for(let I = 0; I < 50; I++){
        await Cache.check(`RRK_${I}`);
    }
    assert.equal(Cache.size, 10);
    assert.equal(Cache.hasHash(HashKey("RRK_49")), true);
    assert.equal(Cache.hasHash(HashKey("RRK_0")), false);
});

test("BackendLookup maps the backend's answers", async () => {
    let Mode: "ok" | "junk" | "500" = "ok";
    const Seen: (string | undefined)[] = [];
    const Server = http.createServer((req, res) => {
        Seen.push(req.url);
        if(req.url !== "/reforged/api/GetUserInfo"){
            res.statusCode = 404;
            res.end();
            return;
        }
        if(req.headers["x-reforged-user-api-key"] !== GOOD){
            res.statusCode = 401;
            res.end();
            return;
        }
        if(Mode === "500"){
            res.statusCode = 500;
            res.end();
            return;
        }
        res.setHeader("Content-Type", "application/json");
        res.end(Mode === "junk" ? "{nope" : JSON.stringify(ACCOUNT));
    });
    await new Promise<void>((resolve) => Server.listen(62011, "127.0.0.1", resolve));
    try{
        const Port = (Server.address() as AddressInfo).port;
        const Lookup = BackendLookup(`http://127.0.0.1:${Port}/`, 2000);

        assert.deepEqual(await Lookup(GOOD), { status: "ok", account: ACCOUNT });
        assert.deepEqual(await Lookup("RRK_nope"), { status: "invalid" });
        Mode = "junk";
        assert.equal((await Lookup(GOOD)).status, "unavailable");
        Mode = "500";
        assert.equal((await Lookup(GOOD)).status, "unavailable");
        assert.ok(Seen.every((Url) => Url === "/reforged/api/GetUserInfo"));

        const Nowhere = BackendLookup("http://127.0.0.1:62019", 1000);
        assert.equal((await Nowhere(GOOD)).status, "unavailable");
    }
    finally{
        await new Promise<void>((resolve) => Server.close(() => resolve()));
    }
});
