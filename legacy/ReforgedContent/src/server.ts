import http from "node:http";
import { CreateContentHandler } from "./app";
import { AuthCache, BackendLookup } from "./auth";
import { Branding } from "./branding";
import { Config, LoadConfig } from "./config";
import { StreamLimiter } from "./limits";
import { logger } from "./log";
import { News } from "./news";
import { RuntimeStore } from "./runtime";

function Fatal(Message: string): never {
    logger.error(Message);
    process.exit(1);
}

function Listen(Server: http.Server, Host: string, Port: number): Promise<void> {
    return new Promise((resolve, reject) => {
        const OnError = (error: Error) => reject(error);
        Server.once("error", OnError);
        Server.listen(Port, Host, () => {
            Server.off("error", OnError);
            resolve();
        });
    });
}

async function Main(){
    let TheConfig: Config;
    try{
        TheConfig = LoadConfig();
    }
    catch(error){
        Fatal(`Configuration: ${error instanceof Error ? error.message : String(error)}`);
    }

    const Runtime = new RuntimeStore(TheConfig.runtimeDir);
    const First = Runtime.get();
    if(First.status !== "ok"){
        logger.warn("no runtime is offered to players yet; the launcher shows them that the host has to publish one", { dir: TheConfig.runtimeDir ?? null, reason: First.reason });
    }

    let NewsSource: News;
    try{
        NewsSource = new News(TheConfig.newsFile, Date.now, TheConfig.newsUrl);
    }
    catch(error){
        Fatal(`Configuration: ${error instanceof Error ? error.message : String(error)}`);
    }

    const Handler = CreateContentHandler({
        runtime: Runtime,
        auth: new AuthCache({ lookup: BackendLookup(TheConfig.backendUrl), ttlMs: TheConfig.authCacheSeconds * 1000 }),
        limiter: new StreamLimiter(TheConfig.maxStreamsPerAccount, TheConfig.maxStreamsTotal),
        branding: new Branding(TheConfig.brandingDir),
        news: NewsSource,
    });

    const Servers: http.Server[] = [];
    for(const Host of TheConfig.bindHosts){
        const Server = http.createServer({ requestTimeout: 30_000, headersTimeout: 20_000, keepAliveTimeout: 5_000 }, Handler);
        // A download whose client stopped reading for two minutes is dropped, which frees its slot.
        Server.setTimeout(120_000);
        Server.maxConnections = 512;
        try{
            await Listen(Server, Host, TheConfig.port);
        }
        catch(error){
            Fatal(`Could not listen on ${Host}:${TheConfig.port}: ${error instanceof Error ? error.message : String(error)}`);
        }
        Servers.push(Server);
    }

    logger.info("content server ready", {
        listen: TheConfig.bindHosts.map((Host) => `${Host}:${TheConfig.port}`),
        backend: TheConfig.backendUrl,
        runtime: TheConfig.runtimeDir ?? null,
        branding: TheConfig.brandingDir ?? null,
        news: TheConfig.newsFile ?? TheConfig.newsUrl ?? null,
        maxStreamsPerAccount: TheConfig.maxStreamsPerAccount,
        maxStreamsTotal: TheConfig.maxStreamsTotal,
    });

    const Stop = (Signal: string) => {
        logger.info("stopping", { signal: Signal });
        for(const Server of Servers){
            Server.close();
            Server.closeAllConnections();
        }
        setTimeout(() => process.exit(0), 500).unref();
    };
    process.on("SIGINT", () => Stop("SIGINT"));
    process.on("SIGTERM", () => Stop("SIGTERM"));
    process.on("SIGBREAK", () => Stop("SIGBREAK"));
}

Main().catch((error) => Fatal(error instanceof Error ? error.stack ?? error.message : String(error)));
