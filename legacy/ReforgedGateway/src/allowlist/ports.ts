// "8770-8777" or "8777": ports 1-65535, a range low to high, nothing else.
export function CheckPorts(Text: string): string {
    const Match = /^(\d{1,5})(?:-(\d{1,5}))?$/.exec(Text);
    if(Match === null){
        throw new Error(`ALLOWLIST_PORTS must be a port or a range like 8770-8777 (got ${JSON.stringify(Text)})`);
    }
    const Low = Number(Match[1]);
    const High = Match[2] === undefined ? Low : Number(Match[2]);
    if(Low < 1 || High > 65535 || Low > High){
        throw new Error("ALLOWLIST_PORTS must be within 1-65535, low to high");
    }
    return Match[2] === undefined ? String(Low) : `${Low}-${High}`;
}
