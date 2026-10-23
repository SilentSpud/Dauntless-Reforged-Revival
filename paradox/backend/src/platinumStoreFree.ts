/**
 * [Dauntless Reforged Revival] Platinum offers are free. Every price in Platinum (the webstore, dyes, Hunt Pass
 * rank skips and tracks, the Middleman's cell fusion slots 2 and 3) is listed as 0 by GET /product/skus/public
 * and nothing is debited for it at purchase (controllers/store.ts PurchaseStoreSku). Prices in other currencies
 * (Lady Luck's marks, the Reward Cache's coins) are unchanged. The SKU data keeps its real prices, so
 * PLATINUM_STORE_FREE=0 in the backend's environment brings them back.
 */
export type StorePrice = { currencyId: string; price: number };

export function PlatinumStoreFree(): boolean {
    return process.env.PLATINUM_STORE_FREE !== "0";
}

export function EffectivePrice(Price: StorePrice): number {
    return PlatinumStoreFree() && Price.currencyId.toLowerCase() === "id_currency_platinum" ? 0 : Price.price;
}
