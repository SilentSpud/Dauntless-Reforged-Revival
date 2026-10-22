import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";
import { migrateOldWorkspace, missingParadox } from "../src/paradox";
import { findRepoRoot, paths } from "../src/paths";
import { tempDir, write } from "./helpers";

const repo = path.resolve(__dirname, "..", "..", "..", "..", "..");

describe("Mystic Paradox in the repository", () => {
    it("this checkout has everything the host kit builds from", () => {
        assert.equal(findRepoRoot(__dirname), repo);
        assert.deepEqual(missingParadox(paths(repo)), []);
        assert.deepEqual(missingParadox(paths(tempDir("empty"))), ["paradox/backend", "paradox/director", "native/paradox-runtime", "native/runtime-loader", "native/catalog-exporter"]);
    });

    it("nothing is downloaded from Paradox's repository any more", () => {
        assert.ok(!fs.existsSync(path.join(repo, "upstream")), "upstream/ is gone");
        const kit = JSON.parse(fs.readFileSync(path.join(repo, "legacy", "ReforgedHost", "package.json"), "utf8")) as { scripts: Record<string, string> };
        assert.equal(kit.scripts.upstream, undefined);
        const src = path.join(repo, "legacy", "ReforgedHost", "src");
        for (const f of fs.readdirSync(src)) assert.ok(!fs.readFileSync(path.join(src, f), "utf8").includes("github.com/pranav158"), f);
    });

    it("the host's own files move over from the old .workspace/Mystic-Paradox once", () => {
        const root = tempDir("migrate");
        const p = paths(root);
        const old = p.oldUpstream;
        assert.deepEqual(migrateOldWorkspace(p), [], "no old folder: nothing to do");

        write(path.join(old, "ParadoxRuntime", "SDK.hpp"), "sdk\n");
        write(path.join(old, "ParadoxRuntime", "SDK", "Basic_functions.cpp"), "fn\n");
        write(path.join(old, "ParadoxRuntime", "x64", "Release", "MysticParadox.dll"), "dll");
        write(path.join(old, "tools", "RuntimeLoader", "target", "release", "winmm.dll"), "loader");
        write(path.join(old, "Items_Analysis", "catalog_1_14_7.jsonl"), "{}\n");
        write(path.join(old, "ParadoxBackend", "game-data", "progression.json"), "{\"mine\":1}\n");
        write(path.join(old, "ParadoxBackend", "game-data", "progression.example.json"), "{}\n");
        write(path.join(old, "ParadoxBackend", "game-data", "raw", "ladyluck_store.source.json"), "[]\n");
        write(path.join(old, "ParadoxDirector", "game-data", "hunts.json"), "{\"old\":1}\n");
        // Already there (from the preset, say): kept.
        write(path.join(p.directorGameData, "hunts.json"), "{\"new\":1}\n");
        fs.mkdirSync(p.rawGameData, { recursive: true });

        const copied = migrateOldWorkspace(p);
        assert.deepEqual(copied.sort(), [
            "native/paradox-runtime/SDK",
            "native/paradox-runtime/SDK.hpp",
            "native/paradox-runtime/x64/Release/MysticParadox.dll",
            "native/runtime-loader/target/release/winmm.dll",
            "paradox/Items_Analysis",
            "paradox/backend/game-data/progression.json",
            "paradox/backend/game-data/raw",
        ]);
        assert.equal(fs.readFileSync(path.join(p.sdkDir, "Basic_functions.cpp"), "utf8"), "fn\n");
        assert.equal(fs.readFileSync(path.join(p.backendGameData, "progression.json"), "utf8"), "{\"mine\":1}\n");
        assert.ok(!fs.existsSync(path.join(p.backendGameData, "progression.example.json")), "the committed examples are not copied");
        assert.equal(fs.readFileSync(path.join(p.directorGameData, "hunts.json"), "utf8"), "{\"new\":1}\n");
        assert.ok(fs.existsSync(path.join(old, "ParadoxRuntime", "SDK.hpp")), "the old folder is left alone");
        assert.deepEqual(migrateOldWorkspace(p), [], "the second time there is nothing left to copy");
    });
});
