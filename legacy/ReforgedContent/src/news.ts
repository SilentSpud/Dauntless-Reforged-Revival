import fs from "node:fs";
import { logger } from "./log";

// The host's news for the launcher, from CONTENT_NEWS_FILE (optional). The file is JSON, either
// {"items": [...]} or just the array:
//
//   { "items": [ { "date": "2026-09-21", "title": "Friends night on Friday", "body": "Plain text.\nNew lines are kept." } ] }
//
// Bodies are plain text: nothing is interpreted as HTML or Markdown here, and the launcher shows them
// as text. The file is re-read when it changes (checked at most every RECHECK_MS). If an edit breaks
// it, the last good version keeps being served and a warning is logged.

export type NewsItem = {
    date: string;
    title: string;
    body: string;
};

export type NewsBody = {
    items: NewsItem[];
};

const MAX_ITEMS = 50;
const MAX_TITLE = 200;
const MAX_BODY = 10000;
const MAX_FILE_BYTES = 1024 * 1024;
const RECHECK_MS = 5 * 1000;
const REMOTE_RECHECK_MS = 60 * 1000;

export function GitHubNewsUrl(Value: string | undefined): string | undefined {
    if (!Value) return undefined;
    const Url = new URL(Value);
    if (Url.protocol !== "https:" || Url.hostname !== "raw.githubusercontent.com" || Url.port || Url.username || Url.password || Url.search || Url.hash || Url.pathname.split("/").length < 5) {
        throw new Error("CONTENT_NEWS_URL must be a raw.githubusercontent.com HTTPS file URL");
    }
    return Url.href;
}

function CleanText(Value: string, Max: number, KeepNewlines: boolean): string {
    let Text = Value.replace(/\r\n?/g, "\n");
    Text = KeepNewlines ? Text.replace(/[\x00-\x08\x0b-\x1f\x7f]/g, "") : Text.replace(/[\x00-\x1f\x7f]/g, " ");
    return Text.trim().slice(0, Max);
}

// Throws if the document as a whole is unusable; single bad items are skipped with a warning.
export function ParseNews(Raw: unknown): NewsItem[] {
    let List: unknown;
    if(Array.isArray(Raw)){
        List = Raw;
    }
    else if(Raw !== null && typeof Raw === "object" && Array.isArray((Raw as Record<string, unknown>).items)){
        List = (Raw as Record<string, unknown>).items;
    }
    else{
        throw new Error('news file must be {"items": [...]} or an array');
    }

    const Items: NewsItem[] = [];
    for(const Entry of List as unknown[]){
        if(Entry === null || typeof Entry !== "object"){
            logger.warn("news item skipped: not an object", {});
            continue;
        }
        const { date: DateValue, title: Title, body: Body } = Entry as Record<string, unknown>;

        if(typeof DateValue !== "string" || typeof Title !== "string"){
            logger.warn("news item skipped: needs a date and a title", {});
            continue;
        }

        const When = new Date(DateValue);
        if(Number.isNaN(When.getTime())){
            logger.warn("news item skipped: date not understood", { date: DateValue.slice(0, 40) });
            continue;
        }

        const CleanTitle = CleanText(Title, MAX_TITLE, false);
        if(CleanTitle.length === 0){
            logger.warn("news item skipped: empty title", {});
            continue;
        }

        Items.push({
            date: When.toISOString(),
            title: CleanTitle,
            body: typeof Body === "string" ? CleanText(Body, MAX_BODY, true) : "",
        });
    }

    // Newest first; the sort is stable, so same-day items keep the file's order.
    Items.sort((A, B) => (A.date < B.date ? 1 : A.date > B.date ? -1 : 0));
    return Items.slice(0, MAX_ITEMS);
}

export class News {
    private readonly File: string | undefined;
    private readonly Now: () => number;
    private Items: NewsItem[] = [];
    private Signature = "";
    private CheckedAt = -Infinity;
    private RemoteCheckedAt = -Infinity;
    private RemoteBusy = false;
    private RemoteItems: NewsItem[] | undefined;
    private Etag: string | undefined;
    private readonly RemoteUrl: string | undefined;

    constructor(File: string | undefined, Now: () => number = Date.now, RemoteUrl?: string, private readonly Fetch: typeof fetch = fetch){
        this.File = File;
        this.Now = Now;
        this.RemoteUrl = GitHubNewsUrl(RemoteUrl);
    }

    body(): NewsBody {
        this.Refresh();
        void this.refreshRemote();
        return { items: this.RemoteItems ?? this.Items };
    }

    // One bounded background fetch per minute; game downloads and launcher polling never wait on GitHub.
    async refreshRemote(): Promise<void> {
        if (!this.RemoteUrl || this.RemoteBusy || this.Now() - this.RemoteCheckedAt < REMOTE_RECHECK_MS) return;
        this.RemoteBusy = true;
        this.RemoteCheckedAt = this.Now();
        try {
            const Response = await this.Fetch(this.RemoteUrl, {
                redirect: "error", signal: AbortSignal.timeout(5000),
                headers: this.Etag ? { "If-None-Match": this.Etag } : {},
            });
            if (Response.status === 304) return;
            if (!Response.ok || !Response.body) throw new Error(`GitHub news HTTP ${Response.status}`);
            const Reader = Response.body.getReader();
            const Chunks: Uint8Array[] = [];
            let Bytes = 0;
            try {
                while (true) {
                    const { done, value } = await Reader.read();
                    if (done) break;
                    Bytes += value.byteLength;
                    if (Bytes > MAX_FILE_BYTES) throw new Error("GitHub news exceeds 1 MB");
                    Chunks.push(value);
                }
            } finally { await Reader.cancel(); }
            const Raw = JSON.parse(Buffer.concat(Chunks).toString("utf8").replace(/^\uFEFF/, ""));
            const Items = ParseNews(Raw);
            const Original = Array.isArray(Raw) ? Raw : Raw.items;
            if (Original.length && !Items.length) throw new Error("GitHub news contains no usable entries");
            this.RemoteItems = Items;
            this.Etag = Response.headers.get("etag") ?? undefined;
        } catch (error) {
            logger.warn("GitHub news unavailable; keeping last good news", { error: error instanceof Error ? error.message : String(error) });
        } finally { this.RemoteBusy = false; }
    }

    private Refresh(){
        if(this.File === undefined || this.File === ""){
            return;
        }
        if(this.Now() - this.CheckedAt < RECHECK_MS){
            return;
        }
        this.CheckedAt = this.Now();

        let Stat: fs.Stats;
        try{
            Stat = fs.statSync(this.File);
        }
        catch{
            if(this.Signature !== "missing"){
                logger.warn("news file not found; serving no news", { file: this.File });
                this.Signature = "missing";
                this.Items = [];
            }
            return;
        }

        const Signature = `${Stat.size}:${Stat.mtimeMs}`;
        if(Signature === this.Signature){
            return;
        }
        this.Signature = Signature;

        if(!Stat.isFile() || Stat.size > MAX_FILE_BYTES){
            logger.warn("news file skipped: not a regular file or over 1 MB", { file: this.File });
            return;
        }

        try{
            const Parsed: unknown = JSON.parse(fs.readFileSync(this.File, "utf8").replace(/^﻿/, ""));
            this.Items = ParseNews(Parsed);
            logger.info("news loaded", { file: this.File, items: this.Items.length });
        }
        catch(error){
            logger.warn("news file could not be read; keeping the last good version", { file: this.File, error: error instanceof Error ? error.message : String(error) });
        }
    }
}
