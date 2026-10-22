<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: `reforged-host start` (RX3)

The checklist for `start` in `rust/services/host` (MIGRATION_PLAN.md RH2, RD3, RD23): the supervisor that runs every
server process. `SECURITY_REVIEW.md` indexes it.

How to use it: before the host kit's cutover, copy this list into the cutover PR, check each item in the code (not
from memory), link the code or test that shows it, and sign at the end with the date and the commit reviewed.

## What it is and where the boundary is

`start` runs as the host's user (administrator on Windows in public mode, for the allowlist helper; the `reforged`
systemd unit on Linux). It starts MongoDB when managed, the backend and the Director (Node), the content server, the
allowlist helper (public) and the gateway, each with its env file, and stops them all.

- **In:** `host.json` and the `.env` files under `.workspace/config` (trusted: whoever can write them chooses what
  runs), the process environment, and with `REFORGED_CONTROL=stdin` the launcher's stdin pipe.
- **Out:** child processes (never through a shell), their logs under `.workspace/logs`, and the console.

Code: `src/commands/start/` (`mod.rs`, `control.rs`, `binaries.rs`), `src/services.rs`, `src/services/tree.rs`,
`src/run.rs`. Tests: the unit tests there and `tests/start.rs`.

No fuzz target: the only input from outside the host's own files is the stdin control pipe, which is read line by
line and compared with one word.

## Checklist

- [ ] Every program is started with an argument list, never a shell string (`services.rs` `launch`, `run.rs`); `npm`
      and `npx` on Windows are the only `.cmd` files, with fixed arguments.
- [ ] A service's environment is the system's variables, then its env file: no `REFORGED_`, `MYSTICPARADOX_`,
      `GATEWAY_`, `ALLOWLIST_`, `MONGODB_` or other server setting left in the shell reaches it
      (`service_environment`, tested).
- [ ] A Rust service gets its env file with `--env-file` and nothing else configures it (DR X7, `binaries.rs`).
- [ ] The stdin pipe can only stop the server: a whole `stop` line or the pipe closing; nothing else read from it is
      acted on (`control.rs`, tested).
- [ ] Each service runs in its own process tree: a job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` on Windows,
      a process group on Linux (`tree.rs`). Stopping ends the whole tree, the Director's game servers included; on
      Windows a hard kill of `start` ends every tree too (`tests/start.rs`).
- [ ] Children have no console window and no share in the host's console, so Ctrl+C reaches `start` only and the
      services stop in order, the gateway first (`tree.rs` `configure`).
- [ ] A port already held by another program stops the start before that service is started, so nothing else
      answers players in its place (`start_services`, tested).
- [ ] In public mode the game ports are closed with the allowlist helper's `--close-ports` after every stop and,
      on Linux, before the first game server starts.
- [ ] Logs hold the services' own output only; `start` prints no secret from the env files.

Signed: ______ (date, commit)
