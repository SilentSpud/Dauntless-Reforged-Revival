import { test } from "node:test";
import assert from "node:assert/strict";
import { EffectivePrice } from "./platinumStoreFree";

function WithEnv(Value: string | undefined, Run: () => void) {
    const Before = process.env.PLATINUM_STORE_FREE;
    if (Value === undefined) delete process.env.PLATINUM_STORE_FREE;
    else process.env.PLATINUM_STORE_FREE = Value;
    try { Run(); } finally {
        if (Before === undefined) delete process.env.PLATINUM_STORE_FREE;
        else process.env.PLATINUM_STORE_FREE = Before;
    }
}

test("Platinum prices are free by default", () => {
    WithEnv(undefined, () => {
        assert.equal(EffectivePrice({ currencyId: "id_currency_platinum", price: 1000 }), 0);
        assert.equal(EffectivePrice({ currencyId: "ID_CURRENCY_PLATINUM", price: 250 }), 0);
    });
});

test("prices in other currencies are unchanged", () => {
    WithEnv(undefined, () => {
        assert.equal(EffectivePrice({ currencyId: "id_currency_marks_steel", price: 300 }), 300);
        assert.equal(EffectivePrice({ currencyId: "id_currency_seasonal_coin", price: 40 }), 40);
    });
});

test("PLATINUM_STORE_FREE=0 brings the Platinum prices back", () => {
    WithEnv("0", () => {
        assert.equal(EffectivePrice({ currencyId: "id_currency_platinum", price: 1000 }), 1000);
    });
});
