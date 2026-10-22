// Where everything lives. The repository holds the code, Mystic Paradox's included (paradox/, native/); what
// is the host's own is git-ignored: the SDK generated from their game, game data, exports and builds next to
// the code that uses them, and in .workspace/ the configuration with secrets, certificates, the database and logs.

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
    gatewayDir: string;
    contentDir: string;
    workspace: string;
    paradox: string;
    // Where setup took the host's own files from before Mystic Paradox was part of the repository.
    oldUpstream: string;
    backendDir: string;
    directorDir: string;
    runtimeSource: string;
    sdkHeader: string;
    sdkDir: string;
    runtimeDll: string;
    loaderDir: string;
    loaderDll: string;
    exporterDir: string;
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

// root: the Dauntless Reforged Revival checkout (the folder with legacy/ReforgedHost/ and paradox/).
export function paths(root: string): Paths {
    const workspace = path.join(root, ".workspace");
    const paradox = path.join(root, "paradox");
    const native = path.join(root, "native");
    const runtimeSource = path.join(native, "paradox-runtime");
    const loaderDir = path.join(native, "runtime-loader");
    const exporterDir = path.join(native, "catalog-exporter");
    const config = path.join(workspace, "config");
    const tls = path.join(workspace, "tls");
    const logs = path.join(workspace, "logs");
    return {
        repo: root,
        preset: path.join(root, "preset"),
        hostKit: path.join(root, "legacy", "ReforgedHost"),
        gatewayDir: path.join(root, "legacy", "ReforgedGateway"),
        contentDir: path.join(root, "legacy", "ReforgedContent"),
        workspace,
        paradox,
        oldUpstream: path.join(workspace, "Mystic-Paradox"),
        backendDir: path.join(paradox, "backend"),
        directorDir: path.join(paradox, "director"),
        runtimeSource,
        sdkHeader: path.join(runtimeSource, "SDK.hpp"),
        sdkDir: path.join(runtimeSource, "SDK"),
        runtimeDll: path.join(runtimeSource, "x64", "Release", "MysticParadox.dll"),
        loaderDir,
        loaderDll: path.join(loaderDir, "target", "release", "winmm.dll"),
        exporterDir,
        exporterDll: path.join(exporterDir, "x64", "Release", "CatalogExporter.dll"),
        backendGameData: path.join(paradox, "backend", "game-data"),
        directorGameData: path.join(paradox, "director", "game-data"),
        rawGameData: path.join(paradox, "backend", "game-data", "raw"),
        // Mystic Paradox's generators read these from its root (paradox/).
        exports: path.join(paradox, "Items_Analysis"),
        storeCaptures: path.join(paradox, "DauntlessEndpointDocumentation"),
        capturesPin: path.join(paradox, "backend", "store-captures.json"),
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

// The repository root, from this file's place in it (legacy/ReforgedHost/dist/ or legacy/ReforgedHost/build/ReforgedHost/src/).
export function findRepoRoot(start: string = __dirname): string {
    const fs = require("node:fs") as typeof import("node:fs");
    let dir = start;
    for (let i = 0; i < 8; i++) {
        if (fs.existsSync(path.join(dir, "paradox", "backend", "package.json")) && fs.existsSync(path.join(dir, "legacy", "ReforgedHost"))) return dir;
        const up = path.dirname(dir);
        if (up === dir) break;
        dir = up;
    }
    throw new Error("Could not find the Dauntless Reforged Revival folder (with paradox/ and legacy/ReforgedHost/) above " + start);
}
