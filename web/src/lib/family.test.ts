import { describe, expect, it } from "vitest";
import type { PersonApp, Registration } from "../api/types";
import { deviceKind, featuredKeys, firstName, gateStep, modelName, possessive } from "./family";

const registration = (patch: Partial<Registration>): Registration => ({
  status: "none",
  udid: null,
  product: null,
  os_build: null,
  requested_at: null,
  decided_at: null,
  apple_registered: false,
  in_profile: false,
  ready: false,
  error: null,
  ...patch,
});

describe("family helpers", () => {
  it("names devices and people", () => {
    expect(modelName("iPhone15,4")).toBe("iPhone 15");
    expect(modelName("iPhone99,9")).toBe("iPhone");
    expect(modelName("iPad14,1")).toBe("iPad");
    expect(modelName(null)).toBe("iPhone");
    expect(deviceKind("iPod9,1")).toBe("iPod touch");
    expect(firstName("Noah Morgan")).toBe("Noah");
    expect(possessive("Noah Morgan")).toBe("Noah's");
    expect(possessive("James")).toBe("James'");
  });


  it("decides which registration screen a member sees", () => {
    expect(gateStep(registration({}), false)).toBe("welcome");
    expect(gateStep(registration({}), true)).toBe("waiting");
    expect(gateStep(registration({ status: "pending" }), false)).toBe("waiting");
    expect(gateStep(registration({ status: "approved" }), false)).toBe("waiting");
    expect(gateStep(registration({ status: "declined" }), true)).toBe("declined");
    expect(gateStep(registration({ status: "approved", in_profile: true, ready: true }), false)).toBe("ready");
  });

  it("features the apps a person may use or has installed", () => {
    const app = (bundle: string, allowed: boolean, installed: string | null): PersonApp => ({
      source_id: "s",
      source_name: "S",
      bundle_id: bundle,
      name: bundle,
      subtitle: null,
      icon_url: null,
      tint_color: null,
      allowed,
      explicit: allowed,
      installed_version: installed,
    });
    expect([...featuredKeys([app("a", true, null), app("b", false, "1.0"), app("c", false, null)])]).toEqual(["s/a", "s/b"]);
  });
});
