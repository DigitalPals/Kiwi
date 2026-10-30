// Regenerates docs/screenshots: every screen in light and dark as an iPhone Home Screen app,
// and hero.png from hero.html. Start the demo first (cd web && VITE_MOCK=1 npm run dev),
// then: npm install && npm run shoot. KIWI_DEMO is the demo's address
// [default: http://localhost:5173]; CHROME a Chrome or Chromium binary [default: Google Chrome].
import { chromium } from "playwright-core";
import sharp from "sharp";
import { mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "../../docs/screenshots");
const raw = join(here, "raw");
const demo = process.env.KIWI_DEMO ?? "http://localhost:5173";
const SCREENS = {
  browse: "/",
  app: "/app/ytlite/com.google.ios.youtube",
  apps: "/apps",
  sources: "/sources",
  builds: "/builds",
  build: "/builds/youtube",
  settings: "/settings",
};

const browser = await chromium.launch(
  process.env.CHROME ? { executablePath: process.env.CHROME } : { channel: "chrome" },
);
mkdirSync(raw, { recursive: true });
mkdirSync(out, { recursive: true });
for (const scheme of ["light", "dark"]) {
  const context = await browser.newContext({
    viewport: { width: 393, height: 852 },
    deviceScaleFactor: 3,
    isMobile: true,
    hasTouch: true,
    colorScheme: scheme,
    userAgent:
      "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Mobile/15E148 Safari/604.1",
  });
  // Opened from the Home Screen, like on the phone (no "Add to Home Screen" hint).
  await context.addInitScript(() => Object.defineProperty(Navigator.prototype, "standalone", { get: () => true }));
  const page = await context.newPage();
  for (const [name, path] of Object.entries(SCREENS)) {
    await page.goto(demo + path, { waitUntil: "networkidle" });
    await page.waitForTimeout(1200);
    const file = join(raw, `${name}-${scheme}.png`);
    await page.screenshot({ path: file });
    // 640 px wide is sharp enough for the README's columns; a palette keeps files small.
    await sharp(file).resize({ width: 640 }).png({ palette: true, quality: 85, effort: 10, compressionLevel: 9 })
      .toFile(join(out, `${name}-${scheme}.png`));
    console.log(`${name}-${scheme}.png`);
  }
  await context.close();
}

const page = await browser.newPage({ viewport: { width: 1200, height: 660 }, deviceScaleFactor: 2 });
await page.goto(pathToFileURL(join(here, "hero.html")).href);
await page.waitForLoadState("load");
const hero = await page.locator(".stage").screenshot({ omitBackground: true });
await sharp(hero).png({ palette: true, quality: 90, effort: 10, compressionLevel: 9 }).toFile(join(out, "hero.png"));
console.log("hero.png");
await browser.close();
rmSync(raw, { recursive: true, force: true });
