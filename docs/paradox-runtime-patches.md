# Taking runtime patches from Freeza's line

Dauntless Reforged Revival forked Mystic Paradox and Freeza's Reforged patches into this repository
([paradox-patches.md](paradox-patches.md)). Until the cut-off date (MIGRATION_PLAN.md, RD16), Freeza's
changes are merged as before: a new patch to the backend, the Director or the runtime becomes a commit to
`paradox/` or `native/`. After the cut-off only two kinds of change are still taken: patches to the game
runtime, which this page covers, and game-data or preset changes. Backend, Director, launcher, gateway,
content and host-kit changes are not ported after the cut-off.

The runtime is taken because it is the one part both lines still share: every player's `MysticParadox.dll`
and `winmm.dll` speak HTTP to whichever backend answers, so a fix to how the game behaves helps a server on
either line.

## Where the runtime is

| Freeza's line | This repository |
| --- | --- |
| `upstream/patches/NNNN-runtime-*.patch`, applied by the host kit to `ParadoxRuntime/` and `tools/RuntimeLoader/` of the pinned Mystic Paradox | Commits to [`native/paradox-runtime`](../native/paradox-runtime) and [`native/runtime-loader`](../native/runtime-loader) |
| `upstream/README.md`, one section per patch | The commit message, and a section in [paradox-patches.md](paradox-patches.md) |

The patches are plain `git diff` output against Paradox's tree, so their paths start with `ParadoxRuntime/`
or `tools/RuntimeLoader/`, and their context lines are those of Freeza's runtime, which is this one plus
whatever runtime patches came after the fork.

## Applying a patch

From the repository root, with the patch saved as `patch.diff`:

1. Turn Paradox's paths into this repository's, then apply it with a three-way merge, so a patch whose
   context moved still applies or stops with conflict markers:

   ```bash
   sed -e 's#\([ab]\)/ParadoxRuntime/#\1/native/paradox-runtime/#g' \
       -e 's#\([ab]\)/tools/RuntimeLoader/#\1/native/runtime-loader/#g' patch.diff > native.diff
   git apply --numstat native.diff   # every path should now start with native/
   git apply --3way native.diff
   ```

   A patch saved on Windows may have CRLF line endings; convert it to LF first (`dos2unix patch.diff`), as
   the host kit did. (Checked on this repository: the 0031 runtime commit, written out with `git show` and
   rewritten this way, reverse-applies cleanly to `native/paradox-runtime`.)
2. If it does not apply, read Freeza's section for the patch and make the change by hand. The runtime here
   can differ from Freeza's after the cut-off only by runtime patches taken or skipped, so a conflict means
   an earlier runtime patch is missing: take that one first.
3. A patch that also changes the backend or the Director (a new environment variable the Director passes
   on, a route the runtime calls) is taken for its runtime part only. Write down what the other part did in
   the commit message; the Rust backend and Director (rows RB and RD in MIGRATION_PLAN.md) decide whether
   they need it.
4. Commit it alone, as `NNNN-runtime-<name>` like the patches before it, with Freeza's section as the
   message body and a line naming the source (`From FreezaMotoqueiro/Dauntless-Reforged-Revival
   upstream/patches/NNNN-runtime-<name>.patch at <commit>`). Add the same section to
   [paradox-patches.md](paradox-patches.md).
5. Build the runtime with a host's SDK (`native\paradox-runtime\_build.bat`, see
   [sdk-and-game-data.md](sdk-and-game-data.md#2-the-runtime)) and play a hunt with it before it is
   released. Hosts rebuild and publish it after `npm run update`, which says so when `native/` changed.

## Keeping track

Freeza numbers the patches in order. The highest number taken is the latest `NNNN-` commit under
`native/` (`git log --oneline -- native/`); a patch is skipped only on purpose, with the reason in the
commit that takes the next one.
