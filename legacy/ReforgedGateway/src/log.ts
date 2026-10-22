// One JSON object per line on stdout (stderr for warnings and errors), so the host can grep or
// load the log. Nothing here ever receives a key, token or secret: callers pass redacted paths
// and never pass headers.

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

// Logs one line per key at most every IntervalMs, so a flood (a scanner, a helper that is down)
// cannot fill the disk. Returns how many lines were held back since the last one it let through.
export class LogThrottle {
    private readonly Last = new Map<string, { at: number; suppressed: number }>();

    constructor(private readonly IntervalMs: number, private readonly MaxKeys = 10_000){}

    Allow(Key: string, Now = Date.now()): number | undefined {
        const Entry = this.Last.get(Key);
        if(Entry !== undefined && Now - Entry.at < this.IntervalMs){
            Entry.suppressed++;
            return undefined;
        }
        if(this.Last.size >= this.MaxKeys){
            this.Last.clear();
        }
        const Suppressed = Entry?.suppressed ?? 0;
        this.Last.set(Key, { at: Now, suppressed: 0 });
        return Suppressed;
    }
}
