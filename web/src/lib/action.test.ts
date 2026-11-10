import { describe, expect, it } from "vitest";
import type { AppState, Build, Installed, Release } from "../api/types";
import {
  IDLE,
  canReinstall,
  deriveAppButton,
  deriveReinstallButton,
  deriveVersionButton,
  isActiveBuild,
  libraryKey,
  needsPrepare,
  replacesLabel,
  versionChange,
  withAppState,
  withLibraryState,
  type LocalFlow,
} from "./action";

const release = (over: Partial<Release> = {}): Release => ({
  version: "2.0",
  build: "200",
  date: "2026-10-01",
  notes: null,
  size: 1000,
  min_os: "15.0",
  max_os: null,
  compatible: true,
  incompatibility: null,
  ...over,
});

const build = (over: Partial<Build> = {}): Build => ({
  id: "b1",
  source_id: "s",
  bundle_id: "com.example",
  version: "2.0",
  build: "200",
  status: "signing",
  step: "Signing 3 of 9",
  progress: 0.5,
  error: null,
  signed_bundle_id: "com.example.TEAM",
  size: 1000,
  expires_at: null,
  dropped_entitlements: [],
  created_at: "2026-10-29T10:00:00Z",
  finished_at: null,
  install_url: null,
  ...over,
});

const installed = (over: Partial<Installed> = {}): Installed => ({
  version: "1.0",
  build: "100",
  signed_bundle_id: "com.example.TEAM",
  origin: "kiwi",
  verified: false,
  installed_at: "2026-10-01T00:00:00Z",
  expires_at: "2027-10-26T00:00:00Z",
  ...over,
});

const state = (over: Partial<AppState> = {}): AppState => ({
  action: "get",
  update_reason: null,
  installed: null,
  target: release(),
  build: null,
  install_url: null,
  open_url: null,
  replaces: null,
  foreign_install: false,
  ...over,
});

const app = (s: Partial<AppState> = {}, unavailable: string | null = null) => ({ unavailable, state: state(s) });
const local = (over: Partial<LocalFlow> = {}): LocalFlow => ({ ...IDLE, ...over });
const URL = "itms-services://?action=download-manifest&url=https%3A%2F%2Fmac%2Fota%2Ft%2Fmanifest.plist";

describe("deriveAppButton", () => {
  it("maps get, retry and unavailable", () => {
    expect(deriveAppButton(app(), IDLE)).toEqual({ kind: "prepare", label: "Get", caption: null, error: false });
    expect(deriveAppButton(app({ action: "retry", build: build({ status: "failed", error: "boom" }) }), IDLE)).toEqual({
      kind: "prepare",
      label: "Retry",
      caption: "boom",
      error: true,
    });
    expect(deriveAppButton(app({ action: "unavailable" }, "Requires iOS 28.0 or later"), IDLE)).toEqual({
      kind: "disabled",
      label: "Get",
      caption: "Requires iOS 28.0 or later",
    });
  });

  it("shows build progress while preparing", () => {
    expect(deriveAppButton(app({ action: "preparing", build: build() }), IDLE)).toEqual({
      kind: "progress",
      progress: 0.5,
      caption: "Signing 3 of 9",
    });
    expect(deriveAppButton(app({ action: "preparing", build: build({ progress: null, step: null, status: "queued" }) }), IDLE))
      .toMatchObject({ kind: "progress", progress: null, caption: "Waiting" });
    expect(deriveAppButton(app(), local({ busy: true }))).toMatchObject({ kind: "progress", progress: null });
  });

  it("prefers the locally polled build over a stale app state", () => {
    expect(deriveAppButton(app(), local({ build: build({ progress: 0.9, step: "Packaging" }) }))).toMatchObject({
      kind: "progress",
      progress: 0.9,
    });
    expect(deriveAppButton(app(), local({ build: build({ status: "failed", error: "no profile" }) }))).toMatchObject({
      kind: "prepare",
      label: "Retry",
      caption: "no profile",
    });
    expect(deriveAppButton(app(), local({ build: build({ status: "ready", install_url: URL }) }))).toMatchObject({
      kind: "link",
      label: "Install",
      href: URL,
    });
  });

  it("clamps progress into 0..1", () => {
    expect(deriveAppButton(app({ action: "preparing", build: build({ progress: 1.7 }) }), IDLE)).toMatchObject({ progress: 1 });
  });

  it("uses a real link for install and update with install_url", () => {
    expect(deriveAppButton(app({ action: "install", install_url: URL }), IDLE)).toEqual({
      kind: "link",
      label: "Install",
      href: URL,
      caption: null,
    });
    expect(
      deriveAppButton(app({ action: "update", update_reason: "new_version", installed: installed(), install_url: URL }), IDLE),
    ).toMatchObject({ kind: "link", label: "Update", href: URL });
  });

  it("prepares an update without install_url and marks re-signs", () => {
    expect(deriveAppButton(app({ action: "update", update_reason: "new_version", installed: installed() }), IDLE)).toEqual({
      kind: "prepare",
      label: "Update",
      caption: null,
      error: false,
    });
    expect(deriveAppButton(app({ action: "update", update_reason: "resign", installed: installed() }), IDLE)).toMatchObject({
      label: "Update",
      caption: "Re-sign",
    });
  });

  it("shows installing after a tap and while the server reports it", () => {
    expect(deriveAppButton(app({ action: "install", install_url: URL }), local({ watching: true }))).toEqual({
      kind: "installing",
      href: URL,
    });
    expect(deriveAppButton(app({ action: "installing", install_url: URL }), IDLE)).toEqual({ kind: "installing", href: URL });
    // Once installed, the open state wins even if still watching.
    expect(deriveAppButton(app({ action: "open", open_url: "kiwi-abc://" }), local({ watching: true }))).toEqual({
      kind: "open",
      href: "kiwi-abc://",
    });
  });

  it("opens with open_url, otherwise shows a disabled Installed label", () => {
    expect(deriveAppButton(app({ action: "open", open_url: "kiwi-123456789abc://" }), IDLE)).toEqual({
      kind: "open",
      href: "kiwi-123456789abc://",
    });
    expect(deriveAppButton(app({ action: "open" }), IDLE)).toEqual({ kind: "disabled", label: "Installed", caption: null });
  });

  it("surfaces prepare request errors", () => {
    expect(deriveAppButton(app(), local({ error: "Signing unavailable" }))).toEqual({
      kind: "prepare",
      label: "Retry",
      caption: "Signing unavailable",
      error: true,
    });
  });
});

describe("deriveVersionButton", () => {
  const r = release({ version: "1.5", build: "150" });

  it("marks the installed release and incompatible releases", () => {
    expect(deriveVersionButton(r, installed({ version: "1.5", build: "150" }), IDLE)).toMatchObject({ label: "Installed" });
    expect(deriveVersionButton(r, installed({ version: "1.5", build: null }), IDLE)).toMatchObject({ label: "Installed" });
    expect(deriveVersionButton(release({ compatible: false, incompatibility: "Requires iOS 28.0 or later" }), null, IDLE))
      .toEqual({ kind: "disabled", label: "Incompatible", caption: "Requires iOS 28.0 or later" });
  });

  it("walks prepare, progress, install and installing", () => {
    expect(deriveVersionButton(r, null, IDLE)).toMatchObject({ kind: "prepare", label: "Install" });
    expect(deriveVersionButton(r, null, local({ build: build() }))).toMatchObject({ kind: "progress", progress: 0.5 });
    const ready = build({ status: "ready", install_url: URL });
    expect(deriveVersionButton(r, null, local({ build: ready }))).toMatchObject({ kind: "link", href: URL });
    expect(deriveVersionButton(r, null, local({ build: ready, watching: true }))).toEqual({ kind: "installing", href: URL });
    expect(deriveVersionButton(r, null, local({ build: build({ status: "failed", error: "x" }) }))).toMatchObject({
      label: "Retry",
      caption: "x",
    });
  });
});

describe("reinstall", () => {
  it("is offered for installed, up-to-date apps with a release to install", () => {
    const inst = installed({ version: "2.0", build: "200" });
    expect(canReinstall(app({ action: "open", installed: inst }))).toBe(true);
    expect(canReinstall(app({ action: "open", installed: inst, target: null }))).toBe(false);
    expect(canReinstall(app({ action: "open", installed: inst }, "Removed from the source"))).toBe(false);
    expect(canReinstall(app({ action: "update", installed: installed() }))).toBe(false);
    expect(canReinstall(app({ action: "get" }))).toBe(false);
  });

  it("walks preparing, reinstall link, installing and retry", () => {
    expect(deriveReinstallButton(IDLE)).toEqual({ kind: "progress", progress: null, caption: "Preparing" });
    expect(deriveReinstallButton(local({ busy: true }))).toMatchObject({ kind: "progress", progress: null });
    expect(deriveReinstallButton(local({ build: build() }))).toMatchObject({ kind: "progress", progress: 0.5 });
    const ready = build({ status: "ready", install_url: URL });
    expect(deriveReinstallButton(local({ build: ready }))).toEqual({ kind: "link", label: "Reinstall", href: URL, caption: null });
    expect(deriveReinstallButton(local({ build: ready, watching: true }))).toEqual({ kind: "installing", href: URL });
    expect(deriveReinstallButton(local({ build: build({ status: "failed", error: "x" }) }))).toMatchObject({
      kind: "prepare",
      label: "Retry",
      caption: "x",
    });
    // A ready build that expired has no link: prepare a new one.
    expect(deriveReinstallButton(local({ build: build({ status: "ready" }) }))).toMatchObject({ label: "Retry" });
    expect(deriveReinstallButton(local({ error: "Signing is unavailable." }))).toMatchObject({
      label: "Retry",
      caption: "Signing is unavailable.",
    });
  });
});

describe("helpers", () => {
  it("detects active builds", () => {
    expect(isActiveBuild(build({ status: "queued" }))).toBe(true);
    expect(isActiveBuild(build({ status: "ready" }))).toBe(false);
    expect(isActiveBuild(build({ status: "failed" }))).toBe(false);
    expect(isActiveBuild(null)).toBe(false);
  });

  it("keys library rows by signed bundle id", () => {
    // devicectl saw the App Store build and Kiwi's build of the same source app.
    const kiwi = { source_id: "s", bundle_id: "a", state: state({ installed: installed({ signed_bundle_id: "a.TEAM" }) }) };
    const store = {
      source_id: "s",
      bundle_id: "a",
      state: state({ installed: installed({ signed_bundle_id: "a", origin: "device" }) }),
    };
    expect(libraryKey(kiwi as never)).toBe("a.TEAM");
    expect(libraryKey(store as never)).toBe("a");
    const next = withLibraryState([kiwi, store] as never[], kiwi as never, state({ action: "update" })) as unknown as (
      typeof kiwi
    )[];
    expect(next[0]?.state.action).toBe("update");
    expect(next[1]).toBe(store);
  });

  it("labels an install that would replace another source's app", () => {
    expect(replacesLabel(state())).toBeNull();
    expect(replacesLabel(state({ replaces: { name: "YouTubePlus", source_name: "YTLite Repository", version: "21.43.5" } }))).toBe(
      "Replaces YouTubePlus (YTLite Repository)",
    );
  });

  it("replaces one app's state in a list", () => {
    const a = { source_id: "s", bundle_id: "a", state: state() };
    const b = { source_id: "s", bundle_id: "b", state: state() };
    const next = withAppState([a, b] as never[], b as never, state({ action: "open" })) as unknown as (typeof a)[];
    expect(next[0]).toBe(a);
    expect(next[1]?.state.action).toBe("open");
  });
});

describe("versionChange", () => {
  it("distinguishes Kiwi revisions without changing the YouTube version", () => {
    const change = versionChange({
      action: "update",
      installed: installed({ version: "21.43.5", build: "21.43.5", revision: 2 }),
      target: release({ version: "21.43.5", build: "21.43.5", revision: 4 }),
    });
    expect(change).toEqual({ from: "21.43.5 · Kiwi 2", to: "21.43.5 · Kiwi 4" });
    const original = { version: "21.43.5", build: "21.43.5" };
    expect(deriveVersionButton(release({ ...original, revision: 4 }), installed({ ...original, revision: 2 }), IDLE))
      .toMatchObject({ kind: "prepare" });
    expect(deriveVersionButton(release({ ...original, revision: 4 }), installed({ ...original, revision: 4 }), IDLE))
      .toMatchObject({ kind: "disabled", label: "Installed" });
  });
  it("shows installed and target versions for an update in any phase", () => {
    const base = { installed: installed({ version: "1.0", build: "100" }), target: release({ version: "2.0", build: "200" }) };
    for (const action of ["update", "preparing", "install", "installing", "retry"] as const) {
      expect(versionChange({ ...base, action })).toEqual({ from: "1.0", to: "2.0" });
    }
  });

  it("adds builds when the versions are equal", () => {
    const change = versionChange({
      action: "update",
      installed: installed({ version: "2.0", build: "199" }),
      target: release({ version: "2.0", build: "200" }),
    });
    expect(change).toEqual({ from: "2.0 (199)", to: "2.0 (200)" });
  });

  it("has none for re-signs, installed apps and apps that aren't installed", () => {
    const same = { installed: installed({ version: "2.0", build: "200" }), target: release({ version: "2.0", build: "200" }) };
    expect(versionChange({ ...same, action: "update" })).toBeNull();
    expect(versionChange({ action: "open", installed: installed(), target: release() })).toBeNull();
    expect(versionChange({ action: "get", installed: null, target: release() })).toBeNull();
    expect(versionChange({ action: "update", installed: installed(), target: null })).toBeNull();
  });
});

describe("needsPrepare", () => {
  it("is true for updates without a ready build and failed builds", () => {
    expect(needsPrepare({ action: "update", install_url: null })).toBe(true);
    expect(needsPrepare({ action: "retry", install_url: null })).toBe(true);
    expect(needsPrepare({ action: "update", install_url: "itms-services://x" })).toBe(false);
    expect(needsPrepare({ action: "preparing", install_url: null })).toBe(false);
  });
});
