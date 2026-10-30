import { useId } from "preact/hooks";

// The kiwi slice from the app icon (public/icons/icon.svg) without its background, drawn on a
// 100x100 grid. Rays and seeds are generated once.
const RAYS = Array.from({ length: 28 }, (_, i) => {
  const a = (i / 28) * Math.PI * 2;
  const outer = 35 + (i % 3) * 2.5;
  const p = (r: number) => `${(50 + Math.cos(a) * r).toFixed(1)} ${(50 + Math.sin(a) * r).toFixed(1)}`;
  return `M${p(15)}L${p(outer)}`;
}).join("");

const SEEDS = Array.from({ length: 18 }, (_, i) => {
  const a = (i / 18) * Math.PI * 2;
  const r = 18 + (i % 2) * 2.6;
  return {
    x: (50 + Math.cos(a) * r).toFixed(1),
    y: (50 + Math.sin(a) * r).toFixed(1),
    angle: ((a * 180) / Math.PI + 90).toFixed(1),
  };
});

export function KiwiLogo({ size = 40 }: { size?: number }) {
  // Gradient ids must be unique per instance: a hidden copy would otherwise break the others.
  const id = useId();
  const skin = `${id}-skin`;
  const flesh = `${id}-flesh`;
  const core = `${id}-core`;
  return (
    <svg class="kiwi-logo" width={size} height={size} viewBox="0 0 100 100" aria-hidden="true">
      <defs>
        <radialGradient id={skin} cx="0.45" cy="0.4" r="0.65">
          <stop offset="0" stop-color="#9a6a3a" />
          <stop offset="1" stop-color="#5e3d1c" />
        </radialGradient>
        <radialGradient id={flesh} cx="0.5" cy="0.5" r="0.5">
          <stop offset="0" stop-color="#e9f5b4" />
          <stop offset="0.3" stop-color="#b5dc52" />
          <stop offset="0.85" stop-color="#78b62a" />
          <stop offset="1" stop-color="#5f9a1e" />
        </radialGradient>
        <radialGradient id={core} cx="0.5" cy="0.45" r="0.55">
          <stop offset="0" stop-color="#fffef4" />
          <stop offset="0.7" stop-color="#f5f6d6" />
          <stop offset="1" stop-color="#e3efa8" />
        </radialGradient>
      </defs>
      <circle cx="50" cy="50" r="48" fill={`url(#${skin})`} />
      <circle cx="50" cy="50" r="44.6" fill="#dcef9e" />
      <circle cx="50" cy="50" r="43.6" fill={`url(#${flesh})`} />
      <path d={RAYS} stroke="#f4fbd8" stroke-opacity="0.4" stroke-width="0.8" stroke-linecap="round" />
      <ellipse cx="50" cy="50" rx="11" ry="9.8" fill={`url(#${core})`} />
      <g fill="#2a1d10">
        {SEEDS.map((s) => (
          <ellipse key={`${s.x},${s.y}`} cx={s.x} cy={s.y} rx="1.3" ry="2.8" transform={`rotate(${s.angle} ${s.x} ${s.y})`} />
        ))}
      </g>
      <path
        d="M17 37A35 35 0 0 1 38 17"
        stroke="#fff"
        stroke-opacity="0.4"
        stroke-width="2.4"
        stroke-linecap="round"
        fill="none"
      />
    </svg>
  );
}
