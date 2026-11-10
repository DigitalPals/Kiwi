import { describe, expect, it } from "vitest";
import type { AppState, AppSummary, Installed, Release } from "../api/types";
import { freshApps, freshLabel } from "./fresh";

const NOW = new Date("2026-10-30T12:00:00Z");

const release = (date: string | null): Release => ({
  version: "2.0",
  build: null,
  date,
  notes: null,
  size: null,
  min_os: null,
  max_os: null,
  compatible: true,
  incompatibility: null,
});

const installed: Installed = {
  version: "1.0",
  build: null,
  signed_bundle_id: "com.example.app",
  origin: "kiwi",
  verified: true,
  installed_at: "2026-10-01T00:00:00Z",
  expires_at: "2027-10-01T00:00:00Z",
};

const app = (name: string, date: string | null, state: Partial<AppState> = {}, over: Partial<AppSummary> = {}): AppSummary => ({
  source_id: "s",
  source_name: "Source",
  bundle_id: `com.example.${name}`,
  platform: "ios",
  name,
  developer: null,
  subtitle: null,
  icon_url: null,
  tint_color: null,
  category: null,
  unavailable: null,
  orphaned: false,
  hidden: false,
  is_new: false,
  original: null,
  state: {
    action: "get",
    update_reason: null,
    installed: null,
    target: release(date),
    build: null,
    install_url: null,
    open_url: null,
    replaces: null,
    ...state,
  },
  ...over,
});

describe("freshApps", () => {
  it("lists releases from the last week, updates first, then the newest", () => {
    const apps = [
      app("old", "2026-10-01"),
      app("new", "2026-10-28"),
      app("newer", "2026-10-29T08:00:00Z"),
      app("update", "2026-10-24", { action: "update", update_reason: "new_version", installed }),
      app("undated", null),
    ];
    expect(freshApps(apps, NOW).map((a) => a.name)).toEqual(["update", "newer", "new"]);
  });

  it("keeps an update first while it is preparing", () => {
    const apps = [app("new", "2026-10-29"), app("update", "2026-10-24", { action: "preparing", installed })];
    expect(freshApps(apps, NOW).map((a) => a.name)).toEqual(["update", "new"]);
  });

  it("leaves out what needs nothing: installed, unavailable, hidden and re-signs", () => {
    const apps = [
      app("open", "2026-10-29", { action: "open" }),
      app("unavailable", "2026-10-29", { action: "unavailable" }),
      app("hidden", "2026-10-29", {}, { hidden: true }),
      app("resign", "2026-10-29", { action: "update", update_reason: "resign" }),
      app("installing", "2026-10-29", { action: "installing" }),
    ];
    expect(freshApps(apps, NOW).map((a) => a.name)).toEqual(["installing"]);
  });

  it("shows at most ten", () => {
    const apps = Array.from({ length: 14 }, (_, i) => app(`a${i}`, "2026-10-29"));
    expect(freshApps(apps, NOW)).toHaveLength(10);
  });
});

describe("freshLabel", () => {
  it("says whether an update or install is ready", () => {
    const update: Partial<AppState> = { action: "update", update_reason: "new_version", installed };
    expect(freshLabel(app("a", null, { ...update, install_url: "itms" }))).toBe("Update Ready");
    expect(freshLabel(app("a", null, update))).toBe("New Version");
    expect(freshLabel(app("a", null, { ...update, action: "preparing" }))).toBe("New Version");
    expect(freshLabel(app("a", null, { action: "install", install_url: "itms" }))).toBe("Ready to Install");
    expect(freshLabel(app("a", null))).toBe("New Release");
  });
});
