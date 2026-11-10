import { useState } from "preact/hooks";
import { appColor } from "../lib/tint";

interface Props {
  url: string | null;
  name: string;
  tint?: string | null;
  size?: number;
}

/** Kiwi's own icon, for its Android app. */
export function KiwiAppIcon({ size = 60 }: { size?: number }) {
  return <AppIcon url="/icons/icon.svg" name="Kiwi" tint="#5f9a1e" size={size} />;
}

/** The icon glows in the app's colour; without artwork, its initial on that colour. */
export function AppIcon({ url, name, tint, size = 60 }: Props) {
  const [failed, setFailed] = useState(false);
  const style = { width: `${size}px`, height: `${size}px`, "--icon-tint": appColor(name, tint) };
  if (url && !failed) {
    return (
      <img
        class="app-icon"
        style={style}
        src={url}
        alt=""
        loading="lazy"
        decoding="async"
        referrerpolicy="no-referrer"
        onError={() => setFailed(true)}
      />
    );
  }
  return (
    <span
      class="app-icon app-icon-fallback"
      style={{ ...style, fontSize: `${Math.round(size * 0.45)}px` }}
      aria-hidden="true"
    >
      {name.trim().charAt(0).toUpperCase() || "?"}
    </span>
  );
}
