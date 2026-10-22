// Strict IP address handling shared by the gateway (peer addresses, rate-limit keys) and the
// allowlist helper (what may go into the firewall rule). Only single addresses are accepted:
// no ranges, no CIDR, no zone ids, no leading zeros, no surrounding whitespace.

export type IpClass =
    | "public"
    | "private"       // RFC 1918, carrier-grade NAT / Tailscale (100.64.0.0/10), IPv6 ULA, benchmarking
    | "loopback"
    | "link-local"
    | "unspecified"   // 0.0.0.0/8 and ::
    | "multicast"
    | "reserved";     // 240.0.0.0/4 (with 255.255.255.255), 192.0.0.0/24, IPv6 outside 2000::/3, NAT64, discard

export type ParsedIp = {
    version: 4 | 6;
    canonical: string;
    class: IpClass;
    // Four bytes for IPv4, sixteen for IPv6.
    bytes: number[];
};

const V4_PART = /^(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9][0-9]|[0-9])$/;

function ParseV4Bytes(Text: string): number[] | undefined {
    const Parts = Text.split(".");
    if(Parts.length !== 4){
        return undefined;
    }
    const Bytes: number[] = [];
    for(const Part of Parts){
        if(!V4_PART.test(Part)){
            return undefined;
        }
        Bytes.push(Number(Part));
    }
    return Bytes;
}

function V4Number(Bytes: number[]): number {
    return ((Bytes[0] << 24) >>> 0) + (Bytes[1] << 16) + (Bytes[2] << 8) + Bytes[3];
}

function InV4(Value: number, Base: string, Bits: number): boolean {
    const BaseValue = V4Number(ParseV4Bytes(Base)!);
    const Mask = Bits === 0 ? 0 : (0xffffffff << (32 - Bits)) >>> 0;
    return ((Value & Mask) >>> 0) === ((BaseValue & Mask) >>> 0);
}

function ClassifyV4(Bytes: number[]): IpClass {
    const Value = V4Number(Bytes);
    if(InV4(Value, "0.0.0.0", 8)){
        return "unspecified";
    }
    if(InV4(Value, "127.0.0.0", 8)){
        return "loopback";
    }
    if(InV4(Value, "169.254.0.0", 16)){
        return "link-local";
    }
    if(InV4(Value, "224.0.0.0", 4)){
        return "multicast";
    }
    if(InV4(Value, "240.0.0.0", 4) || InV4(Value, "192.0.0.0", 24)){
        return "reserved";
    }
    if(InV4(Value, "10.0.0.0", 8) || InV4(Value, "172.16.0.0", 12) || InV4(Value, "192.168.0.0", 16)
        || InV4(Value, "100.64.0.0", 10) || InV4(Value, "198.18.0.0", 15)){
        return "private";
    }
    return "public";
}

const V6_GROUP = /^[0-9a-fA-F]{1,4}$/;

// Eight 16-bit words, or undefined. A dotted IPv4 tail is allowed in the last 32 bits.
function ParseV6Words(Text: string): number[] | undefined {
    if(Text.length < 2 || Text.length > 45 || !/^[0-9a-fA-F:.]+$/.test(Text)){
        return undefined;
    }

    let Body = Text;
    let Tail: number[] = [];
    const LastColon = Body.lastIndexOf(":");
    if(LastColon === -1){
        return undefined;
    }
    const LastPart = Body.slice(LastColon + 1);
    if(LastPart.includes(".")){
        const V4 = ParseV4Bytes(LastPart);
        if(V4 === undefined){
            return undefined;
        }
        Tail = [(V4[0] << 8) | V4[1], (V4[2] << 8) | V4[3]];
        Body = Body.slice(0, LastColon + 1);
        // "::1.2.3.4" leaves "::", "a::b:1.2.3.4" leaves "a::b:"; a lone trailing colon must go.
        if(Body.endsWith(":") && !Body.endsWith("::")){
            Body = Body.slice(0, -1);
        }
    }

    const Halves = Body.split("::");
    if(Halves.length > 2){
        return undefined;
    }

    const ParseGroups = (Part: string): number[] | undefined => {
        if(Part === ""){
            return [];
        }
        const Groups = Part.split(":");
        const Words: number[] = [];
        for(const Group of Groups){
            if(!V6_GROUP.test(Group)){
                return undefined;
            }
            Words.push(parseInt(Group, 16));
        }
        return Words;
    };

    const Head = ParseGroups(Halves[0]);
    if(Head === undefined){
        return undefined;
    }

    if(Halves.length === 1){
        const Words = [...Head, ...Tail];
        return Words.length === 8 ? Words : undefined;
    }

    const After = ParseGroups(Halves[1]);
    if(After === undefined){
        return undefined;
    }
    const Known = Head.length + After.length + Tail.length;
    // "::" stands for at least one zero word.
    if(Known > 7){
        return undefined;
    }
    return [...Head, ...new Array(8 - Known).fill(0), ...After, ...Tail];
}

// RFC 5952: lowercase, no leading zeros, the longest run (first on a tie) of two or more zero
// words becomes "::".
function FormatV6(Words: number[]): string {
    let BestStart = -1;
    let BestLength = 0;
    for(let Index = 0; Index < 8;){
        if(Words[Index] !== 0){
            Index++;
            continue;
        }
        let End = Index;
        while(End < 8 && Words[End] === 0){
            End++;
        }
        if(End - Index > BestLength){
            BestStart = Index;
            BestLength = End - Index;
        }
        Index = End;
    }

    const Hex = Words.map((Word) => Word.toString(16));
    if(BestLength < 2){
        return Hex.join(":");
    }
    return Hex.slice(0, BestStart).join(":") + "::" + Hex.slice(BestStart + BestLength).join(":");
}

function ClassifyV6(Words: number[]): IpClass {
    const ZeroUpTo = (Count: number) => Words.slice(0, Count).every((Word) => Word === 0);

    if(Words.every((Word) => Word === 0)){
        return "unspecified";
    }
    if(ZeroUpTo(7) && Words[7] === 1){
        return "loopback";
    }
    if((Words[0] & 0xff00) === 0xff00){
        return "multicast";
    }
    if((Words[0] & 0xffc0) === 0xfe80){
        return "link-local";
    }
    if((Words[0] & 0xfe00) === 0xfc00 || (Words[0] & 0xffc0) === 0xfec0){
        return "private";
    }
    // Global unicast is 2000::/3. NAT64 prefixes stand for IPv4 hosts and are never a real peer.
    if((Words[0] & 0xe000) !== 0x2000){
        return "reserved";
    }
    return "public";
}

export function ParseIp(Text: unknown): ParsedIp | undefined {
    if(typeof Text !== "string" || Text.length === 0 || Text.length > 45){
        return undefined;
    }

    const V4 = ParseV4Bytes(Text);
    if(V4 !== undefined){
        return { version: 4, canonical: V4.join("."), class: ClassifyV4(V4), bytes: V4 };
    }

    const Words = ParseV6Words(Text);
    if(Words === undefined){
        return undefined;
    }

    // ::ffff:a.b.c.d is an IPv4 peer seen through a dual-stack socket: treat it as that address.
    if(ZeroWords(Words, 5) && Words[5] === 0xffff){
        const Bytes = [Words[6] >> 8, Words[6] & 0xff, Words[7] >> 8, Words[7] & 0xff];
        return { version: 4, canonical: Bytes.join("."), class: ClassifyV4(Bytes), bytes: Bytes };
    }

    const Bytes: number[] = [];
    for(const Word of Words){
        Bytes.push(Word >> 8, Word & 0xff);
    }
    return { version: 6, canonical: FormatV6(Words), class: ClassifyV6(Words), bytes: Bytes };
}

function ZeroWords(Words: number[], Count: number): boolean {
    return Words.slice(0, Count).every((Word) => Word === 0);
}

// A socket's remote address as the gateway reports it: canonical, IPv4-mapped addresses unwrapped.
export function PeerAddress(Raw: string | undefined): string {
    const Parsed = ParseIp(Raw);
    return Parsed === undefined ? "unknown" : Parsed.canonical;
}

// Rate limits count per IPv4 address, and per /64 for IPv6 (one home or one VPS gets a whole
// /64, so counting single IPv6 addresses would be trivial to dodge).
export function RateLimitKey(Address: string): string {
    const Parsed = ParseIp(Address);
    if(Parsed === undefined){
        return Address;
    }
    if(Parsed.version === 4){
        return Parsed.canonical;
    }
    const Words: number[] = [];
    for(let Index = 0; Index < 8; Index += 2){
        Words.push((Parsed.bytes[Index] << 8) | Parsed.bytes[Index + 1]);
    }
    return FormatV6([...Words, 0, 0, 0, 0]) + "/64";
}

export type AllowlistCheck =
    | { ok: true; ip: ParsedIp }
    | { ok: false; reason: "not_a_string" | "range_not_allowed" | "not_an_ip" | "unspecified" | "multicast" | "reserved" | "private" };

// What may go into the firewall rule: exactly one routable address. Private, loopback and
// link-local addresses only with AllowPrivate (tests and LAN parties).
export function CheckAllowlistIp(Text: unknown, AllowPrivate: boolean): AllowlistCheck {
    if(typeof Text !== "string"){
        return { ok: false, reason: "not_a_string" };
    }
    if(/[\/,\-*]/.test(Text) || /^(any|localsubnet|dns|dhcp|wins|defaultgateway|internet|intranet|localhost)$/i.test(Text)){
        return { ok: false, reason: "range_not_allowed" };
    }
    const Parsed = ParseIp(Text);
    if(Parsed === undefined){
        return { ok: false, reason: "not_an_ip" };
    }
    switch(Parsed.class){
        case "public":
            return { ok: true, ip: Parsed };
        case "private":
        case "loopback":
        case "link-local":
            return AllowPrivate ? { ok: true, ip: Parsed } : { ok: false, reason: "private" };
        case "unspecified":
            return { ok: false, reason: "unspecified" };
        case "multicast":
            return { ok: false, reason: "multicast" };
        case "reserved":
            return { ok: false, reason: "reserved" };
    }
}
