# Dauntless Reforged Revival Launcher

The Windows launcher for Dauntless Reforged Revival: paste an invite from a host, pick a name, point
it at your own Dauntless 1.14.7 and press PLAY. It is a modified version of the
[Dauntless Revived](https://github.com/mixutin/dauntless-revived) launcher (same look, same way of
joining servers) adapted to Dauntless 1.14.7 and the [Mystic Paradox](https://github.com/pranav158/Mystic-Paradox)
runtime. The interface is in English and Brazilian Portuguese.

Players: see [docs/players.md](../../docs/players.md). Hosts: see [docs/hosting.md](../../docs/hosting.md).

## What it does

1. **Join**: parses a `reforged-revival://join?...` invite (v1 private over Tailscale, v2 public
   over TLS pinned to the certificate fingerprint in the invite) and checks the server's
   `/reforged/api/ServerStatus`.
2. **Register**: `POST /reforged/api/Register` with the invite code; the account key (`RRK_...`) is
   stored with Electron `safeStorage`, never shown, never logged, and only sent to that server.
3. **Game**: finds Dauntless 1.14.7 (Legendary, Heroic, the Epic Games Launcher, or a folder you
   choose) and checks the executable's SHA-256 against the 1.14.7 build (CL 647472). The game is
   never downloaded.
4. **Runtime**: the server's content server lists the host-built `MysticParadox.dll` and `winmm.dll`
   with their sizes and SHA-256. The player is asked once per set of files; the files are checked
   before they replace anything and again before every launch.
5. **Play**: starts a local relay on `127.0.0.1:61100` that carries the game's traffic to the
   server (pinned TLS in public mode, plain HTTP inside the tailnet in private mode) and answers
   the game's QoS ping, writes Engine.ini's chat section, removes the `[OnlineSubsystemPhoenix]`
   endpoints Dauntless Revived's launcher leaves in Game.ini (they point every Dauntless version at
   `127.0.0.1:61000`, and 1.14.7's login then stops at CheckCrossPlayAndCrossProgression; a copy is
   kept as `Game.ini.before-reforged`), and starts the game with the 1.14.7
   command line (`-AUTH_TYPE=exchangecode -AUTH_PASSWORD=<key> ...`) and
   `REFORGED_BACKEND_URL=http://127.0.0.1:61100` for the runtime.

**Dauntless Revived servers** (`dauntless-revived://` invites, Dauntless 1.4.4) are played the way
Dauntless Revived's launcher 0.1.35 plays them (`src/main/revived/`): their `/undaunted/api`, the 1.4.4
game downloaded from the server's content server and checked against the built-in list of files, their
two client DLLs (downloaded from that release, checked by SHA-256) and their command line. See
[docs/players.md](../../docs/players.md#dauntless-revived-servers-dauntless-144).

The **Host** page (Settings > Advanced > Host tools) does a host's work without a terminal, in four
tabs, in the order a new host goes through them: **Requirements** (what this PC needs, detected, with
downloads and install steps), **Build** (the SDK with Dumper-7, the runtime, CatalogExporter,
publishing the runtime), **Game data** (generate it from CatalogExporter's export, save and import
packs) and **Server** (start and stop it, create invites, the test multipliers, update the project
from GitHub with `npm run update`, prepare it with `npm run setup`, its output). With Host tools on and
no project folder chosen, it clones the project into a `Reforged` folder beside the game's folder. It runs ReforgedHost's commands with the
host's own Node.js (`src/main/hostserver.ts`); the server stops when the launcher closes. See
[docs/hosting.md](../../docs/hosting.md) and [docs/sdk-and-game-data.md](../../docs/sdk-and-game-data.md).

## Development

Node.js 20.19 or newer.

    npm ci
    npm run typecheck
    npm test                 # unit and flow tests (fake servers on 127.0.0.1)
    npm run preview          # the page in a browser with a fake main process
    npm start                # the real app (Windows; Electron)
    npm run make             # Squirrel installer and ZIP (Windows)

`npm run preview` serves http://127.0.0.1:5199/ — add `?scene=` (`join`, `register`, `locate`,
`trust`, `noruntime`, `installing`, `ready`, `running`, ...), `&lang=pt-BR`, `&view=host` and so on;
the list is at the top of `scripts/preview.mjs`.

## Releases

As in Dauntless Revived, a launcher update ships by raising `version` in `package.json` (and
`package-lock.json`: `npm version 1.0.2 --no-git-tag-version`) in a pull request. Once it is merged
and every CI check passed on `main`, CI publishes the release `1.0.2` (tag and release created at that
commit, with generated notes) and installed launchers update to it. A version that has a release
already, or is not newer than every release, publishes nothing. The repository variable
`LAUNCHER_AUTO_RELEASE=false` (Settings > Secrets and variables > Actions > Variables) turns this off.

A release can also be published by hand, and then builds the launcher and attaches it to that release
([`.github/workflows/launcher-release.yml`](../../.github/workflows/launcher-release.yml)):
`ReforgedRevivalLauncher-Setup.exe`, a portable ZIP, the Squirrel update files (`RELEASES`, the
`-full.nupkg`) and `SHA256SUMS.txt`. The run takes about 15 minutes (see the Actions tab).

- The tag is the launcher version: `0.2`, `0.2.0` or `v0.2.0` all build 0.2.0. A version with a
  prerelease part (`0.3.0-beta.1`), or a release marked as a prerelease, is a test version.
- Every other release then updates the rolling `launcher-updates` release, which installed
  launchers read (`UPDATE_FEED_URL` in `src/main/constants.ts`) to update themselves within an
  hour. Test versions and versions older than the one it offers never reach it. Each new release
  needs a higher version than the last, or installed launchers do not update.
- The release must be made from a commit that has the workflow. For an older release, or one whose
  run failed: Actions > Launcher release > Run workflow, with the release's tag.

## Layout

| Path | What |
| --- | --- |
| `src/main.ts` | Electron main process: window, IPC with validation, dialogs, deep links |
| `src/preload.ts` | the typed bridge (the page's only way out) |
| `src/main/controller.ts` | the state machine: everything the page shows comes from its Snapshot |
| `src/main/relay.ts`, `pinned.ts`, `http.ts` | the local relay and the pinned TLS connections |
| `src/main/hostapi.ts`, `runtime-files.ts` | server API calls and the runtime install |
| `src/main/game-folder.ts`, `launch.ts`, `engineini.ts` | finding, configuring and starting the game |
| `src/main/hostkit.ts`, `injector.ts` | the Host page's checks and actions |
| `src/main/revived/` | Dauntless 1.4.4 for Dauntless Revived servers, as their launcher 0.1.35 plays it |
| `src/main/hostserver.ts`, `requirements.ts` | the host kit's commands and the server run from the Host page; the Requirements tab's checks |
| `src/shared/` | code shared with the page: types, invite parser, text (`i18n.ts`), UI model |
| `src/renderer/` | the page: plain TypeScript and CSS, Dauntless Revived's design |
| `test/` | node:test tests |
| `preview/` | the fake bridge for `npm run preview` |

## Translations

All text is in `src/shared/i18n.ts`: an English table and a Brazilian Portuguese table with the
same keys and placeholders (a test checks both). Credits are in `src/shared/credits.ts`.

## License

AGPL-3.0-only, like Dauntless Revived and Undaunted. Third-party software and its licenses:
[THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt).
