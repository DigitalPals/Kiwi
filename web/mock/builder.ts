// The Mac builder of a mock Kiwi on Linux (`GET /api/builder` and the owner routes in docs/api.md
// "Mac builder"): every state Settings › Mac Builder shows, switchable at start
// (KIWI_MOCK_BUILDER) and while running (POST /__mock/builder {"state": ...}).
import { randomInt } from "node:crypto";
import type { Builder, BuilderInfo, BuilderPairing, BuilderTask, SetupCheck } from "../src/api/types.ts";

export const MOCK_BUILDER_STATES = ["none", "pairing", "online", "building", "offline", "incompatible"] as const;
export type MockBuilderState = (typeof MOCK_BUILDER_STATES)[number];

export const isMockBuilderState = (value: unknown): value is MockBuilderState =>
  MOCK_BUILDER_STATES.includes(value as MockBuilderState);

/** kiwi-server's answer on a Mac (`409 builder_unsupported`). */
export const BUILDER_UNSUPPORTED = "This Kiwi runs on a Mac and builds iOS apps itself.";
/** The Mac the mock pairs. */
export const MOCK_MAC_NAME = "Mac Studio";
const MOCK_MAC_OS = "macOS 26.1 (arm64)";
/** How long the imitated Mac takes to run the pairing command after a code is made. */
export const AUTO_PAIR_MS = 15_000;
const PAIRING_MS = 10 * 60_000;
/** How long the imitated Mac's doctor run takes after "Check Again". */
const DOCTOR_MS = 5000;
/** 32 letters and digits without look-alikes (0/O, 1/I): 5 bits each, 60 bits per code. */
const CODE_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const MAC_CHECKS = ["Mac build tools", "Tweak toolchain", "App Store sign-in", "Private decryption enrollment"];

const iso = (ms: number) => new Date(ms).toISOString();

export function pairingCode(): string {
  const chars = Array.from({ length: 12 }, () => CODE_ALPHABET[randomInt(CODE_ALPHABET.length)]);
  return [0, 4, 8].map((i) => chars.slice(i, i + 4).join("")).join("-");
}

/** Why an iOS job can't start now: no Mac, an offline one, or one running another Kiwi. */
export type Availability = "ready" | "none" | "offline" | "incompatible";

export function createMockBuilder(options: { supported: boolean; publicUrl: string; version: string }) {
  let builder: Builder | null = null;
  /** A code; `autoPairAt` when the imitated Mac uses it (codes made by the owner in the mock). */
  let pairing: (BuilderPairing & { autoPairAt: number | null }) | null = null;
  let staticTask: BuilderTask | null = null;
  /** A doctor run asked for with "Check Again": when it starts (the next poll) and ends. */
  let doctor: { from: number; until: number } | null = null;

  const macChecks = (): SetupCheck[] => MAC_CHECKS.map((name) => ({ name, ready: true }));

  function paired(now: number, over: Partial<Builder> = {}): Builder {
    return {
      name: MOCK_MAC_NAME,
      paired_at: iso(now - 3 * 86_400_000),
      last_seen: iso(now - 4000),
      online: true,
      compatible: true,
      version: options.version,
      os: MOCK_MAC_OS,
      checks: macChecks(),
      task: null,
      ...over,
    };
  }

  function newPairing(now: number, autoPair: boolean): BuilderPairing & { autoPairAt: number | null } {
    const code = pairingCode();
    return {
      code,
      expires_at: iso(now + PAIRING_MS),
      command: `./install.sh --builder --server ${options.publicUrl} --pair ${code}`,
      autoPairAt: autoPair ? now + AUTO_PAIR_MS : null,
    };
  }

  /** Switches to one of the states the UI shows. */
  function set(state: MockBuilderState, now: number) {
    pairing = null;
    staticTask = null;
    doctor = null;
    builder = null;
    switch (state) {
      case "none":
        return;
      case "pairing":
        // Not used by any Mac: shows the code until it expires (or is cancelled).
        pairing = newPairing(now, false);
        return;
      case "online":
        builder = paired(now);
        return;
      case "building":
        builder = paired(now);
        staticTask = {
          job: "6f1c0b9e2a4d4c7f8e3b1a20",
          app: "ios-youtube",
          name: "YouTube",
          command: "build",
          stage: "Decrypting YouTube",
          started_at: iso(now - 3 * 60_000),
        };
        return;
      case "offline":
        builder = paired(now, { online: false, last_seen: iso(now - 2 * 3_600_000) });
        return;
      case "incompatible":
        builder = paired(now, { compatible: false, version: "0.1.9" });
        return;
    }
  }

  /** The imitated Mac pairs with an owner's code, codes expire, and a doctor run ends. */
  function tick(now: number) {
    if (pairing?.autoPairAt && now >= pairing.autoPairAt) {
      builder = paired(now, { paired_at: iso(now), last_seen: iso(now) });
      pairing = null;
      staticTask = null;
    }
    if (pairing && Date.parse(pairing.expires_at) <= now) pairing = null;
    if (doctor && now >= doctor.until) doctor = null;
    if (builder?.online) builder.last_seen = iso(now - 4000);
  }

  function availability(): Availability {
    if (!builder) return "none";
    if (!builder.online) return "offline";
    return builder.compatible ? "ready" : "incompatible";
  }

  /** The checks Kiwi Builds shows while a Mac is paired: the builder's own first. */
  function checks(): SetupCheck[] {
    if (!builder) return [];
    return [{ name: "Mac builder", ready: availability() === "ready" }, ...structuredClone(builder.checks)];
  }

  /** `GET /api/builder`; `job` is the iOS job a simulated build runs on the Mac. */
  function info(now: number, job: BuilderTask | null = null): BuilderInfo {
    if (!options.supported) return { supported: false, reason: BUILDER_UNSUPPORTED, builder: null, pairing: null };
    let view: Builder | null = null;
    if (builder) {
      const running = doctor && now >= doctor.from && builder.online
        ? { job: null, app: null, name: null, command: "doctor" as const, stage: "Checking the Mac's setup", started_at: iso(doctor.from) }
        : null;
      view = { ...structuredClone(builder), checks: checks(), task: builder.online && builder.compatible ? (running ?? job ?? staticTask) : null };
    }
    const code = pairing ? { code: pairing.code, expires_at: pairing.expires_at, command: pairing.command } : null;
    return { supported: true, reason: null, builder: view, pairing: code };
  }

  return {
    set,
    tick,
    info,
    availability,
    checks,
    get name(): string | null {
      return builder?.name ?? null;
    },
    /** `POST /api/builder/pairing`: a new code, replacing an unused one; the imitated Mac uses it. */
    startPairing(now: number): BuilderPairing {
      pairing = newPairing(now, true);
      return { code: pairing.code, expires_at: pairing.expires_at, command: pairing.command };
    },
    cancelPairing() {
      pairing = null;
    },
    /** `DELETE /api/builder`: false when none is paired. */
    remove(): boolean {
      if (!builder) return false;
      builder = null;
      staticTask = null;
      doctor = null;
      return true;
    },
    /** `POST /api/builder/check`: a doctor run at the Mac's next poll (now, if it is online). */
    requestCheck(now: number): boolean {
      if (!builder) return false;
      doctor = builder.online ? { from: now + 1500, until: now + 1500 + DOCTOR_MS } : { from: Infinity, until: Infinity };
      return true;
    },
  };
}

export type MockBuilder = ReturnType<typeof createMockBuilder>;
