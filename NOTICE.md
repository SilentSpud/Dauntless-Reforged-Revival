# NOTICE

Dauntless Reforged Revival is licensed as a whole under the GNU Affero General Public License,
version 3 only (`AGPL-3.0-only`); see [LICENSE.txt](LICENSE.txt). Additional terms under AGPLv3
section 7 apply to the Mystic Paradox material only; see [ADDITIONAL_TERMS.md](ADDITIONAL_TERMS.md).
This file records where each part of the repository comes from.

## Modification notice

This repository, [github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival](https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival),
combines and modifies two AGPL-3.0-only projects to run Dauntless 1.14.7 (`rel-1.14.7-Archon`,
changelist 647472) on private servers. Work began on 14 October 2026, and every change is recorded
in the commit history. It is **not** an official release of Dauntless Revived, Mystic Paradox,
Undaunted or any other project named here, and none of them endorses it.

## Where the parts come from

### Dauntless Revived

- By mixutin and the Dauntless Revived contributors (among them anxo0 and Vvoidddd).
- Repository: [github.com/mixutin/dauntless-revived](https://github.com/mixutin/dauntless-revived),
  AGPL-3.0-only. Dauntless Revived is itself a modified version of Undaunted.
- Modified here:
  - `legacy/ReforgedLauncher/` is a modified version of `UndauntedLauncher/`: its design and look (the
    `dauntless-style.css` theme, the steel frames, the PLAY plate, the night scene), its state machine
    and IPC structure, the invite format, the account key store, the pinned TLS connections, the
    local relay, ServerStatus and the saved servers. Changed for 1.14.7: the game is found instead of
    downloaded, the host's game runtime is installed after the player accepts it, the relay always
    runs and answers QoS, the Host tools page, the English and Brazilian Portuguese text, the brand.
  - `legacy/ReforgedGateway/` is a modified version of `UndauntedGateway/` (routing rules for the Mystic
    Paradox backend and its `/__origin/` paths, a plain-HTTP private mode).
  - `legacy/ReforgedContent/` is based on `UndauntedContent/` (the key check, limits, news and art pack;
    it serves the game runtime instead of game files).
  - The Linux server support is adapted from Dauntless Revived's Linux port (`deploy/linux-server/`
    and `deploy/unix-common/`): the Wine/Proton game-server runner on `xvfb-run`
    (`legacy/ReforgedHost/src/wine.ts`), the nftables allowlist table (`legacy/ReforgedGateway/src/allowlist/nft.ts`)
    and the idea of systemd units with one capability each (`legacy/ReforgedHost/src/linux.ts`). Changed for
    1.14.7: Mystic Paradox's `winmm.dll` loader instead of `dxgi.dll`, the table swapped in one
    transaction from a private file (nft no longer reads a pipe), the host kit's own supervisor run by
    one unit, and no root anywhere.
  - `legacy/ReforgedLauncher/src/main/revived/` plays Dauntless Revived's own servers (Dauntless 1.4.4) the
    way their launcher 0.1.35 does ([tag `launcher-v0.1.35`](https://github.com/mixutin/dauntless-revived/releases/tag/launcher-v0.1.35),
    commit `0e2b1e6730b9f14f1020d7c903a6955464fa4586`). Taken over from that release: the game
    manifest check and the list itself (`UndauntedContent/data/dauntless-1.4.4.json`, copied as
    `dauntless-1.4.4.json`), the file checks, the game downloader, the hunt-region choice, the client
    DLL pins and install, and the 1.4.4 command line and config lines. Its two client DLLs
    (`dxgi.dll`, `UndauntedInternalServer.dll`, built from Undaunted's sources) are not in this
    repository: the launcher downloads them from that commit and keeps them only if their SHA-256 is
    the pinned one.
  - `rust/` is a snapshot copy of Dauntless Revived's `rust/` folder at commit
    `e1c5fec`: the cargo workspace files (`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`,
    `deny.toml`, `.cargo/config.toml`, `clippy.toml`, `rustfmt.toml`), the crates `protocol`,
    `proxy`, `service-kit` and `store`, and the fuzz targets `http_body` and `proxy_headers`.
    Changed here: the workspace lists Reforged's members and drops the dependencies only Dauntless
    Revived's `deploy`, `discord-bot` and `metagame` services used. From the copy on, the code is
    this project's own; nothing is taken from Dauntless Revived afterwards.
  - Files and functions taken over say so in their comments.

### Mystic Paradox

- By Pranav Karande (MysticFox).
- Repository: [github.com/pranav158/Mystic-Paradox](https://github.com/pranav158/Mystic-Paradox),
  AGPL-3.0-only with additional terms. Mystic Paradox is itself a modified version of Undaunted.
- **Mystic Paradox's Dauntless 1.12.0 port and related modifications were developed by Pranav
  Karande. See NOTICE.md for contribution and provenance information.** (Mystic Paradox's own
  notice is [paradox/NOTICE.md](paradox/NOTICE.md), its licence [paradox/LICENSE](paradox/LICENSE) and
  its additional terms [paradox/ADDITIONAL_TERMS.md](paradox/ADDITIONAL_TERMS.md).)
- **Forked into this repository with its history.** The fork
  [github.com/SilentSpud/Mystic-Paradox](https://github.com/SilentSpud/Mystic-Paradox) has a `reforged`
  branch: Mystic Paradox's history up to commit `00d3be8aa812e334aaa26e5149d2607acb744360`, then the 33
  Reforged patches as one commit each (`24e60c8dd89c871f273b8f6b8753f2ee334b017f`). That branch is merged
  here as a subtree at [`paradox/`](paradox), so `git log` shows Mystic Paradox's commits with their
  original authors. The backend and Director are in `paradox/backend` and `paradox/director`; the
  runtime (`ParadoxRuntime` upstream), its loader (`tools/RuntimeLoader`) and the CatalogExporter
  (`tools/CatalogExporter`) are in [`native/`](native). Paradox's launcher, deployment scripts and
  self-hosting guide were left out. Every file from Mystic Paradox, and every change made to it here,
  stays under Mystic Paradox's terms.
- The Reforged changes: the runtime reads its backend address from `REFORGED_BACKEND_URL`; the backend
  gains the `/reforged/api` launcher API and an HTTP-only mode behind the gateway; the Director binds to
  one address and lists its instances; and the fixes and features described patch by patch in
  [docs/paradox-patches.md](docs/paradox-patches.md). Runtime changes from Mystic Paradox's own line
  are taken as described in [docs/paradox-runtime-patches.md](docs/paradox-runtime-patches.md).
- The runtime's loader (`native/runtime-loader`, `tools/RuntimeLoader` upstream) is derived from
  [coldloader-proxy](https://github.com/denuvosanctuary/coldloader-proxy); Mystic Paradox's
  README, NOTICE.md and Cargo.toml give its license as Apache-2.0, while the LICENSE file in that
  folder holds the GNU LGPL-3.0 text. The runtime includes MinHook and the Hacker Disassembler
  Engine 64 (BSD-2-Clause).

### Undaunted

- By gwog (Gregory Morford, [SyST3MDeV](https://github.com/SyST3MDeV)), EisigesEis and its other
  contributors. Repository: [github.com/SyST3MDeV/Undaunted](https://github.com/SyST3MDeV/Undaunted),
  AGPL-3.0-only.
- The project both of the above are built on: the server-mode DLL, the deploy server, the
  metagame and the original launcher.

### This project

- `legacy/ReforgedHost/` (the host kit), `rust/`, `brand/` (the logo and icons, made for this project),
  `docs/` and the new parts of the components above, by the Dauntless Reforged Revival
  contributors, AGPL-3.0-only.

### Store captures

- Captures of the live Dauntless store service by EisigesEis:
  [github.com/EisigesEis/DauntlessEndpointDocumentation](https://github.com/EisigesEis/DauntlessEndpointDocumentation),
  published for academic and informational purposes, with no license. Mystic Paradox's store
  generators read five of its files (Lady Luck's rotation, the platinum webstore and dyes, the Hunt
  Pass rank skips and passes). They are **not** copied into this repository:
  [paradox/backend/store-captures.json](paradox/backend/store-captures.json) names the commit and each file's
  SHA-256, and `npm run gamedata` fetches them on the host's own machine.

### Other material

- The software the launcher ships, with its license texts:
  [legacy/ReforgedLauncher/THIRD-PARTY-NOTICES.txt](legacy/ReforgedLauncher/THIRD-PARTY-NOTICES.txt).
- The services' npm dependencies are listed in each `package-lock.json` and keep their own licenses.

## What this repository does not contain

No game files, no generated Unreal Engine SDK, no exported or extracted game data, and no built
runtime. Each host generates the SDK and the game data from their own legally obtained Dauntless
1.14.7 installation, and builds their own runtime ([docs/sdk-and-game-data.md](docs/sdk-and-game-data.md)).
Those files stay in `.workspace/`, which is git-ignored.

"Dauntless" is a trademark of its owners. Dauntless Reforged Revival is an unofficial fan project,
not affiliated with or endorsed by Phoenix Labs, Epic Games or any current or former Dauntless
rights holder.
