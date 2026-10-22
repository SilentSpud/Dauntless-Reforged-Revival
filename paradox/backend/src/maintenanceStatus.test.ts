// [Reforged 0018] The login queue's maintenance status, which the 1.14.7 client needs before it logs in.
import assert from "node:assert/strict";
import http from "node:http";
import test from "node:test";
import express from "express";
import { loginQueueRouter } from "./routes/loginQueue";

function get(port: number, path: string): Promise<{ status: number; type: string; body: string }> {
    return new Promise((resolve, reject) => {
        const req = http.request({ hostname: "127.0.0.1", port, path, method: "GET" }, (res) => {
            let body = "";
            res.setEncoding("utf8");
            res.on("data", (chunk) => { body += chunk; });
            res.on("end", () => resolve({ status: res.statusCode ?? 0, type: String(res.headers["content-type"] ?? ""), body }));
        });
        req.on("error", reject);
        req.end();
    });
}

test("GET /maintenance/status answers the captured shape, without auth: no maintenance", async () => {
    const application = express();
    application.use(loginQueueRouter);
    const server = application.listen(0, "127.0.0.1");
    await new Promise<void>((resolve, reject) => {
        server.once("listening", () => resolve());
        server.once("error", reject);
    });
    const port = (server.address() as import("node:net").AddressInfo).port;
    try {
        const res = await get(port, "/maintenance/status");
        assert.equal(res.status, 200);
        assert.match(res.type, /application\/json/);
        assert.deepEqual(JSON.parse(res.body), { "show-status": false });
    } finally {
        await new Promise<void>((resolve) => server.close(() => resolve()));
    }
});
