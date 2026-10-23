import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { CheckAllowlistIp } from "../ip";
import { CheckPorts } from "./ports";

// Linux: the helper owns one nftables table (and nothing else). Its input chain lets the game
// ports' UDP traffic through only from the addresses in its two sets, and drops the rest of it.
// The table is adapted from Dauntless Revived's Linux port (deploy/linux-server/allowlist.mjs).
export const NFT_TABLE = "reforged_revival";

export type NftOptions = {
    // Absolute path of nft. A fixed path, never PATH: the helper runs with CAP_NET_ADMIN.
    binary: string;
    table: string;
    // Only filter traffic that arrives on this interface (eth0, ens3...). Unset: every interface but lo.
    iface: string | undefined;
};

export function CheckNftTable(Name: string): string {
    if(!/^[A-Za-z][A-Za-z0-9_]{0,31}$/.test(Name)){
        throw new Error(`ALLOWLIST_NFT_TABLE must be a letter then up to 31 letters, digits or _ (got ${JSON.stringify(Name)})`);
    }
    return Name;
}

export function CheckInterface(Name: string): string {
    // Linux interface names: at most 15 characters, no spaces, quotes or slashes.
    if(!/^[A-Za-z0-9][A-Za-z0-9_.@-]{0,14}$/.test(Name)){
        throw new Error(`ALLOWLIST_INTERFACE must be a network interface name like eth0 (got ${JSON.stringify(Name)})`);
    }
    return Name;
}

export function DefaultNft(): string {
    for(const Candidate of ["/usr/sbin/nft", "/sbin/nft"]){
        if(fs.existsSync(Candidate)){
            return Candidate;
        }
    }
    return "/usr/sbin/nft";
}

// The whole nftables script for one desired state. nft -f applies a file as one transaction, so
// the old table and the new one are swapped atomically: the first line makes sure the table
// exists, the second deletes it, and the rest defines it again with exactly these addresses.
//  - no addresses: the sets are empty and the game ports drop every packet (closed)
//  - addresses: only they reach the game ports
// Loopback is never filtered (as Windows Firewall never filters it). Addresses reach this point
// already validated; they are checked again so no caller can ever put anything else into a script.
export function BuildNftScript(Addresses: readonly string[], Ports: string, Options: Pick<NftOptions, "table" | "iface">): string {
    const CheckedPorts = CheckPorts(Ports);
    const Table = CheckNftTable(Options.table);
    const If = Options.iface === undefined ? "" : `iifname "${CheckInterface(Options.iface)}" `;
    const V4: string[] = [];
    const V6: string[] = [];
    for(const Address of Addresses){
        const Check = CheckAllowlistIp(Address, true);
        if(!Check.ok || Check.ip.canonical !== Address){
            throw new Error(`refusing to build a firewall script with ${JSON.stringify(Address)}`);
        }
        (net.isIPv4(Address) ? V4 : V6).push(Address);
    }

    const Set = (Name: string, Type: string, Elements: string[]) => [
        `\tset ${Name} {`,
        `\t\ttype ${Type}`,
        ...(Elements.length ? [`\t\telements = { ${Elements.join(", ")} }`] : []),
        "\t}",
    ];
    const Lines = [
        `table inet ${Table} {}`,
        `delete table inet ${Table}`,
        `table inet ${Table} {`,
        ...Set("players4", "ipv4_addr", V4),
        ...Set("players6", "ipv6_addr", V6),
        "\tchain input {",
        "\t\ttype filter hook input priority -5; policy accept;",
        "\t\tiifname \"lo\" accept",
        `\t\t${If}ip saddr @players4 udp dport ${CheckedPorts} accept`,
        `\t\t${If}ip6 saddr @players6 udp dport ${CheckedPorts} accept`,
        `\t\t${If}udp dport ${CheckedPorts} drop`,
        "\t}",
        "}",
    ];
    return Lines.join("\n") + "\n";
}

// Runs a script with nft -f. nft reads only a regular file (it refuses a pipe on stdin), so the
// script goes into a new private folder (0700) as a 0600 file, and both are removed afterwards.
// Rejects on a non-zero exit code, with the first part of stderr.
export function NftRunner(Binary: string, TimeoutMs = 30_000): (Script: string) => Promise<void> {
    return async (Script: string) => {
        const Dir = fs.mkdtempSync(path.join(os.tmpdir(), "reforged-nft-"));
        try{
            const File = path.join(Dir, "rules.nft");
            fs.writeFileSync(File, Script, { encoding: "ascii", mode: 0o600, flag: "wx" });
            await new Promise<void>((resolve, reject) => {
                const Child = spawn(Binary, ["-f", File], { stdio: ["ignore", "pipe", "pipe"] });
                let Stderr = "";
                Child.stdout.resume();
                Child.stderr.on("data", (Chunk: Buffer) => {
                    Stderr = (Stderr + Chunk.toString("utf8")).slice(-4000);
                });
                const Timer = setTimeout(() => {
                    Child.kill();
                    reject(new Error(`nft did not finish within ${TimeoutMs} ms`));
                }, TimeoutMs);
                Child.on("error", (error) => {
                    clearTimeout(Timer);
                    reject(error);
                });
                Child.on("close", (Code) => {
                    clearTimeout(Timer);
                    if(Code === 0){
                        resolve();
                        return;
                    }
                    const Detail = Stderr.trim().slice(0, 1000);
                    reject(new Error(`nft exited with ${Code}${Detail === "" ? "" : `: ${Detail}`}`));
                });
            });
        }
        finally{
            fs.rmSync(Dir, { recursive: true, force: true });
        }
    };
}

// Read-only: can this process change nftables (root, or CAP_NET_ADMIN)? Real mode refuses to start
// without it, because every firewall change would fail.
export function CanManageNft(Binary: string): boolean {
    if(!fs.existsSync(Binary)){
        return false;
    }
    return spawnSync(Binary, ["list", "tables"], { stdio: "ignore", timeout: 15_000 }).status === 0;
}
