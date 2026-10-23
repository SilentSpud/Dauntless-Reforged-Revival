# Brand

The Dauntless Reforged Revival logo: the name in silver and lime over a spiked green crystal crest. It
was made for this project; it is not Dauntless Revived's or Mystic Paradox's logo, and not Phoenix
Labs' artwork.

| File | What |
|---|---|
| `logo.png` | the logo (square, transparent; the source of everything else) |
| `palette.json` | the colours |
| `build.mjs` | renders the files below from `logo.png` (needs `playwright-core` and a Chromium) |
| `launcher/icon.ico` | Windows icon, 16 to 256 px |
| `launcher/icon.png`, `icon-512.png`, `icon-1024.png` | PNG renders |
| `launcher/logo-384.png`, `logo-64.png` | the launcher page's logo (the rail) and its title-bar mark |

`npm run icon` in `legacy/ReforgedLauncher/` copies the icons and the two logo sizes into the launcher. Same
license as the repository (AGPL-3.0-only).
