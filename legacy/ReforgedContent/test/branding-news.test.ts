import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { Branding, CleanAccent, CleanCredit, IsSafeImageName, MagicMatches, ScanBranding } from "../src/branding";
import { News, ParseNews } from "../src/news";
import { SetLogSink } from "../src/log";

// Synthetic image bytes: just the right signature plus filler. No real artwork anywhere in the tests.
const JPEG = Buffer.concat([Buffer.from([0xff, 0xd8, 0xff, 0xe0]), Buffer.alloc(60, 1)]);
const PNG = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), Buffer.alloc(60, 2)]);
const WEBP = Buffer.concat([Buffer.from("RIFF"), Buffer.from([0x40, 0, 0, 0]), Buffer.from("WEBP"), Buffer.alloc(52, 3)]);

function TempDir(){
    return fs.mkdtempSync(path.join(os.tmpdir(), "reforged-content-test-"));
}

function Quiet<T>(Fn: () => T): { result: T; lines: string[] } {
    const Lines: string[] = [];
    SetLogSink((Line) => Lines.push(Line));
    try{
        return { result: Fn(), lines: Lines };
    }
    finally{
        SetLogSink(undefined);
    }
}

test("image names", () => {
    for(const Good of ["a.jpg", "Harbour-Dusk_2.JPEG", "x.png", "bg.v2.webp"]){
        assert.equal(IsSafeImageName(Good), true, Good);
    }
    for(const Bad of ["../a.jpg", "a/b.jpg", "a\\b.jpg", ".a.jpg", "a..jpg", "a.gif", "a.svg", "a.html", "a.jpg.exe", "a b.jpg", "", "branding.json", "x".repeat(100) + ".jpg"]){
        assert.equal(IsSafeImageName(Bad), false, Bad);
    }
});

test("signatures must match the extension", () => {
    assert.equal(MagicMatches(JPEG, "image/jpeg"), true);
    assert.equal(MagicMatches(PNG, "image/png"), true);
    assert.equal(MagicMatches(WEBP, "image/webp"), true);
    assert.equal(MagicMatches(PNG, "image/jpeg"), false);
    assert.equal(MagicMatches(Buffer.from("<html>"), "image/png"), false);
    assert.equal(MagicMatches(Buffer.alloc(0), "image/webp"), false);
});

test("accent and credit cleaning", () => {
    assert.equal(CleanAccent("#C8A24A"), "#c8a24a");
    assert.equal(CleanAccent(" #abc "), "#abc");
    for(const Bad of ["red", "#abcd", "#12345g", "url(x)", 5, null]){
        assert.equal(CleanAccent(Bad), null);
    }
    assert.equal(CleanCredit("  Photo\nby   a friend "), "Photo by a friend");
    assert.equal(CleanCredit(""), null);
    assert.equal(CleanCredit(7), null);
    assert.equal(CleanCredit("x".repeat(500))?.length, 200);
});

test("no branding folder means no art", () => {
    const B = new Branding(undefined);
    assert.deepEqual(B.body(), { backgrounds: [], accent: null });
    assert.equal(B.image("a.jpg"), undefined);
});

test("without branding.json every valid image is listed in name order", () => {
    const Dir = TempDir();
    try{
        fs.writeFileSync(path.join(Dir, "b.png"), PNG);
        fs.writeFileSync(path.join(Dir, "a.jpg"), JPEG);
        fs.writeFileSync(path.join(Dir, "c.webp"), WEBP);
        fs.writeFileSync(path.join(Dir, "fake.jpg"), Buffer.from("<script>alert(1)</script>"));
        fs.writeFileSync(path.join(Dir, "notes.txt"), "hello");
        fs.writeFileSync(path.join(Dir, "empty.png"), Buffer.alloc(0));
        fs.mkdirSync(path.join(Dir, "sub.jpg"));

        const { result } = Quiet(() => ScanBranding(Dir));
        assert.deepEqual([...result.images.keys()], ["a.jpg", "b.png", "c.webp"]);
        assert.equal(result.images.get("b.png")?.contentType, "image/png");
        assert.equal(result.accent, null);

        const B = new Branding(Dir);
        const Body = Quiet(() => B.body()).result;
        assert.deepEqual(Body, {
            backgrounds: [
                { url: "/content/v1/branding/a.jpg", credit: null },
                { url: "/content/v1/branding/b.png", credit: null },
                { url: "/content/v1/branding/c.webp", credit: null },
            ],
            accent: null,
        });
        assert.equal(B.image("a.jpg")?.fullPath, path.join(Dir, "a.jpg"));
        assert.equal(B.image("fake.jpg"), undefined);
        assert.equal(B.image("notes.txt"), undefined);
        assert.equal(B.image("../a.jpg"), undefined);
    }
    finally{
        fs.rmSync(Dir, { recursive: true, force: true });
    }
});

test("branding.json sets order, credits and the accent, and can't reach outside the folder", () => {
    const Dir = TempDir();
    try{
        fs.writeFileSync(path.join(Dir, "a.jpg"), JPEG);
        fs.writeFileSync(path.join(Dir, "b.png"), PNG);
        fs.writeFileSync(path.join(Dir, "branding.json"), JSON.stringify({
            accent: "#D4A24C",
            backgrounds: [
                { file: "b.png", credit: "Photo by a friend" },
                "a.jpg",
                { file: "../../secret.jpg", credit: "nope" },
                { file: "missing.jpg" },
                "a.jpg",
            ],
        }));
        const { result, lines } = Quiet(() => ScanBranding(Dir));
        assert.deepEqual([...result.images.values()].map((I) => [I.file, I.credit]), [["b.png", "Photo by a friend"], ["a.jpg", null]]);
        assert.equal(result.accent, "#d4a24c");
        assert.ok(lines.length >= 3, "skipped entries are logged");
    }
    finally{
        fs.rmSync(Dir, { recursive: true, force: true });
    }
});

test("a broken branding.json or a missing folder never throws", () => {
    const Dir = TempDir();
    try{
        fs.writeFileSync(path.join(Dir, "a.jpg"), JPEG);
        fs.writeFileSync(path.join(Dir, "branding.json"), "{ nope");
        const { result } = Quiet(() => ScanBranding(Dir));
        assert.deepEqual([...result.images.keys()], ["a.jpg"]);
        const Missing = Quiet(() => ScanBranding(path.join(Dir, "does-not-exist"))).result;
        assert.equal(Missing.images.size, 0);
    }
    finally{
        fs.rmSync(Dir, { recursive: true, force: true });
    }
});

test("the folder is re-read after the rescan interval", () => {
    const Dir = TempDir();
    try{
        let Now = 0;
        const B = new Branding(Dir, () => Now);
        assert.equal(B.body().backgrounds.length, 0);
        fs.writeFileSync(path.join(Dir, "a.jpg"), JPEG);
        Now += 5_000;
        assert.equal(B.body().backgrounds.length, 0, "cached");
        Now += 5_000;
        assert.equal(B.body().backgrounds.length, 1, "rescanned");
    }
    finally{
        fs.rmSync(Dir, { recursive: true, force: true });
    }
});

test("news parsing: validation, cleaning and newest first", () => {
    const { result } = Quiet(() => ParseNews({
        items: [
            { date: "2026-09-01", title: "Older", body: "line one\r\nline two" },
            { date: "2026-09-21T18:00:00Z", title: "  Newest\n title ", body: "<b>not html</b>" },
            { date: "not a date", title: "skipped" },
            { title: "no date" },
            { date: "2026-09-10", title: "   " },
            "not an object",
            { date: "2026-09-10", title: "No body" },
        ],
    }));
    assert.deepEqual(result, [
        { date: "2026-09-21T18:00:00.000Z", title: "Newest  title", body: "<b>not html</b>" },
        { date: "2026-09-10T00:00:00.000Z", title: "No body", body: "" },
        { date: "2026-09-01T00:00:00.000Z", title: "Older", body: "line one\nline two" },
    ]);
    assert.deepEqual(Quiet(() => ParseNews([{ date: "2026-01-01", title: "bare array" }])).result.length, 1);
    assert.throws(() => ParseNews({ news: [] }));
    assert.throws(() => ParseNews("x"));
});

test("news file: none, missing, reload on change, last good kept on a broken edit", () => {
    assert.deepEqual(new News(undefined).body(), { items: [] });

    const Dir = TempDir();
    try{
        const File = path.join(Dir, "news.json");
        let Now = 0;
        const N = new News(File, () => Now);
        assert.deepEqual(Quiet(() => N.body()).result, { items: [] }, "missing file");

        fs.writeFileSync(File, JSON.stringify({ items: [{ date: "2026-09-21", title: "First" }] }));
        Now += 6_000;
        assert.equal(Quiet(() => N.body()).result.items[0]?.title, "First");

        fs.writeFileSync(File, "{ broken json");
        Now += 6_000;
        assert.equal(Quiet(() => N.body()).result.items[0]?.title, "First", "last good version kept");

        fs.writeFileSync(File, JSON.stringify([{ date: "2026-09-22", title: "Second" }, { date: "2026-09-21", title: "First" }]));
        Now += 6_000;
        assert.deepEqual(Quiet(() => N.body()).result.items.map((I) => I.title), ["Second", "First"]);
    }
    finally{
        fs.rmSync(Dir, { recursive: true, force: true });
    }
});
