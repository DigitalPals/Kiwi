import { describe, expect, it } from "vitest";
import type { Builder, BuilderInfo, BuilderTask } from "../api/types";
import {
  builderState,
  builderStatus,
  checkView,
  countdown,
  iosBuilds,
  pairingStep,
  rowValue,
  statusDetail,
  statusText,
  taskLine,
  UPDATE_COMMAND,
  watchPairing,
} from "./builder";

const NOW = Date.parse("2026-11-11T12:00:00Z");
const at = (ms: number) => new Date(NOW + ms).toISOString();

const builder = (over: Partial<Builder> = {}): Builder => ({
  name: "Mac Studio",
  paired_at: "2026-11-08T09:00:00Z",
  last_seen: at(-4000),
  online: true,
  compatible: true,
  version: "0.9.2",
  os: "macOS 26.1 (arm64)",
  checks: [{ name: "Mac builder", ready: true }, { name: "Mac build tools", ready: true }],
  task: null,
  ...over,
});

const pairing = (over: Partial<NonNullable<BuilderInfo["pairing"]>> = {}) => ({
  code: "K7QF-9XMD-2PLA",
  expires_at: at(10 * 60_000),
  command: "./install.sh --builder --server https://kiwi.example.ts.net --pair K7QF-9XMD-2PLA",
  ...over,
});

const info = (over: Partial<BuilderInfo> = {}): BuilderInfo => ({ supported: true, reason: null, builder: null, pairing: null, ...over });

const task = (over: Partial<BuilderTask> = {}): BuilderTask => ({
  job: "6f1c0b9e2a4d4c7f8e3b1a20", app: "ios-youtube", name: "YouTube", command: "build",
  stage: "Decrypting YouTube", started_at: at(-60_000), ...over,
});

describe("builderState", () => {
  it("maps the answer to the screen's state", () => {
    expect(builderState(info({ supported: false, reason: "This Kiwi runs on a Mac and builds iOS apps itself." }), NOW)).toBe("unsupported");
    expect(builderState(info(), NOW)).toBe("unpaired");
    expect(builderState(info({ pairing: pairing() }), NOW)).toBe("pairing");
    expect(builderState(info({ builder: builder() }), NOW)).toBe("paired");
  });

  it("shows a code that replaces the paired Mac, but not an expired one", () => {
    expect(builderState(info({ builder: builder(), pairing: pairing() }), NOW)).toBe("pairing");
    expect(builderState(info({ builder: builder(), pairing: pairing({ expires_at: at(0) }) }), NOW)).toBe("paired");
    expect(builderState(info({ pairing: pairing({ expires_at: at(-1) }) }), NOW)).toBe("unpaired");
    expect(builderState(info({ pairing: pairing({ expires_at: "soon" }) }), NOW)).toBe("unpaired");
  });
});

describe("countdown", () => {
  it("counts minutes and seconds down to the code's expiry", () => {
    expect(countdown(at(10 * 60_000), NOW)).toEqual({ seconds: 600, label: "10:00", expired: false });
    expect(countdown(at(9 * 60_000 + 41_000), NOW).label).toBe("9:41");
    expect(countdown(at(5000), NOW).label).toBe("0:05");
  });

  it("rounds up, so 0:00 means expired", () => {
    expect(countdown(at(400), NOW)).toEqual({ seconds: 1, label: "0:01", expired: false });
    expect(countdown(at(0), NOW)).toEqual({ seconds: 0, label: "0:00", expired: true });
    expect(countdown(at(-90_000), NOW)).toEqual({ seconds: 0, label: "0:00", expired: true });
    expect(countdown("not a date", NOW).expired).toBe(true);
  });
});

describe("pairing polls", () => {
  it("keeps polling while the code is out and unused", () => {
    const start = info({ pairing: pairing() });
    const watch = watchPairing(start)!;
    expect(watch).toEqual({ code: "K7QF-9XMD-2PLA", expires_at: at(10 * 60_000), previous: null });
    expect(pairingStep(watch, start, NOW + 3000)).toBe("poll");
    expect(watchPairing(info())).toBeNull();
  });

  it("stops when a Mac has paired", () => {
    const watch = watchPairing(info({ pairing: pairing() }))!;
    expect(pairingStep(watch, info({ builder: builder({ paired_at: at(0) }) }), NOW + 3000)).toBe("paired");
    // Even if the server still lists the code.
    expect(pairingStep(watch, info({ builder: builder(), pairing: pairing() }), NOW + 3000)).toBe("paired");
  });

  it("waits for a new Mac when one is already paired", () => {
    const old = builder();
    const watch = watchPairing(info({ builder: old, pairing: pairing() }))!;
    expect(watch.previous).toBe(old.paired_at);
    expect(pairingStep(watch, info({ builder: old, pairing: pairing() }), NOW + 3000)).toBe("poll");
    expect(pairingStep(watch, info({ builder: builder({ name: "MacBook Pro", paired_at: at(2000) }) }), NOW + 3000)).toBe("paired");
  });

  it("stops when the code expires, whether or not the server still lists it", () => {
    const watch = watchPairing(info({ pairing: pairing() }))!;
    const after = NOW + 10 * 60_000;
    expect(pairingStep(watch, info({ pairing: pairing() }), after - 1)).toBe("poll");
    expect(pairingStep(watch, info({ pairing: pairing() }), after)).toBe("expired");
    expect(pairingStep(watch, info(), after + 2000)).toBe("expired");
  });

  it("stops when the code is withdrawn, and follows a newer code", () => {
    const watch = watchPairing(info({ pairing: pairing() }))!;
    expect(pairingStep(watch, info(), NOW + 3000)).toBe("withdrawn");
    expect(pairingStep(watch, info({ supported: false }), NOW + 3000)).toBe("withdrawn");
    const newer = pairing({ code: "ABCD-EFGH-JKLM", expires_at: at(12 * 60_000) });
    expect(pairingStep(watch, info({ pairing: newer }), NOW + 11 * 60_000)).toBe("poll");
  });
});

describe("builder status", () => {
  it("is online, offline or needs an update; offline wins", () => {
    expect(builderStatus(builder())).toBe("online");
    expect(builderStatus(builder({ online: false }))).toBe("offline");
    expect(builderStatus(builder({ compatible: false }))).toBe("needs-update");
    expect(builderStatus(builder({ online: false, compatible: false }))).toBe("offline");
  });

  it("says when an offline Mac was last seen", () => {
    const now = new Date(NOW);
    expect(statusText(builder(), now)).toBe("Online");
    expect(statusText(builder({ online: false, last_seen: at(-2 * 3_600_000) }), now)).toBe("Offline · last seen 2 hours ago");
    expect(statusText(builder({ online: false, last_seen: null }), now)).toBe("Offline · not seen yet");
    expect(statusText(builder({ compatible: false }), now)).toBe("Needs update");
  });

  it("explains what waits, and shows the current task", () => {
    expect(statusDetail(builder())).toBe("Ready to build iOS apps.");
    expect(statusDetail(builder({ task: task() }))).toBe("Building YouTube · Decrypting YouTube");
    expect(statusDetail(builder({ online: false }))).toMatch(/^iOS builds wait until Mac Studio is back\./);
    expect(statusDetail(builder({ compatible: false }))).toBe("Kiwi on Mac Studio is a different version. iOS builds wait until it is updated.");
  });

  it("names each kind of task", () => {
    expect(taskLine(task())).toBe("Building YouTube · Decrypting YouTube");
    expect(taskLine(task({ command: "plan", stage: "Looking for a new YouTube version" }))).toBe("Checking YouTube · Looking for a new YouTube version");
    expect(taskLine(task({ command: "doctor", job: null, app: null, name: null, stage: "" }))).toBe("Checking the Mac");
    expect(taskLine(task({ name: null }))).toBe("Building ios-youtube · Decrypting YouTube");
  });

  it("sums the state up for the Settings row", () => {
    expect(rowValue(info(), NOW)).toEqual({ text: "Not paired", tone: null });
    expect(rowValue(info({ pairing: pairing() }), NOW)).toEqual({ text: "Pairing…", tone: null });
    expect(rowValue(info({ builder: builder() }), NOW)).toEqual({ text: "Online", tone: "ok" });
    expect(rowValue(info({ builder: builder({ task: task() }) }), NOW)).toEqual({ text: "Building", tone: "ok" });
    expect(rowValue(info({ builder: builder({ online: false }) }), NOW)).toEqual({ text: "Offline", tone: "warn" });
    expect(rowValue(info({ builder: builder({ compatible: false }) }), NOW)).toEqual({ text: "Needs update", tone: "warn" });
  });
});

describe("iosBuilds", () => {
  it("builds on a Mac itself, on a paired builder elsewhere, or not at all", () => {
    expect(iosBuilds({ os: "macos" }, undefined)).toBe("local");
    expect(iosBuilds({ os: "linux" }, undefined)).toBe("unknown");
    expect(iosBuilds({ os: "linux" }, info())).toBe("none");
    expect(iosBuilds({ os: "other" }, info({ builder: builder() }))).toBe("builder");
    expect(iosBuilds({ os: "linux" }, info({ supported: false }))).toBe("local");
  });
});

describe("checkView", () => {
  it("shows the Mac's setup fixes", () => {
    expect(checkView({ name: "Mac build tools", ready: true })).toMatchObject({ label: "Mac build tools", status: "Ready", hint: null, settings: false });
    expect(checkView({ name: "App Store sign-in", ready: false })).toMatchObject({
      status: "Setup needed",
      hint: expect.stringMatching(/^Sign in to Kiwi's App Store downloader on the Mac\./),
      settings: false,
    });
    expect(checkView({ name: "Something new", ready: false })).toMatchObject({ status: "Setup needed", hint: null });
  });

  it("points the missing Mac to Settings › Mac Builder instead of a dead end", () => {
    expect(checkView({ name: "macOS", ready: false })).toEqual({
      label: "Mac Builder", ready: false, status: "Not paired", hint: "iOS apps need a Mac builder.", command: null, settings: true,
    });
  });

  it("says why the Mac builder isn't ready", () => {
    const check = { name: "Mac builder", ready: false };
    expect(checkView({ ...check, ready: true }, info({ builder: builder() }))).toMatchObject({ label: "Mac Builder", status: "Ready", settings: false });
    const offline = checkView(check, info({ builder: builder({ online: false, last_seen: new Date(Date.now() - 3 * 3_600_000).toISOString() }) }));
    expect(offline).toMatchObject({ status: "Offline", settings: true, command: null });
    expect(offline.hint).toBe("Mac Studio is offline (last seen 3 hours ago). iOS builds wait until it is back.");
    expect(checkView(check, info({ builder: builder({ compatible: false }) }))).toMatchObject({
      status: "Needs update", hint: "Kiwi on Mac Studio is a different version. On the Mac:", command: UPDATE_COMMAND, settings: true,
    });
    expect(checkView(check, info())).toMatchObject({ status: "Not paired", settings: true });
    // Before `GET /api/builder` answers.
    expect(checkView(check)).toMatchObject({ status: "Not ready", hint: null, settings: true });
  });
});
