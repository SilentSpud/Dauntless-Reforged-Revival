// One JSON object per line on stdout (stderr for warnings and errors), so the host can grep or
// load the log. Nothing here ever receives an account key: callers log the account id instead.

export type LogLevel = "debug" | "info" | "warn" | "error";

const ORDER: Record<LogLevel, number> = { debug: 10, info: 20, warn: 30, error: 40 };

function ThresholdFromEnv(): number {
    const Wanted = (process.env.LOG_LEVEL || "info").toLowerCase() as LogLevel;
    return ORDER[Wanted] ?? ORDER.info;
}

let Threshold = ThresholdFromEnv();
let Sink: ((Line: string, Level: LogLevel) => void) | undefined;

// Tests capture the log instead of printing it.
export function SetLogSink(NewSink: ((Line: string, Level: LogLevel) => void) | undefined){
    Sink = NewSink;
}

export function SetLogLevel(Level: LogLevel){
    Threshold = ORDER[Level];
}

export function Log(Level: LogLevel, Msg: string, Fields: Record<string, unknown> = {}){
    if(ORDER[Level] < Threshold){
        return;
    }

    const Line = JSON.stringify({ t: new Date().toISOString(), level: Level, msg: Msg, ...Fields });

    if(Sink){
        Sink(Line, Level);
        return;
    }

    if(Level === "warn" || Level === "error"){
        process.stderr.write(Line + "\n");
    }
    else{
        process.stdout.write(Line + "\n");
    }
}

export const logger = {
    debug: (Msg: string, Fields?: Record<string, unknown>) => Log("debug", Msg, Fields),
    info: (Msg: string, Fields?: Record<string, unknown>) => Log("info", Msg, Fields),
    warn: (Msg: string, Fields?: Record<string, unknown>) => Log("warn", Msg, Fields),
    error: (Msg: string, Fields?: Record<string, unknown>) => Log("error", Msg, Fields),
};
