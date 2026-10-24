<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: the Director's game-server launch (RX3)

The checklist for the Rust Director, `rust/services/director` (MIGRATION_PLAN.md RB10), the port of Mystic Paradox's
DeployServer (`paradox/director`). It starts a game server process for every hunt a player asks for, so what a
request can put on that process's command line is the boundary. `SECURITY_REVIEW.md` indexes it.

How to use it: before the Rust Director replaces the Node one (RB14 flips `rust_ported` in
`services/host/src/commands/start/binaries.rs`), copy this list into the cutover's review, check each item in the code
(not from memory), link the code or test that shows it, and sign at the end with the date and the commit reviewed. An
item that doesn't hold is a bug to fix before the cutover, not a box to leave open.

## What it is and where the boundary is

The Director has no authentication, as the Node one never had: only the backend calls it, so it must only be
reachable from this machine.

- **In, from the backend:** HTTP on `127.0.0.1:61001`. `POST /api/matchmaker/handle-matchmaking-for-player` with
  `GameMode`, `GameArgs`, `HuntId` and `ExpectedPlayers`, and `POST /api/matchmaker/resolve-hunt-launch-manifest`.
  `GameArgs` is the client's travel URL and `HuntId` the client's hunt id, passed on by the backend: treat both as
  the player's input. `GET /health/ready`, `/api/hub-status` and `/api/instances` read state and change nothing.
- **In, from the host:** `director.env` (written by `reforged-host setup`) and the hunt tables in `game-data/`.
- **In, from the game servers:** each one's stdout (the readiness marker) and the runtime's log beside the game
  executable (who is in the Training Grounds).
- **Out:** game server processes, with the gameserver key (`METAGAME_API_KEY`) as their first argument and the
  Director's whole environment, which holds no other secret (`hostconfig.rs` writes only the key there).

Code: `rust/services/director/src/` (`app.rs` the routes, `matchmaker.rs` and `gameservers.rs` what a request
starts, `hunt.rs` the manifest, `process.rs` and `win_spawn.rs` the process, `cmdline.rs` its Windows command line,
`config.rs`). Tests: `tests/director.rs` runs the API end to end with fake game servers; `tests/hunt.rs` checks the
manifest against the Node resolver's golden digest (`paradox/shared/hunt-manifest/golden-digest.json`).

Fuzz target in `rust/fuzz`:

- `director_launch`: the matchmaking body, hunt tables and travel arguments, and a readiness line. Checks that a
  manifest names a `/Game/` map and a party of one to four. Checks that the Windows command line splits back into
  exactly the arguments built from the travel URL, and that a readiness line counts only for its own launch.

## Checklist

### Who can call it

- [ ] `BIND_HOST` defaults to `127.0.0.1` (`config.rs`), and `reforged-host setup` writes `127.0.0.1`
      (`hostconfig.rs`). Only the gateway listens off loopback.
- [ ] The gateway never forwards to port 61001: it forwards only to `GATEWAY_BACKEND_URL` and
      `GATEWAY_CONTENT_URL`. In Docker the Director publishes no port (`compose.yaml`).
- [ ] Bodies are parsed as Express parses them, capped at 100 kB (`app.rs`, `service_kit::body`). A malformed body is
      400 and starts nothing.

### What a request can start

- [ ] Arguments go to the game server as a list, never through a shell. On Windows each one is quoted for the MSVC
      runtime, so a space or a quote in a map or Behemoth can't make a second argument (`cmdline.rs`; the fuzz
      target's round trip; `quotes_arguments_as_the_msvc_runtime_reads_them`).
- [ ] The program is `GAMESERVER_BINARY_PATH` from the env file, never from a request.
- [ ] A hunt id or party that the tables don't resolve starts nothing (`hunt.rs`, `resolve_manifest`). A party is one
      to four non-empty strings. The matchmaker row, map and Behemoth then come from the host's tables, not from the
      request.
- [ ] The number of hunts is capped (`MAX_CONCURRENT_HUNT_SERVERS`, checked after the stagger so a burst can't race
      past it) and, with `GAMESERVER_MEMORY_GUARD=1`, so is their memory (`memory.rs`). A refused start takes no port
      and no process.
- [ ] Hunt starts are staggered by `SECONDS_TO_WAIT_BETWEEN_GAMESERVER_STARTUP`, and the two hubs never cold-start at
      once (`gameservers.rs`, `start_server`; `hub_gate`).
- [ ] A port is leased to one launch at a time and freed only by its owner (`port_pool.rs`). A late exit or a timed-out
      start can't free a newer hunt's port (`tests/units.rs`).
- [ ] A server is advertised only after it printed the readiness marker with its own launch id and port
      (`process.rs`, `readiness`). A marker for another launch is logged and ignored.
- [ ] A hub's address is handed out only while its process is alive (`hub_connection_details`); a dead hub's never is
      (`a_crashed_ramsgate_is_refused_then_restarted`).

### The processes

- [ ] Both output pipes are read to their end for the whole life of the process, whatever the readiness check does.
      A full pipe once froze a game thread mid-hunt (`process.rs`, `drain`).
- [ ] On Windows a game server inherits only its three standard handles, never the Director's other handles, such as
      the host kit's log pipe (`win_spawn.rs`, PROC_THREAD_ATTRIBUTE_HANDLE_LIST).
- [ ] A game server started hidden on Windows (`GAMESERVER_WINDOWS_HIDE`, default on) opens no window on the host's
      desktop.
- [ ] A stop is SIGTERM on Linux (the game-server launcher passes it to Wine, the game and Xvfb) and
      TerminateProcess on Windows. The host kit's job object or process group ends every game server with the
      Director.
- [ ] The runtime's log is read from where it ended when the hub started, at most 4 MiB at a time, and an unreadable
      or unknown count never stops the Training Grounds (`occupancy.rs`).

### Secrets and logs

- [ ] The gameserver key is never logged: the spawn line logs the label, port, pid and log file, and the game server's
      own log file starts with the map, Behemoth, matchmaker row and party, not the arguments
      (`gameservers.rs`, `start_server`).
- [ ] A client-sent request id is taken only when it matches `[A-Za-z0-9][A-Za-z0-9._:-]{7,95}`; otherwise one is made
      (`app.rs`, `normalize_request_id`).
- [ ] The signed runtime update stays off (`SERVER_RUNTIME_AUTO_UPDATE=false` from setup). When on, it takes only
      HTTPS without credentials, the artifact from the manifest's origin, and installs only a PE file whose size,
      SHA-256 and Ed25519 signature match (`runtime_update.rs`).

Reviewed at commit `________` on `____-__-__` by `________`.
