import { test } from "node:test";
import assert from "node:assert/strict";
import { StreamLimiter } from "../src/limits";

test("per-account limit", () => {
    const L = new StreamLimiter(6, 100);
    const Held = [];
    for(let I = 0; I < 6; I++){
        const Slot = L.tryAcquire("A");
        assert.equal(Slot.ok, true);
        Held.push(Slot);
    }
    assert.deepEqual(L.tryAcquire("A"), { ok: false, reason: "account_limit" });
    assert.equal(L.tryAcquire("B").ok, true, "another account is not affected");
    assert.equal(L.active("A"), 6);

    const First = Held[0];
    if(First.ok){
        First.release();
        First.release(); // twice is harmless
    }
    assert.equal(L.active("A"), 5);
    assert.equal(L.tryAcquire("A").ok, true);
    assert.equal(L.active(), 7);
});

test("server-wide limit", () => {
    const L = new StreamLimiter(6, 3);
    assert.equal(L.tryAcquire("A").ok, true);
    assert.equal(L.tryAcquire("B").ok, true);
    const C = L.tryAcquire("C");
    assert.equal(C.ok, true);
    assert.deepEqual(L.tryAcquire("D"), { ok: false, reason: "total_limit" });
    if(C.ok){
        C.release();
    }
    assert.equal(L.tryAcquire("D").ok, true);
    assert.equal(L.active("C"), 0);
});
