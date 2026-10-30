/// <reference types="vitest/config" />
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { defineConfig, type Plugin } from "vite";
import { mockApi } from "./mock/plugin.ts";

const pkg = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8")) as { version: string };
const mock = process.env.VITE_MOCK === "1";
const apiTarget = process.env.KIWI_API ?? "http://127.0.0.1:8742";

/** Stamps public/sw.js with a hash of the build so each deploy gets a fresh cache. */
function serviceWorkerVersion(): Plugin {
  let outDir = "dist";
  return {
    name: "kiwi-sw-version",
    apply: "build",
    configResolved(config) {
      outDir = config.build.outDir;
    },
    writeBundle(_options, bundle) {
      const hash = createHash("sha256").update(Object.keys(bundle).sort().join("\n")).digest("hex").slice(0, 12);
      const file = join(outDir, "sw.js");
      writeFileSync(file, readFileSync(file, "utf8").replaceAll("__BUILD_ID__", hash));
    },
  };
}

export default defineConfig({
  oxc: { jsx: { runtime: "automatic", importSource: "preact" } },
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  plugins: [mock ? mockApi() : null, serviceWorkerVersion()],
  server: {
    proxy: mock
      ? undefined
      : {
          "/api": { target: apiTarget },
          "/ota": { target: apiTarget },
          "/enroll": { target: apiTarget },
        },
  },
  build: {
    target: "safari16",
    outDir: "dist",
    assetsDir: "assets",
    // Single chunk; every supported Safari has modulepreload.
    modulePreload: { polyfill: false },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts", "mock/**/*.test.ts", "tests/**/*.test.ts"],
  },
});
