import net from "node:net";
import path from "node:path";

export type Config = {
    port: number;
    bindHosts: string[];
    backendUrl: string;
    runtimeDir: string | undefined;
    brandingDir: string | undefined;
    newsFile: string | undefined;
    newsUrl: string | undefined;
    maxStreamsPerAccount: number;
    maxStreamsTotal: number;
    authCacheSeconds: number;
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

// The content server sits behind the gateway, which is the only listener players reach: it binds
// to loopback only.
export function IsLoopbackBindHost(Host: string): boolean {
    if(Host === "localhost"){
        return true;
    }
    if(net.isIPv4(Host)){
        return Host.startsWith("127.");
    }
    return Host === "::1";
}

export function LoadConfig(Env: NodeJS.ProcessEnv = process.env): Config {
    const Port = Int(Env, "PORT", 61002, 1, 65535);

    const BindHosts = (Optional(Env, "BIND_HOST") ?? "127.0.0.1").split(",").map((Host) => Host.trim()).filter((Host) => Host.length > 0);
    if(BindHosts.length === 0){
        throw new Error("BIND_HOST is empty");
    }
    for(const Host of BindHosts){
        if(!IsLoopbackBindHost(Host)){
            throw new Error(`BIND_HOST must be a loopback address (got ${JSON.stringify(Host)}): players reach the content server through the gateway`);
        }
    }

    const BackendUrl = (Optional(Env, "BACKEND_URL") ?? "http://127.0.0.1:61000").replace(/\/+$/, "");
    let Parsed: URL;
    try{
        Parsed = new URL(BackendUrl);
    }
    catch{
        throw new Error("BACKEND_URL is not a URL");
    }
    if(Parsed.protocol !== "http:" || !IsLoopbackBindHost(Parsed.hostname.replace(/^\[|\]$/g, ""))){
        throw new Error("BACKEND_URL must be http:// on this machine (the account keys travel in it)");
    }

    const Dir = (Name: string) => {
        const Value = Optional(Env, Name);
        return Value === undefined ? undefined : path.resolve(Value);
    };

    return {
        port: Port,
        bindHosts: BindHosts,
        backendUrl: BackendUrl,
        runtimeDir: Dir("CONTENT_RUNTIME_DIR"),
        brandingDir: Dir("CONTENT_BRANDING_DIR"),
        newsFile: Dir("CONTENT_NEWS_FILE"),
        newsUrl: Optional(Env, "CONTENT_NEWS_URL"),
        maxStreamsPerAccount: Int(Env, "CONTENT_MAX_STREAMS_PER_ACCOUNT", 4, 1, 64),
        maxStreamsTotal: Int(Env, "CONTENT_MAX_STREAMS_TOTAL", 48, 1, 1024),
        authCacheSeconds: Int(Env, "CONTENT_AUTH_CACHE_SECONDS", 300, 0, 3600),
    };
}
