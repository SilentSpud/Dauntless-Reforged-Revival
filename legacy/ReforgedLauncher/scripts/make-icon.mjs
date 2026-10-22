// Copies the icons and the logo from ../brand (made by brand/build.mjs) into the launcher:
// assets/icon.ico (the exe and installer), assets/icon.png (the window) and the two logo sizes the page uses.
import { copyFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const brand = path.resolve(root, "..", "brand", "launcher");
const page = path.join(root, "src", "renderer", "brand");
copyFileSync(path.join(brand, "icon.ico"), path.join(root, "assets", "icon.ico"));
copyFileSync(path.join(brand, "icon.png"), path.join(root, "assets", "icon.png"));
copyFileSync(path.join(brand, "logo-384.png"), path.join(page, "logo.png"));
copyFileSync(path.join(brand, "logo-64.png"), path.join(page, "logo-64.png"));
console.log("icons and logo copied from brand/");
