import { test } from "node:test";
import assert from "node:assert/strict";
import { News, GitHubNewsUrl } from "../src/news";

const url = "https://raw.githubusercontent.com/Vvoidddd/dauntless-revived/launcher-news/news.json";
const item = (date: string, title: string) => ({ date, title, body: "Public notes only" });

test("GitHub feed sorts newest first, uses ETag, and retains good news on errors", async () => {
    let now = 0, calls = 0;
    let response = new Response(JSON.stringify({items: [item("2026-10-01", "Old"), item("2026-10-02", "New")]}), {headers: {etag: '"v1"'}});
    let headers: HeadersInit | undefined;
    const fetcher = (async (_url, options) => { calls++; headers = options?.headers; return response; }) as typeof fetch;
    const news = new News(undefined, () => now, url, fetcher);
    await news.refreshRemote();
    assert.equal(news.body().items[0].title, "New");
    await news.refreshRemote();
    assert.equal(calls, 1);
    now += 60000;
    response = new Response(null, {status: 304});
    await news.refreshRemote();
    assert.equal(new Headers(headers).get("if-none-match"), '"v1"');
    assert.equal(news.body().items.length, 2);
    for (const bad of [new Response("oops", {status: 503}), new Response("invalid JSON"), new Response(JSON.stringify({items:[{}]})), new Response("x".repeat(1024*1024+1))]) {
        now += 60000; response = bad;
        await news.refreshRemote();
        assert.equal(news.body().items[0].title, "New");
    }
    now += 60000; response = new Response('{"items":[]}');
    await news.refreshRemote();
    assert.deepEqual(news.body().items, []);
});

test("remote news accepts only the curated GitHub file host", () => {
    assert.equal(GitHubNewsUrl(url), url);
    for (const value of ["http://raw.githubusercontent.com/a/b/c/d", "https://example.com/feed", "https://raw.githubusercontent.com@127.0.0.1/a/b/c/d", url + "?token=secret"]) assert.throws(() => GitHubNewsUrl(value));
});

test("concurrent launcher polls share one background download", async () => {
    let finish!: (response: Response) => void, calls = 0;
    const fetcher = (() => { calls++; return new Promise<Response>(resolve => { finish = resolve; }); }) as typeof fetch;
    const news = new News(undefined, () => 0, url, fetcher);
    const pending = news.refreshRemote();
    news.body(); news.body();
    assert.equal(calls, 1);
    finish(new Response('{"items":[]}'));
    await pending;
});
