import { spawn } from "node:child_process";
import { CheckAllowlistIp } from "../ip";
import type { AllowlistConfig } from "./config";
import { CheckPorts } from "./ports";
import { BuildNftScript } from "./nft";

// The one Windows Firewall rule this helper owns. Name is the stable id; DisplayName is what
// the host sees in "Windows Defender Firewall with Advanced Security".
export const RULE_NAME = "ReforgedRevival-GamePorts-Allowlist";
export const RULE_DISPLAY_NAME = "Dauntless Reforged Revival game ports (allowlist)";

export type ScriptRunner = (Script: string) => Promise<void>;

// The whole PowerShell script for one desired state. Addresses reach this point already
// validated; they are checked again here so no caller can ever put anything else into a script.
//  - non-empty set: create or update the rule (inbound UDP on the game ports, Allow, only from
//    those addresses) and enable it
//  - empty set: disable the rule (Windows then drops the game ports' traffic by default)
// Any other rule with the same display name is disabled, so exactly one rule is in effect.
export function BuildFirewallScript(Addresses: readonly string[], Ports: string): string {
    const CheckedPorts = CheckPorts(Ports);
    for(const Address of Addresses){
        const Check = CheckAllowlistIp(Address, true);
        if(!Check.ok || Check.ip.canonical !== Address){
            throw new Error(`refusing to build a firewall script with ${JSON.stringify(Address)}`);
        }
    }

    const Lines = [
        "$ErrorActionPreference = 'Stop'",
        `$Name = '${RULE_NAME}'`,
        `$Display = '${RULE_DISPLAY_NAME}'`,
        "Get-NetFirewallRule -DisplayName $Display -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne $Name } | Disable-NetFirewallRule",
        "$Rule = Get-NetFirewallRule -Name $Name -ErrorAction SilentlyContinue",
    ];

    if(Addresses.length === 0){
        Lines.push(
            "if ($null -ne $Rule) {",
            "    Set-NetFirewallRule -Name $Name -Enabled False",
            "}",
        );
    }
    else{
        const Settings = `-Direction Inbound -Action Allow -Protocol UDP -LocalPort '${CheckedPorts}' -RemoteAddress $Addresses -Profile Any -Enabled True`;
        Lines.push(
            `$Addresses = @(${Addresses.map((Address) => `'${Address}'`).join(", ")})`,
            "if ($null -eq $Rule) {",
            `    New-NetFirewallRule -Name $Name -DisplayName $Display ${Settings} | Out-Null`,
            "} else {",
            `    Set-NetFirewallRule -Name $Name -NewDisplayName $Display ${Settings}`,
            "}",
        );
    }

    return Lines.join("\r\n") + "\r\n";
}

// The script for this helper's firewall: PowerShell for Windows Firewall, or an nftables script.
export function BuildScriptFor(Config: Pick<AllowlistConfig, "firewall" | "ports" | "nft">, Addresses: readonly string[]): string {
    return Config.firewall === "nftables"
        ? BuildNftScript(Addresses, Config.ports, Config.nft)
        : BuildFirewallScript(Addresses, Config.ports);
}

// Runs a script in Windows PowerShell 5.1 fed through stdin (no temp file, no command-line
// length limit). Rejects on a non-zero exit code, with the first part of stderr.
export function PowerShellRunner(Exe: string, TimeoutMs = 60_000): ScriptRunner {
    return (Script: string) => new Promise((resolve, reject) => {
        const Child = spawn(Exe, [
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$s = [Console]::In.ReadToEnd(); & ([scriptblock]::Create($s))",
        ], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true });

        let Stderr = "";
        let Stdout = "";
        Child.stdout.on("data", (Chunk: Buffer) => {
            Stdout = (Stdout + Chunk.toString("utf8")).slice(-4000);
        });
        Child.stderr.on("data", (Chunk: Buffer) => {
            Stderr = (Stderr + Chunk.toString("utf8")).slice(-4000);
        });

        const Timer = setTimeout(() => {
            Child.kill();
            reject(new Error(`PowerShell did not finish within ${TimeoutMs} ms`));
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
            const Detail = (Stderr.trim() || Stdout.trim()).slice(0, 1000);
            reject(new Error(`PowerShell exited with ${Code}${Detail === "" ? "" : `: ${Detail}`}`));
        });

        Child.stdin.on("error", () => undefined);
        Child.stdin.end(Script, "ascii");
    });
}

// Read-only: is this process running with administrator rights? (Real mode refuses to start
// without them, because every firewall change would fail.)
export async function IsElevated(Exe: string): Promise<boolean> {
    try{
        await PowerShellRunner(Exe, 30_000)(
            "if (([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { exit 0 } else { exit 3 }\r\n",
        );
        return true;
    }
    catch{
        return false;
    }
}
