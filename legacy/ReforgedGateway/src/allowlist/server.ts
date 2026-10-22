import { logger } from "../log";
import { AllowlistConfig, LoadAllowlistConfig } from "./config";
import { BuildScriptFor, IsElevated, PowerShellRunner, ScriptRunner } from "./firewall";
import { CanManageNft, NftRunner } from "./nft";
import { AllowlistHelper, AuditLog, DryRunRunner } from "./helper";

function Fatal(Message: string): never {
    logger.error(Message);
    process.exit(1);
}

async function Main(){
    let Config: AllowlistConfig;
    try{
        Config = LoadAllowlistConfig();
    }
    catch(error){
        Fatal(`Configuration: ${error instanceof Error ? error.message : String(error)}`);
    }

    const Audit = new AuditLog(Config.auditLog);
    // `--close-ports`: disable the rule once and exit. For stop scripts: on Windows a supervisor
    // usually ends the helper with TerminateProcess, and then its own fail-closed step never runs.
    const CloseOnly = process.argv.includes("--close-ports");

    let Runner: ScriptRunner;
    if(Config.dryRun){
        Runner = DryRunRunner(Audit);
        logger.warn("DRY RUN: the firewall is never changed; every change is only written to the audit log");
    }
    else if(Config.firewall === "nftables"){
        if(process.platform !== "linux"){
            Fatal("ALLOWLIST_FIREWALL=nftables needs Linux");
        }
        if(!CanManageNft(Config.nft.binary)){
            Fatal(`${Config.nft.binary} could not list the firewall: install nftables (apt install nftables) and run the helper as root or with CAP_NET_ADMIN (the reforged-allowlist systemd unit does)`);
        }
        Runner = NftRunner(Config.nft.binary);
    }
    else{
        if(process.platform !== "win32"){
            Fatal("ALLOWLIST_FIREWALL=windows needs Windows (Windows Firewall through PowerShell); on Linux use ALLOWLIST_FIREWALL=nftables");
        }
        if(!(await IsElevated(Config.powershell))){
            Fatal("ALLOWLIST_DRY_RUN=0 needs administrator rights: run it as a scheduled task with \"Run with highest privileges\"");
        }
        Runner = PowerShellRunner(Config.powershell);
    }

    if(CloseOnly){
        try{
            await Runner(BuildScriptFor(Config, []));
            Audit.Write("closed_ports", { dryRun: Config.dryRun });
            process.exit(0);
        }
        catch(error){
            Fatal(`Could not disable the game-port rule: ${error instanceof Error ? error.message : String(error)}`);
        }
    }

    const Helper = new AllowlistHelper(Config, { runner: Runner, audit: Audit });
    try{
        await Helper.Start();
    }
    catch(error){
        Fatal(`Could not listen on ${Config.bindHost}:${Config.port}: ${error instanceof Error ? error.message : String(error)}`);
    }

    logger.info("allowlist helper ready", {
        listen: `${Config.bindHost}:${Config.port}`,
        dryRun: Config.dryRun,
        firewall: Config.firewall,
        ports: Config.ports,
        ttlSeconds: Config.ttlMs / 1000,
        minIntervalMs: Config.minIntervalMs,
        maxEntries: Config.maxEntries,
        allowPrivate: Config.allowPrivate,
        auditLog: Config.auditLog,
        stateFile: Config.stateFile,
    });

    let Stopping = false;
    const Stop = (Signal: string) => {
        if(Stopping){
            return;
        }
        Stopping = true;
        logger.info("stopping: disabling the game-port rule", { signal: Signal });
        Helper.Close().then(() => process.exit(0), (error) => Fatal(`Shutdown failed: ${error instanceof Error ? error.message : String(error)}`));
        setTimeout(() => process.exit(1), 90_000).unref();
    };
    process.on("SIGINT", () => Stop("SIGINT"));
    process.on("SIGTERM", () => Stop("SIGTERM"));
    process.on("SIGBREAK", () => Stop("SIGBREAK"));
}

Main().catch((error) => Fatal(error instanceof Error ? error.stack ?? error.message : String(error)));
