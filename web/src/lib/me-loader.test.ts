import { describe, expect, it } from "vitest";
import type { Me } from "../api/types";
import { createMeLoader } from "./me-loader";

/** A Me told apart by its device's platform, which is what the setup changes. */
const meOn = (platform: "ios" | "android") => ({ device: { platform } }) as Me;

/** `/api/me` answered by hand, in whatever order the test wants. */
function harness() {
  const requests: { resolve: (me: Me) => void; reject: (error: Error) => void }[] = [];
  const shown: Me[] = [];
  const errors: (Error | null)[] = [];
  const loader = createMeLoader(
    () => new Promise<Me>((resolve, reject) => requests.push({ resolve, reject })),
    { me: (me) => shown.push(me), error: (error) => errors.push(error) },
  );
  return { loader, requests, shown, errors };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("createMeLoader", () => {
  it("shows what /api/me answers", async () => {
    const { loader, requests, shown, errors } = harness();
    const done = loader.load();
    requests[0]?.resolve(meOn("ios"));
    await done;
    expect(shown).toEqual([meOn("ios")]);
    expect(errors).toEqual([null]);
  });

  it("drops a reload that started before a newer Me was set", async () => {
    const { loader, requests, shown } = harness();
    // The reload leaves first, then the setup's PATCH /api/me answers…
    const done = loader.load();
    loader.set(meOn("android"));
    // …and the reload's answer, from before the change, arrives last.
    requests[0]?.resolve(meOn("ios"));
    await done;
    expect(shown).toEqual([meOn("android")]);

    // A reload started after the change lands as usual.
    const next = loader.load();
    requests[1]?.resolve(meOn("android"));
    await next;
    expect(shown).toEqual([meOn("android"), meOn("android")]);
  });

  it("drops the error of a reload that started before a newer Me was set", async () => {
    const { loader, requests, errors } = harness();
    const done = loader.load();
    loader.set(meOn("android"));
    requests[0]?.reject(new Error("offline"));
    await done;
    expect(errors).toEqual([]);
  });

  it("runs one more request for every reload asked for while one is on its way", async () => {
    const { loader, requests, shown } = harness();
    const first = loader.load();
    void loader.load();
    void loader.load();
    expect(requests).toHaveLength(1);
    requests[0]?.resolve(meOn("ios"));
    await first;
    expect(requests).toHaveLength(2);
    requests[1]?.resolve(meOn("android"));
    await settle();
    expect(shown).toEqual([meOn("ios"), meOn("android")]);
    expect(requests).toHaveLength(2);
  });
});
