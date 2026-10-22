import path from "node:path";
import { CheckSecret } from "../config";
import { CheckInterface, CheckNftTable, DefaultNft, NFT_TABLE, type NftOptions } from "./nft";
import { CheckPorts } from "./ports";

// windows: Windows Firewall through PowerShell. nftables: one nftables table on Linux.
export type FirewallKind = "windows" | "nftables";

export type AllowlistConfig = {
    bindHost: string;
    port: number;
    secret: string;
    // true: only log the script it would run. Never touches the firewall.
    dryRun: boolean;
    firewall: FirewallKind;
    allowPrivate: boolean;
    ttlMs: number;
    minIntervalMs: number;
    maxEntries: number;
    // Local UDP ports the rule opens, PowerShell -LocalPort syntax ("8770-8777").
    ports: string;
    auditLog: string;
    stateFile: string;
    powershell: string;
    nft: NftOptions;
};

function Int(Env: NodeJS.ProcessEnv, Name: string, Default: number, Min: number, Max: number): number {
    const Raw = Env[Name];
    if(Raw === undefined || Raw.trim() === ""){
        return Default;
    }
    const Value = Number(Raw);
    if(!Number.isInteger(Value) || Value < Min || Value > Max){
        throw new Error(`${Name} must be a whole number from ${Min} to ${Max}`);
    }
    return Value;
}

function Optional(Env: NodeJS.ProcessEnv, Name: string): string | undefined {
    const Raw = Env[Name];
    return Raw === undefined || Raw.trim() === "" ? undefined : Raw.trim();
}

// The port check lives in ports.ts (nft.ts needs it too); kept exported from here.
export { CheckPorts } from "./ports";

function InterfaceOf(Name: string | undefined): string | undefined {
    return Name === undefined ? undefined : CheckInterface(Name);
}

export function DefaultPowerShell(): string {
    // A fixed path, never PATH: this process runs with administrator rights.
    const Root = process.env.SystemRoot ?? "C:\\Windows";
    return path.join(Root, "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
}

export function LoadAllowlistConfig(Env: NodeJS.ProcessEnv = process.env): AllowlistConfig {
    const DryRunRaw = Optional(Env, "ALLOWLIST_DRY_RUN");
    if(DryRunRaw !== "0" && DryRunRaw !== "1"){
        throw new Error("ALLOWLIST_DRY_RUN must be set: 1 only logs what it would do (any test machine), 0 really changes the firewall (the server)");
    }

    const BindHost = Optional(Env, "ALLOWLIST_BIND") ?? "127.0.0.1";
    if(BindHost !== "127.0.0.1" && BindHost !== "::1"){
        throw new Error("ALLOWLIST_BIND must be 127.0.0.1 or ::1: the helper runs with administrator rights and only the gateway on this machine may reach it");
    }

    // Windows unless the env file says otherwise: the host kit writes nftables on Linux.
    const FirewallRaw = Optional(Env, "ALLOWLIST_FIREWALL") ?? "windows";
    if(FirewallRaw !== "windows" && FirewallRaw !== "nftables"){
        throw new Error(`ALLOWLIST_FIREWALL must be windows (Windows Firewall) or nftables (Linux), not ${JSON.stringify(FirewallRaw)}`);
    }
    const Nft = Optional(Env, "ALLOWLIST_NFT");
    if(Nft !== undefined && !path.posix.isAbsolute(Nft)){
        throw new Error("ALLOWLIST_NFT must be the absolute path of nft (the helper never looks it up in PATH)");
    }

    return {
        bindHost: BindHost,
        port: Int(Env, "ALLOWLIST_PORT", 61005, 1, 65535),
        secret: CheckSecret(Optional(Env, "ALLOWLIST_SECRET"), "ALLOWLIST_SECRET"),
        dryRun: DryRunRaw === "1",
        firewall: FirewallRaw,
        allowPrivate: Env.ALLOWLIST_ALLOW_PRIVATE === "1",
        ttlMs: Int(Env, "ALLOWLIST_TTL_SECONDS", 600, 1, 86_400) * 1000,
        minIntervalMs: Int(Env, "ALLOWLIST_MIN_INTERVAL_MS", 3000, 50, 600_000),
        maxEntries: Int(Env, "ALLOWLIST_MAX_ENTRIES", 256, 1, 1000),
        ports: CheckPorts(Optional(Env, "ALLOWLIST_PORTS") ?? "8770-8777"),
        auditLog: path.resolve(Optional(Env, "ALLOWLIST_AUDIT_LOG") ?? "allowlist-audit.log"),
        stateFile: path.resolve(Optional(Env, "ALLOWLIST_STATE_FILE") ?? "allowlist-state.json"),
        powershell: Optional(Env, "ALLOWLIST_POWERSHELL") ?? DefaultPowerShell(),
        nft: {
            binary: Nft ?? DefaultNft(),
            table: CheckNftTable(Optional(Env, "ALLOWLIST_NFT_TABLE") ?? NFT_TABLE),
            iface: InterfaceOf(Optional(Env, "ALLOWLIST_INTERFACE")),
        },
    };
}
