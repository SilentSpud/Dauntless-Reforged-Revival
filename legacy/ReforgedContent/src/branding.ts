import fs from "node:fs";
import path from "node:path";
import { logger } from "./log";

// The host's optional "art pack": background images for the launcher and an accent colour, from a
// folder the host controls (CONTENT_BRANDING_DIR). The repository ships none. The folder holds
// jpg/png/webp files and, optionally, branding.json:
//
//   {
//     "accent": "#c8a24a",
//     "backgrounds": [
//       { "file": "harbour-dusk.jpg", "credit": "Photo by a friend" },
//       "second-image.webp"
//     ]
//   }
//
// Without "backgrounds", every image in the folder is used in name order. The folder is re-read at
// most every RESCAN_MS, so the host can drop images in without a restart.

export const BRANDING_PREFIX = "/content/v1/branding/";

const IMAGE_NAME = /^[A-Za-z0-9][A-Za-z0-9_-]*(?:\.[A-Za-z0-9_-]+)*\.(jpe?g|png|webp)$/i;
const ACCENT = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;
const MAX_IMAGE_BYTES = 25 * 1024 * 1024;
const MAX_BACKGROUNDS = 50;
const MAX_CREDIT = 200;
const RESCAN_MS = 10 * 1000;

export type BrandingImage = {
    file: string;
    credit: string | null;
    contentType: string;
    size: number;
    mtimeMs: number;
};

export type BrandingState = {
    accent: string | null;
    images: Map<string, BrandingImage>; // insertion order is display order
};

export type BrandingBody = {
    backgrounds: { url: string; credit: string | null }[];
    accent: string | null;
};

export function IsSafeImageName(Name: unknown): Name is string {
    return typeof Name === "string" && Name.length <= 100 && IMAGE_NAME.test(Name);
}

export function ContentTypeFor(Name: string): string {
    const Ext = Name.slice(Name.lastIndexOf(".") + 1).toLowerCase();
    if(Ext === "png"){
        return "image/png";
    }
    if(Ext === "webp"){
        return "image/webp";
    }
    return "image/jpeg";
}

// The first bytes must match the extension, so a renamed file of any other kind is never served.
export function MagicMatches(Head: Buffer, ContentType: string): boolean {
    if(ContentType === "image/jpeg"){
        return Head.length >= 3 && Head[0] === 0xff && Head[1] === 0xd8 && Head[2] === 0xff;
    }
    if(ContentType === "image/png"){
        return Head.length >= 8 && Head.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]));
    }
    if(ContentType === "image/webp"){
        return Head.length >= 12 && Head.toString("latin1", 0, 4) === "RIFF" && Head.toString("latin1", 8, 12) === "WEBP";
    }
    return false;
}

export function CleanCredit(Value: unknown): string | null {
    if(typeof Value !== "string"){
        return null;
    }
    const Cleaned = Value.replace(/[\x00-\x1f\x7f]/g, " ").replace(/\s+/g, " ").trim().slice(0, MAX_CREDIT);
    return Cleaned.length > 0 ? Cleaned : null;
}

export function CleanAccent(Value: unknown): string | null {
    return typeof Value === "string" && ACCENT.test(Value.trim()) ? Value.trim().toLowerCase() : null;
}

function ReadHead(File: string): Buffer {
    const Fd = fs.openSync(File, "r");
    try{
        const Head = Buffer.alloc(12);
        const Read = fs.readSync(Fd, Head, 0, 12, 0);
        return Head.subarray(0, Read);
    }
    finally{
        fs.closeSync(Fd);
    }
}

// Reads the folder once. Never throws: a broken art pack means no art, not a dead server.
export function ScanBranding(Dir: string | undefined): BrandingState {
    const Empty: BrandingState = { accent: null, images: new Map() };

    if(Dir === undefined || Dir === ""){
        return Empty;
    }

    let Names: string[];
    try{
        Names = fs.readdirSync(Dir);
    }
    catch(error){
        logger.warn("branding folder unreadable", { dir: Dir, error: error instanceof Error ? error.message : String(error) });
        return Empty;
    }

    let Config: Record<string, unknown> = {};
    const ConfigPath = path.join(Dir, "branding.json");
    if(fs.existsSync(ConfigPath)){
        try{
            const Parsed: unknown = JSON.parse(fs.readFileSync(ConfigPath, "utf8").replace(/^﻿/, ""));
            if(Parsed !== null && typeof Parsed === "object" && !Array.isArray(Parsed)){
                Config = Parsed as Record<string, unknown>;
            }
            else{
                logger.warn("branding.json is not an object; ignoring it", { dir: Dir });
            }
        }
        catch(error){
            logger.warn("branding.json is not valid JSON; ignoring it", { dir: Dir, error: error instanceof Error ? error.message : String(error) });
        }
    }

    let Wanted: { file: unknown; credit: unknown }[];
    if(Array.isArray(Config.backgrounds)){
        Wanted = Config.backgrounds.map((Item: unknown) =>
            typeof Item === "string" ? { file: Item, credit: null } :
            (Item !== null && typeof Item === "object") ? { file: (Item as Record<string, unknown>).file, credit: (Item as Record<string, unknown>).credit } :
            { file: undefined, credit: null });
    }
    else{
        Wanted = Names.filter((Name) => IsSafeImageName(Name)).sort().map((Name) => ({ file: Name, credit: null }));
    }

    const Images = new Map<string, BrandingImage>();
    for(const Item of Wanted){
        if(Images.size >= MAX_BACKGROUNDS){
            break;
        }

        if(!IsSafeImageName(Item.file) || !Names.includes(Item.file) || Images.has(Item.file)){
            logger.warn("branding image skipped (bad name, missing or listed twice)", { file: String(Item.file).slice(0, 120) });
            continue;
        }

        const Full = path.join(Dir, Item.file);
        try{
            const Stat = fs.lstatSync(Full);
            if(!Stat.isFile() || Stat.size === 0 || Stat.size > MAX_IMAGE_BYTES){
                logger.warn("branding image skipped (not a regular file, empty or over 25 MB)", { file: Item.file });
                continue;
            }

            const ContentType = ContentTypeFor(Item.file);
            if(!MagicMatches(ReadHead(Full), ContentType)){
                logger.warn("branding image skipped (contents don't match the extension)", { file: Item.file });
                continue;
            }

            Images.set(Item.file, { file: Item.file, credit: CleanCredit(Item.credit), contentType: ContentType, size: Stat.size, mtimeMs: Stat.mtimeMs });
        }
        catch(error){
            logger.warn("branding image skipped (unreadable)", { file: Item.file, error: error instanceof Error ? error.message : String(error) });
        }
    }

    if(Config.accent !== undefined && CleanAccent(Config.accent) === null){
        logger.warn("branding.json accent ignored: use #rgb or #rrggbb", {});
    }

    return { accent: CleanAccent(Config.accent), images: Images };
}

export function BrandingToBody(State: BrandingState): BrandingBody {
    return {
        backgrounds: [...State.images.values()].map((Image) => ({ url: BRANDING_PREFIX + Image.file, credit: Image.credit })),
        accent: State.accent,
    };
}

// Re-reads the folder at most every RESCAN_MS.
export class Branding {
    private readonly Dir: string | undefined;
    private readonly Now: () => number;
    private State: BrandingState = { accent: null, images: new Map() };
    private ScannedAt = -Infinity;

    constructor(Dir: string | undefined, Now: () => number = Date.now){
        this.Dir = Dir;
        this.Now = Now;
    }

    get dir(): string | undefined {
        return this.Dir;
    }

    current(): BrandingState {
        if(this.Now() - this.ScannedAt >= RESCAN_MS){
            this.State = ScanBranding(this.Dir);
            this.ScannedAt = this.Now();
        }
        return this.State;
    }

    body(): BrandingBody {
        return BrandingToBody(this.current());
    }

    // Exact name lookup against the current scan; undefined for anything not listed.
    image(Name: string): { image: BrandingImage; fullPath: string } | undefined {
        if(this.Dir === undefined || !IsSafeImageName(Name)){
            return undefined;
        }
        const Image = this.current().images.get(Name);
        if(Image === undefined){
            return undefined;
        }
        return { image: Image, fullPath: path.join(this.Dir, Image.file) };
    }
}
