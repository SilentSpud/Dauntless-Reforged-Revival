// Where everything lives. The repository holds the code; .workspace/ (git-ignored) holds what is
// the host's own: the pinned upstream source with the patches applied, the SDK generated from
// their game, game data, builds, configuration with secrets, certificates, the database and logs.

import path from "node:path";

export const GAME_EXE_RELATIVE = path.join("Archon", "Binaries", "Win64", "Dauntless-Win64-Shipping.exe");
export const RUNTIME_FILE_NAMES = ["MysticParadox.dll", "winmm.dll"] as const;
export const GAME_CHANGELIST = 647472;
export const GAME_VERSION_TEXT = "dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_647472";
export const PINNED_EXE_SHA256 = "f3f95fbf22709ef077a11632ed95d675b7209b7e0172d9ecb9ba7d02a1fbb66b";

export const PORTS = {
    backend: 61000,
    director: 61001,
    content: 61002,
    allowlist: 61005,
    publicGateway: 443,
    privateGateway: 61000,
    relay: 61100,
    mongo: 27017,
    // The game servers' UDP ports: Ramsgate on gameLast, the Training Grounds on the one below, hunts below
    // that. Up to 6 hunts they are 8770-8777, as always; more hunts reach further down (gamePortRange).
    gameFirst: 8770,
    gameLast: 8777,
    gameLowest: 8766,
} as const;

// Hunts at once: 1 to MAX_HUNTS (the ports from gameLowest to gameLast).
export const MAX_HUNTS = PORTS.gameLast - PORTS.gameLowest - 1;

// The UDP ports the game servers use with this many hunts at once: 8770-8777 up to 6 (unchanged, so a
// firewall opened for them keeps working), one more port below 8770 for each hunt above 6 (8766-8777 at 10).
export function gamePortRange(maxHunts: number): { first: number; last: number } {
    const hunts = Math.min(Math.max(Math.trunc(maxHunts) || 1, 1), MAX_HUNTS);
    return { first: Math.min(PORTS.gameFirst, PORTS.gameLast - 1 - hunts), last: PORTS.gameLast };
}

export interface Paths {
    repo: string;
    // The project's server preset (preset.ts), committed with the project.
    preset: string;
    hostKit: string;
    upstreamPin: string;
    patchesDir: string;
    gatewayDir: string;
    contentDir: string;
    workspace: string;
    upstream: string;
    marker: string;
    backendDir: string;
    directorDir: string;
    runtimeSource: string;
    sdkHeader: string;
    sdkDir: string;
    runtimeDll: string;
    loaderDir: string;
    loaderDll: string;
    exporterDll: string;
    backendGameData: string;
    directorGameData: string;
    rawGameData: string;
    exports: string;
    storeCaptures: string;
    capturesPin: string;
    runtimePublish: string;
    config: string;
    hostJson: string;
    tls: string;
    certFile: string;
    keyFile: string;
    mongoData: string;
    logs: string;
    gameserverLogs: string;
    branding: string;
    newsFile: string;
    // Linux: what runs the Windows game servers (wine.ts) and the systemd units.
    bin: string;
    gameserverWrapper: string;
    winePrefix: string;
    protonData: string;
    systemd: string;
}

// root: the Dauntless Reforged Revival checkout (the folder with ReforgedHost/ and upstream/).
export function paths(root: string): Paths {
    const workspace = path.join(root, ".workspace");
    const upstream = path.join(workspace, "Mystic-Paradox");
    const runtimeSource = path.join(upstream, "ParadoxRuntime");
    const config = path.join(workspace, "config");
    const tls = path.join(workspace, "tls");
    const logs = path.join(workspace, "logs");
    return {
        repo: root,
        preset: path.join(root, "preset"),
        hostKit: path.join(root, "ReforgedHost"),
        upstreamPin: path.join(root, "upstream", "mystic-paradox.json"),
        patchesDir: path.join(root, "upstream"),
        gatewayDir: path.join(root, "ReforgedGateway"),
        contentDir: path.join(root, "ReforgedContent"),
        workspace,
        upstream,
        marker: path.join(upstream, ".reforged-patches.json"),
        backendDir: path.join(upstream, "ParadoxBackend"),
        directorDir: path.join(upstream, "ParadoxDirector"),
        runtimeSource,
        sdkHeader: path.join(runtimeSource, "SDK.hpp"),
        sdkDir: path.join(runtimeSource, "SDK"),
        runtimeDll: path.join(runtimeSource, "x64", "Release", "MysticParadox.dll"),
        loaderDir: path.join(upstream, "tools", "RuntimeLoader"),
        loaderDll: path.join(upstream, "tools", "RuntimeLoader", "target", "release", "winmm.dll"),
        exporterDll: path.join(upstream, "tools", "CatalogExporter", "x64", "Release", "CatalogExporter.dll"),
        backendGameData: path.join(upstream, "ParadoxBackend", "game-data"),
        directorGameData: path.join(upstream, "ParadoxDirector", "game-data"),
        rawGameData: path.join(upstream, "ParadoxBackend", "game-data", "raw"),
        exports: path.join(upstream, "Items_Analysis"),
        storeCaptures: path.join(upstream, "DauntlessEndpointDocumentation"),
        capturesPin: path.join(root, "upstream", "store-captures.json"),
        runtimePublish: path.join(workspace, "runtime"),
        config,
        hostJson: path.join(config, "host.json"),
        tls,
        certFile: path.join(tls, "gateway-cert.pem"),
        keyFile: path.join(tls, "gateway-key.pem"),
        mongoData: path.join(workspace, "mongodb"),
        logs,
        gameserverLogs: path.join(logs, "gameservers"),
        branding: path.join(workspace, "branding"),
        newsFile: path.join(config, "news.json"),
        bin: path.join(workspace, "bin"),
        gameserverWrapper: path.join(workspace, "bin", "reforged-gameserver"),
        winePrefix: path.join(workspace, "wine"),
        protonData: path.join(workspace, "proton"),
        systemd: path.join(config, "systemd"),
    };
}

// The repository root, from this file's place in it (ReforgedHost/dist/ or build/ReforgedHost/src/).
export function findRepoRoot(start: string = __dirname): string {
    const fs = require("node:fs") as typeof import("node:fs");
    let dir = start;
    for (let i = 0; i < 8; i++) {
        if (fs.existsSync(path.join(dir, "upstream", "mystic-paradox.json")) && fs.existsSync(path.join(dir, "ReforgedHost"))) return dir;
        const up = path.dirname(dir);
        if (up === dir) break;
        dir = up;
    }
    throw new Error("Could not find the Dauntless Reforged Revival folder (with upstream/ and ReforgedHost/) above " + start);
}
