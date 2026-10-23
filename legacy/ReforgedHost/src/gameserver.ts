// Linux only: what the Director starts for each game server (through .workspace/bin/reforged-gameserver,
// which setup writes). It runs Dauntless-Win64-Shipping.exe with the Director's arguments through Wine
// or Proton; see wine.ts.

import { runGameServer } from "./wine";

runGameServer(process.argv.slice(2)).then(
    (code) => process.exit(code),
    (e) => {
        process.stderr.write(`reforged-gameserver: ${e instanceof Error ? e.message : String(e)}\n`);
        process.exit(127);
    },
);
