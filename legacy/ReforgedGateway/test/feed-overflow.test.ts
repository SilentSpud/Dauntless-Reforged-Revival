import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import { AllowlistFeed } from '../src/feed';

test('allowlist fanout retries a failed worker independently of the healthy main helper', async () => {
    const counts = [0, 0]; let workerStatus = 503;
    const servers = [0, 1].map(index => http.createServer(async (req, res) => {
        assert.equal(req.headers['x-allowlist-secret'], 'test-secret');
        let data = ''; for await (const chunk of req) data += chunk;
        assert.equal(JSON.parse(data).ip, '8.8.8.8'); counts[index]++;
        res.writeHead(index ? workerStatus : 200); res.end();
    }));
    await Promise.all(servers.map(server => new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve))));
    const urls = servers.map(server => `http://127.0.0.1:${(server.address() as any).port}`);
    const feed = new AllowlistFeed({url:urls[0],additionalUrls:[urls[1]],secret:'test-secret',refreshMs:60000,timeoutMs:1000});
    try {
        feed.Report('8.8.8.8'); await feed.Idle();
        workerStatus = 200; feed.Report('8.8.8.8'); await feed.Idle();
        feed.Report('8.8.8.8'); await feed.Idle();
        assert.deepEqual(counts, [1, 2]);
    } finally { feed.Close(); await Promise.all(servers.map(server => new Promise<void>(resolve => server.close(() => resolve())))); }
});
