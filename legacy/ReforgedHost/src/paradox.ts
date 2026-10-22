// Mystic Paradox is part of this repository: its backend and Director in paradox/, the game runtime, its
// loader and the CatalogExporter in native/. Nothing is downloaded from Paradox's repository any more.
//
// Hosts set up before that had it in .workspace/Mystic-Paradox, a patched checkout that also held their own
// files: the SDK from their game, game data, exports and builds. migrateOldWorkspace copies those into the
// new places once, where nothing of the host's is there yet, and never deletes the old folder.

import fs from "node:fs";
import path from "node:path";
import type { Paths } from "./paths";
import { capture } from "./run";

export function gitAvailable(): boolean {
    return capture("git", ["--version"]) !== null;
}

// The folders the host kit builds from, relative to the repository. A checkout without one of them is
// incomplete (a sparse or damaged clone).
export function missingParadox(p: Paths): string[] {
    const needed: [string, string][] = [
        ["paradox/backend", path.join(p.backendDir, "package.json")],
        ["paradox/director", path.join(p.directorDir, "package.json")],
        ["native/paradox-runtime", path.join(p.runtimeSource, "MysticParadox.vcxproj")],
        ["native/runtime-loader", path.join(p.loaderDir, "Cargo.toml")],
        ["native/catalog-exporter", path.join(p.exporterDir, "CatalogExporter.vcxproj")],
    ];
    return needed.filter(([, file]) => !fs.existsSync(file)).map(([name]) => name);
}

// The Dumper-7 SDK files next to SDK.hpp (the SDK folder is copied as a whole).
export const SDK_FILES = ["SDK.hpp", "Assertions.inl", "NameCollisions.inl", "PropertyFixup.hpp", "UnrealContainers.hpp", "UtfN.hpp"];

// What the old .workspace/Mystic-Paradox held of the host's own, as [old path, new path] pairs. Game data is
// listed file by file, so a file the preset or the host already put in the new place is kept.
export function oldWorkspaceItems(p: Paths): [string, string][] {
    const old = p.oldUpstream;
    const items: [string, string][] = [];
    const runtime = path.join(old, "ParadoxRuntime");
    for (const f of [...SDK_FILES, "SDK"]) items.push([path.join(runtime, f), path.join(p.runtimeSource, f)]);
    items.push([path.join(runtime, "x64", "Release", "MysticParadox.dll"), p.runtimeDll]);
    items.push([path.join(old, "tools", "RuntimeLoader", "target", "release", "winmm.dll"), p.loaderDll]);
    items.push([path.join(old, "tools", "CatalogExporter", "x64", "Release", "CatalogExporter.dll"), p.exporterDll]);
    items.push([path.join(old, "Items_Analysis"), p.exports]);
    items.push([path.join(old, "DauntlessEndpointDocumentation"), p.storeCaptures]);
    for (const [from, to] of [
        [path.join(old, "ParadoxBackend", "game-data"), p.backendGameData],
        [path.join(old, "ParadoxDirector", "game-data"), p.directorGameData],
    ]) {
        if (!fs.existsSync(from)) continue;
        for (const entry of fs.readdirSync(from, { withFileTypes: true })) {
            // The committed examples are already in place; raw/ is the host's own sources.
            if (entry.isFile() && entry.name.endsWith(".json") && !entry.name.endsWith(".example.json")) items.push([path.join(from, entry.name), path.join(to, entry.name)]);
            else if (entry.isDirectory() && entry.name === "raw") items.push([path.join(from, entry.name), path.join(to, entry.name)]);
        }
    }
    return items;
}

// Copies the host's own files from the old .workspace/Mystic-Paradox. Returns what was copied, relative to
// the repository. Anything already in the new place is left as it is.
export function migrateOldWorkspace(p: Paths): string[] {
    if (!fs.existsSync(p.oldUpstream)) return [];
    const copied: string[] = [];
    for (const [from, to] of oldWorkspaceItems(p)) {
        if (!fs.existsSync(from)) continue;
        if (fs.existsSync(to) && !(fs.statSync(to).isDirectory() && fs.readdirSync(to).length === 0)) continue;
        fs.mkdirSync(path.dirname(to), { recursive: true });
        fs.cpSync(from, to, { recursive: true, force: false, errorOnExist: false });
        copied.push(path.relative(p.repo, to).split(path.sep).join("/"));
    }
    return copied;
}
