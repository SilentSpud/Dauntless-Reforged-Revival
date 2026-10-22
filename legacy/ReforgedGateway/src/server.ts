import fs from "node:fs";
import tls from "node:tls";
import { GatewayConfig, LoadGatewayConfig } from "./config";
import { AllowlistFeed } from "./feed";
import { DescribeCertificate, Gateway } from "./gateway";
import { logger } from "./log";

function Fatal(Message: string): never {
    logger.error(Message);
    process.exit(1);
}

async function Main(){
    let Config: GatewayConfig;
    try{
        Config = LoadGatewayConfig();
    }
    catch(error){
        Fatal(`Configuration: ${error instanceof Error ? error.message : String(error)}`);
    }

    let Tls: { cert: Buffer; key: Buffer } | null = null;
    if(Config.tls){
        let Cert: Buffer;
        let Key: Buffer;
        try{
            Cert = fs.readFileSync(Config.certFile);
            Key = fs.readFileSync(Config.keyFile);
            // Throws when the key does not belong to the certificate.
            tls.createSecureContext({ cert: Cert, key: Key });
        }
        catch(error){
            Fatal(`Certificate ${Config.certFile} / key ${Config.keyFile}: ${error instanceof Error ? error.message : String(error)}`);
        }

        const Info = DescribeCertificate(Cert);
        const Now = Date.now();
        if(Info.notAfter.getTime() <= Now || Info.notBefore.getTime() > Now){
            Fatal(`The certificate is not valid now (valid ${Info.notBefore.toISOString()} to ${Info.notAfter.toISOString()}). Make a new one with tools/make-cert.js; invites then need the new fingerprint.`);
        }
        if(Info.notAfter.getTime() - Now < 30 * 24 * 3600 * 1000){
            logger.warn("the certificate expires within 30 days; make a new one and send new invites", { notAfter: Info.notAfter.toISOString() });
        }
        Tls = { cert: Cert, key: Key };
    }

    const Feed = Config.allowlist === undefined ? undefined : new AllowlistFeed(Config.allowlist);
    if(Feed === undefined){
        logger.warn("allowlist feed is off (GATEWAY_ALLOWLIST=0): the game ports open for nobody unless the firewall is managed another way");
    }

    const TheGateway = new Gateway(Config, Tls, Feed);
    try{
        await TheGateway.Listen();
    }
    catch(error){
        Fatal(`Could not listen on ${Config.bindHost}:${Config.port}: ${error instanceof Error ? error.message : String(error)}`);
    }

    logger.info("gateway ready", {
        listen: `${Config.bindHost}:${Config.port}`,
        mode: Config.tls ? "public (TLS)" : "private (plain HTTP inside the tailnet)",
        ...(TheGateway.Certificate === null ? {} : {
            fingerprint: TheGateway.Certificate.fingerprint,
            certificateNames: TheGateway.Certificate.subjectAltName,
            certificateValidUntil: TheGateway.Certificate.notAfter.toISOString(),
        }),
        backend: `${Config.backend.host}:${Config.backend.port}`,
        content: `${Config.content.host}:${Config.content.port}`,
        websocket: `${Config.ws.host}:${Config.ws.port}`,
        allowlistFeed: Config.allowlist === undefined ? "off" : Config.allowlist.url,
        maxBodyBytes: Config.limits.maxBodyBytes,
        maxConnections: Config.limits.maxConnections,
        maxConnectionsPerIp: Config.limits.maxConnectionsPerIp,
        rate: Config.limits.rate,
    });

    let Stopping = false;
    const Stop = (Signal: string) => {
        if(Stopping){
            return;
        }
        Stopping = true;
        logger.info("stopping", { signal: Signal, graceMs: Config.timeouts.shutdownGraceMs });
        TheGateway.Close().then(() => {
            logger.info("stopped");
            process.exit(0);
        }, (error) => Fatal(`Shutdown failed: ${error instanceof Error ? error.message : String(error)}`));
        // Never hang on the way out.
        setTimeout(() => process.exit(0), Config.timeouts.shutdownGraceMs + 5_000).unref();
    };
    process.on("SIGINT", () => Stop("SIGINT"));
    process.on("SIGTERM", () => Stop("SIGTERM"));
    // Ctrl+Break on Windows.
    process.on("SIGBREAK", () => Stop("SIGBREAK"));
}

Main().catch((error) => Fatal(error instanceof Error ? error.stack ?? error.message : String(error)));
