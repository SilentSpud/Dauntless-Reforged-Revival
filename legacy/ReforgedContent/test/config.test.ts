import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { LoadConfig } from "../src/config";

describe("LoadConfig", () => {
    it("has the documented defaults", () => {
        const Config = LoadConfig({});
        assert.equal(Config.port, 61002);
        assert.deepEqual(Config.bindHosts, ["127.0.0.1"]);
        assert.equal(Config.backendUrl, "http://127.0.0.1:61000");
        assert.equal(Config.runtimeDir, undefined);
        assert.equal(Config.maxStreamsPerAccount, 4);
    });

    it("listens on loopback only and checks keys with the backend on this machine only", () => {
        for(const Bind of ["0.0.0.0", "100.101.102.103", "203.0.113.7", "::"]){
            assert.throws(() => LoadConfig({ BIND_HOST: Bind }), /loopback/, Bind);
        }
        assert.deepEqual(LoadConfig({ BIND_HOST: "127.0.0.1,::1" }).bindHosts, ["127.0.0.1", "::1"]);
        assert.throws(() => LoadConfig({ BACKEND_URL: "http://203.0.113.7:61000" }), /this machine/);
        assert.throws(() => LoadConfig({ BACKEND_URL: "https://127.0.0.1:61000" }), /this machine/);
        assert.throws(() => LoadConfig({ PORT: "0" }), /PORT/);
    });
});
