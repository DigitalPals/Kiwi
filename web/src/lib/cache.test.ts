import { describe, expect, it } from "vitest";
import { BoundedCache } from "./cache";

describe("bounded session cache", () => {
  it("evicts the least recently read result when search keys accumulate", () => {
    const cache = new BoundedCache<string, number>(2);
    cache.set("browse", 1);
    cache.set("search:a", 2);
    expect(cache.get("browse")).toBe(1);
    cache.set("search:ab", 3);
    expect(cache.get("search:a")).toBeUndefined();
    expect(cache.get("browse")).toBe(1);
    expect(cache.get("search:ab")).toBe(3);
  });

  it("updating an entry keeps it recent without evicting another one", () => {
    const cache = new BoundedCache<string, number>(2);
    cache.set("app", 1);
    cache.set("sources", 2);
    cache.set("app", 3);
    expect(cache.get("sources")).toBe(2);
    expect(cache.get("app")).toBe(3);
    cache.set("updates", 4);
    expect(cache.get("sources")).toBeUndefined();
    expect(cache.get("app")).toBe(3);
  });

  it("supports the default locale as an undefined key", () => {
    const cache = new BoundedCache<string | undefined, string>(1);
    cache.set(undefined, "default");
    expect(cache.get(undefined)).toBe("default");
    cache.set("nl", "Dutch");
    expect(cache.get(undefined)).toBeUndefined();
    expect(cache.get("nl")).toBe("Dutch");
  });
});
