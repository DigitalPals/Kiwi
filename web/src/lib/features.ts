import type { Feature, Me } from "../api/types";
import { useMe } from "./me";

const NONE: readonly Feature[] = [];

/** Whether `/api/me` says to show `feature` on this device; false while it loads. */
export function hasFeature(me: Pick<Me, "features"> | null | undefined, feature: Feature): boolean {
  return me?.features.includes(feature) ?? false;
}

/** This device's features (`Me.features`), none while `/api/me` loads. */
export function useFeatures(): { features: readonly Feature[]; has: (feature: Feature) => boolean } {
  const { me } = useMe();
  const features = me?.features ?? NONE;
  return { features, has: (feature) => features.includes(feature) };
}
