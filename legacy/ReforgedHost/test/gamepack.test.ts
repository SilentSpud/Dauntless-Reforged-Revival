import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { describe, it } from "node:test";
import { exportGameDataPack, importGameDataPack, PACK_FORMAT, readGameDataPack } from "../src/gamepack";
import { paths } from "../src/paths";
import { tempDir, write } from "./helpers";
import { importGameDataPack as launcherImport, workspacePaths } from "../../ReforgedLauncher/src/main/hostkit";

function host(name: string) {
    const p = paths(tempDir(name));
    // The examples upstream ships beside the real files.
    write(path.join(p.backendGameData, "ladyluck_store.example.json"), "[]\n");
    write(path.join(p.backendGameData, "progression_config.example.json"), "{\"example\":true}\n");
    return p;
}

describe("game data packs", () => {
    it("carry the generated files to another setup, leaving out the examples", () => {
        const from = host("pack-from");
        write(path.join(from.backendGameData, "progression_config.json"), "{\"paths\":1}\n");
        write(path.join(from.backendGameData, "ladyluck_store.json"), "[]\n"); // still the example
        write(path.join(from.directorGameData, "player_hunts_table.json"), "{\"hunts\":2}\n");
        write(path.join(from.rawGameData, "progression_config.source.json"), "{\"source\":3}\n");
        write(path.join(from.backendGameData, "unrelated.json"), "{}\n");
        const pack = path.join(tempDir("pack-file"), "p.json.gz");
        const r = exportGameDataPack(from, pack);
        assert.deepEqual(r.included.sort(), ["backend/progression_config.json", "backend/raw/progression_config.source.json", "director/player_hunts_table.json"]);
        assert.ok(r.left.includes("backend/ladyluck_store.json"), "the example is not packed");

        const to = host("pack-to");
        write(path.join(to.backendGameData, "progression_config.json"), "{\"old\":true}\n");
        const imported = importGameDataPack(to, pack);
        assert.equal(imported.written.length, 3);
        assert.equal(fs.readFileSync(path.join(to.backendGameData, "progression_config.json"), "utf8"), "{\"paths\":1}\n");
        assert.equal(fs.readFileSync(path.join(to.directorGameData, "player_hunts_table.json"), "utf8"), "{\"hunts\":2}\n");
        assert.equal(fs.readFileSync(path.join(to.rawGameData, "progression_config.source.json"), "utf8"), "{\"source\":3}\n");
        assert.equal(imported.backups.length, 1, "the replaced file is kept");
        assert.equal(fs.readFileSync(imported.backups[0], "utf8"), "{\"old\":true}\n");
        assert.equal(importGameDataPack(to, pack).unchanged.length, 3, "importing again changes nothing");
    });

    it("refuses a pack with an unknown path, a bad hash, another game build or no pack at all", () => {
        const p = host("pack-bad");
        const dir = tempDir("pack-bad-files");
        const make = (files: unknown[], game = "1.14.7 (647472)") => {
            const f = path.join(dir, `${Math.random()}.json.gz`);
            fs.writeFileSync(f, zlib.gzipSync(JSON.stringify({ format: PACK_FORMAT, version: 1, game, created: "", files })));
            return f;
        };
        const hash = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"; // sha256("{}")
        assert.equal(readGameDataPack(p, make([{ path: "backend/progression_config.json", sha256: hash, data: "{}" }])).files.length, 1);
        assert.throws(() => readGameDataPack(p, make([{ path: "../../evil.json", sha256: hash, data: "{}" }])), /unknown file/);
        assert.throws(() => readGameDataPack(p, make([{ path: "backend/progression_config.json", sha256: "00", data: "{}" }])), /damaged/);
        assert.throws(() => readGameDataPack(p, make([], "1.12.0 (392819)")), /1\.12\.0/);
        const notPack = path.join(dir, "x.json.gz");
        fs.writeFileSync(notPack, "hello");
        assert.throws(() => readGameDataPack(p, notPack), /not a game data pack/);
        assert.throws(() => exportGameDataPack(host("pack-empty"), path.join(dir, "e.json.gz")), /npm run gamedata first/);
    });

    it("are read the same way by the launcher's Host page", async () => {
        const from = host("pack-launcher-from");
        write(path.join(from.backendGameData, "progression_config.json"), "{\"paths\":1}\n");
        write(path.join(from.rawGameData, "ladyluck_store.source.json"), "[1]\n");
        const pack = path.join(tempDir("pack-launcher-file"), "p.json.gz");
        exportGameDataPack(from, pack);
        const root = tempDir("pack-launcher-to");
        assert.deepEqual(await launcherImport(workspacePaths(root), pack), { written: 2, unchanged: 0 });
        const to = paths(root);
        assert.equal(fs.readFileSync(path.join(to.backendGameData, "progression_config.json"), "utf8"), "{\"paths\":1}\n");
        assert.equal(fs.readFileSync(path.join(to.rawGameData, "ladyluck_store.source.json"), "utf8"), "[1]\n");
        const bad = path.join(tempDir("pack-launcher-bad"), "x.json.gz");
        fs.writeFileSync(bad, "nope");
        await assert.rejects(launcherImport(workspacePaths(root), bad), /pack_invalid/);
    });
});
