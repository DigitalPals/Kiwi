import { describe, expect, it } from "vitest";
import type { AppSummary, Original } from "../api/types";
import { countryName, isBehind, originalNote, searchTerm, timeBetween } from "./store";

const original = (status: Original["status"]): Original => ({
  bundle_id: "com.spotify.client",
  name: "Spotify: Music and Podcasts",
  short_name: "Spotify",
  developer: "Spotify",
  version: "9.1.94",
  released_at: "2026-10-23T08:00:00Z",
  icon_url: null,
  store_url: "https://apps.apple.com/nl/app/spotify/id324684580",
  app_version: "9.1.50",
  status,
});

const app = (status: Original["status"], date: string | null = "2026-06-12") =>
  ({
    bundle_id: "com.spotify.client",
    original: original(status),
    state: { target: { date } },
  }) as unknown as AppSummary;

describe("store helpers", () => {
  it("names countries", () => {
    expect(countryName("nl")).toBe("Netherlands");
    expect(countryName("US")).toBe("United States");
  });

  it("measures how far apart releases are", () => {
    expect(timeBetween("2026-06-12", "2026-10-23T08:00:00Z")).toBe("4 months");
    expect(timeBetween("2026-10-20", "2026-10-23")).toBe("3 days");
    expect(timeBetween("2026-10-01", "2026-10-22")).toBe("3 weeks");
    expect(timeBetween("2024-10-01", "2026-10-22")).toBe("2 years");
    expect(timeBetween("2026-10-23T01:00:00Z", "2026-10-23T08:00:00Z")).toBeNull();
    expect(timeBetween(null, "2026-10-23")).toBeNull();
  });

  it("explains the comparison and the match", () => {
    expect(originalNote(app("behind"), "bundle_id")).toBe(
      "The App Store version is 4 months newer. Matched by bundle ID com.spotify.client.",
    );
    expect(originalNote(app("behind", null), "linked")).toBe("The App Store has a newer version. Linked by hand.");
    expect(originalNote(app("same"), "bundle_id")).toMatch(/^Same version as the App Store\./);
    expect(originalNote(app("unknown"), "bundle_id")).toMatch(/^The version numbers can't be compared\./);
    expect(isBehind(original("behind"))).toBe(true);
    expect(isBehind(original("ahead"))).toBe(false);
    expect(isBehind(null)).toBe(false);
  });

  it("drops notes from names for searching", () => {
    expect(searchTerm("Spotify (EeveeSpotifyReincarnated)")).toBe("Spotify");
    expect(searchTerm("YouMod + Tweaks (driftywinds)")).toBe("YouMod + Tweaks");
    expect(searchTerm("Instagram [RyukGram] Plus")).toBe("Instagram Plus");
    expect(searchTerm("(only notes)")).toBe("(only notes)");
  });
});
