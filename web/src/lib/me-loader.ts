import type { Me } from "../api/types";

export interface MeLoader {
  /** Loads `/api/me` now, or once more after the request already on its way. */
  load: () => Promise<void>;
  /** Replaces Me, e.g. with the answer of `PATCH /api/me`. */
  set: (me: Me) => void;
}

/**
 * How MeProvider loads `/api/me`: one request at a time (a load asked for meanwhile runs once
 * after it), and every change to Me starts a new generation. A load's answer, or its error,
 * only lands when Me is still the generation it started from: a request sent before a
 * `PATCH /api/me` can answer after it and would undo the change, such as the platform chosen
 * in the first-run setup.
 */
export function createMeLoader(
  fetchMe: () => Promise<Me>,
  show: { me: (me: Me) => void; error: (error: Error | null) => void },
): MeLoader {
  let generation = 0;
  let inFlight = false;
  let again = false;

  const set = (me: Me) => {
    generation++;
    show.me(me);
  };

  const load = async () => {
    if (inFlight) {
      again = true;
      return;
    }
    inFlight = true;
    const started = generation;
    try {
      const me = await fetchMe();
      if (generation === started) {
        set(me);
        show.error(null);
      }
    } catch (e) {
      if (generation === started) show.error(e instanceof Error ? e : new Error(String(e)));
    } finally {
      inFlight = false;
      if (again) {
        again = false;
        void load();
      }
    }
  };

  return { load, set };
}
