import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, it, after } from "node:test";
import { ParseRuntimeIndex, RuntimeStore } from "../src/runtime";
import { SetLogSink } from "../src/log";
import { Publish } from "./helpers";

SetLogSink(() => undefined);

const Dirs: string[] = [];
after(() => {
    for(const Dir of Dirs) fs.rmSync(Dir, { recursive: true, force: true });
});

function TempDir(): string {
    const Dir = fs.mkdtempSync(path.join(os.tmpdir(), "rr-content-runtime-"));
    Dirs.push(Dir);
    return Dir;
}

describe("ParseRuntimeIndex", () => {
    const Good = { version: "1.0", build: { changelist: 647472 }, files: [{ name: "MysticParadox.dll", size: 10, sha256: "a".repeat(64) }, { name: "winmm.dll", size: 5, sha256: "b".repeat(64) }] };

    it("accepts the two runtime files, sorted by name", () => {
        const Index = ParseRuntimeIndex({ ...Good, files: [...Good.files].reverse() });
        assert.deepEqual(Index.files.map((F) => F.name), ["MysticParadox.dll", "winmm.dll"]);
        assert.deepEqual(Index.build, { name: "rel-1.14.7-Archon", changelist: 647472 });
    });

    it("refuses anything else: other names, duplicates, missing files, bad hashes, sizes and versions", () => {
        const Bad: unknown[] = [
            null, [], { ...Good, version: "" }, { ...Good, version: "a b" }, { ...Good, files: "x" },
            { ...Good, files: [Good.files[0]] },
            { ...Good, files: [...Good.files, Good.files[0]] },
            { ...Good, files: [{ ...Good.files[0], name: "..\\MysticParadox.dll" }, Good.files[1]] },
            { ...Good, files: [{ ...Good.files[0], name: "dxgi.dll" }, Good.files[1]] },
            { ...Good, files: [{ ...Good.files[0], sha256: "A".repeat(64) }, Good.files[1]] },
            { ...Good, files: [{ ...Good.files[0], size: 0 }, Good.files[1]] },
            { ...Good, files: [{ ...Good.files[0], size: 1e9 }, Good.files[1]] },
            { ...Good, build: { changelist: -1 } },
        ];
        for(const Raw of Bad){
            assert.throws(() => ParseRuntimeIndex(Raw), Error, JSON.stringify(Raw));
        }
    });
});

describe("RuntimeStore", () => {
    it("offers nothing without a folder or before the host publishes", () => {
        assert.equal(new RuntimeStore(undefined).get().status, "none");
        assert.equal(new RuntimeStore(TempDir()).get().status, "none");
    });

    it("offers a published runtime only when every file matches, and picks up a new one without a restart", () => {
        let Clock = 0;
        const Dir = TempDir();
        Publish(Dir, 1, "v1");
        const Store = new RuntimeStore(Dir, () => Clock);
        const First = Store.get();
        assert.equal(First.status, "ok");
        assert.equal(First.status === "ok" && First.index.version, "v1");
        assert.ok(Store.file("winmm.dll"));
        assert.equal(Store.file("WINMM.DLL"), undefined);
        assert.equal(Store.file("runtime.json"), undefined);

        // A file that does not match its hash: nothing is offered.
        fs.writeFileSync(path.join(Dir, "winmm.dll"), Buffer.alloc(3001, 9));
        Clock += 11_000;
        const Broken = Store.get();
        assert.equal(Broken.status, "invalid");
        assert.match(Broken.status === "invalid" ? Broken.reason : "", /winmm\.dll/);
        assert.equal(Store.file("winmm.dll"), undefined);

        Publish(Dir, 2, "v2");
        Clock += 11_000;
        const Second = Store.get();
        assert.equal(Second.status === "ok" && Second.index.version, "v2");
    });
});
