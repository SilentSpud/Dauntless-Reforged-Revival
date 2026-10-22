import type { ForgeConfig } from "@electron-forge/shared-types";
import { MakerSquirrel } from "@electron-forge/maker-squirrel";
import { MakerZIP } from "@electron-forge/maker-zip";
import { VitePlugin } from "@electron-forge/plugin-vite";
import { FusesPlugin } from "@electron-forge/plugin-fuses";
import { FuseV1Options, FuseVersion } from "@electron/fuses";

// Windows only: Dauntless 1.14.7 and its runtime are Windows programs. Squirrel installer plus a
// portable ZIP, as in Dauntless Revived.
const config: ForgeConfig = {
  packagerConfig: {
    asar: true,
    name: "Dauntless Reforged Revival Launcher",
    executableName: "ReforgedRevivalLauncher",
    appBundleId: "io.github.freezamotoqueiro.reforgedrevival",
    appCopyright: "Dauntless Reforged Revival contributors. Free software under the GNU AGPL-3.0.",
    // assets/icon.ico (the exe and the installer) and assets/icon.png (the window) are copies of the
    // brand icons in ../brand/launcher/, made by npm run icon.
    icon: "assets/icon",
    // The window icon and the license texts of the third-party software the launcher includes. The
    // game runtime is not shipped: each server's host builds and publishes their own.
    extraResource: ["assets/icon.png", "THIRD-PARTY-NOTICES.txt"],
    protocols: [{ name: "Dauntless Reforged Revival invite", schemes: ["reforged-revival"] }],
    win32metadata: {
      CompanyName: "Dauntless Reforged Revival",
      FileDescription: "Dauntless Reforged Revival Launcher",
      ProductName: "Dauntless Reforged Revival Launcher",
      InternalName: "ReforgedRevivalLauncher",
      OriginalFilename: "ReforgedRevivalLauncher.exe",
    },
  },
  rebuildConfig: {
    onlyModules: [],
  },
  makers: [
    new MakerSquirrel(
      {
        name: "ReforgedRevivalLauncher",
        authors: "Dauntless Reforged Revival contributors",
        description: "Dauntless Reforged Revival Launcher",
        setupExe: "ReforgedRevivalLauncher-Setup.exe",
        setupIcon: "assets/icon.ico",
        iconUrl: "https://raw.githubusercontent.com/FreezaMotoqueiro/Dauntless-Reforged-Revival/main/ReforgedLauncher/assets/icon.ico",
        noMsi: true,
      },
      ["win32"],
    ),
    new MakerZIP({}, ["win32"]),
  ],
  plugins: [
    new VitePlugin({
      build: [
        { entry: "src/main.ts", config: "vite.main.config.ts", target: "main" },
        { entry: "src/preload.ts", config: "vite.preload.config.ts", target: "preload" },
      ],
      renderer: [{ name: "main_window", config: "vite.renderer.config.ts" }],
    }),
    // Switched off at package time: running the exe as plain Node, NODE_OPTIONS, the inspector
    // flags, and loading app code from anywhere but the (integrity-checked) asar.
    new FusesPlugin({
      version: FuseVersion.V1,
      [FuseV1Options.RunAsNode]: false,
      [FuseV1Options.EnableCookieEncryption]: true,
      [FuseV1Options.EnableNodeOptionsEnvironmentVariable]: false,
      [FuseV1Options.EnableNodeCliInspectArguments]: false,
      [FuseV1Options.EnableEmbeddedAsarIntegrityValidation]: true,
      [FuseV1Options.OnlyLoadAppFromAsar]: true,
    }),
  ],
};

export default config;
