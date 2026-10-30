import type { JSX } from "preact";

// 24x24 stroke icons. Functions so each use gets fresh vnodes.
const PATHS = {
  browse: () => (
    <>
      <rect x="3" y="3" width="7.5" height="7.5" rx="2" />
      <rect x="13.5" y="3" width="7.5" height="7.5" rx="2" />
      <rect x="3" y="13.5" width="7.5" height="7.5" rx="2" />
      <rect x="13.5" y="13.5" width="7.5" height="7.5" rx="2" />
    </>
  ),
  updates: () => (
    <>
      <circle cx="12" cy="12" r="9.5" />
      <path d="M12 7.5v9M8 12.5l4 4 4-4" />
    </>
  ),
  apps: () => (
    <>
      <path d="M12 2.8 2.5 7.6 12 12.4l9.5-4.8L12 2.8Z" />
      <path d="m2.5 16.4 9.5 4.8 9.5-4.8M2.5 12l9.5 4.8 9.5-4.8" />
    </>
  ),
  sources: () => (
    <>
      <circle cx="12" cy="12" r="9.5" />
      <path d="M2.5 12h19M12 2.5c2.5 2.6 3.8 5.8 3.8 9.5s-1.3 6.9-3.8 9.5c-2.5-2.6-3.8-5.8-3.8-9.5S9.5 5.1 12 2.5Z" />
    </>
  ),
  settings: () => (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z" />
    </>
  ),
  search: () => (
    <>
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-3.9-3.9" />
    </>
  ),
  back: () => <path d="M15 4.5 7.5 12l7.5 7.5" />,
  chevron: () => <path d="m9 5 7 7-7 7" />,
  down: () => <path d="m6 9 6 6 6-6" />,
  refresh: () => (
    <>
      <path d="M20.5 4.5v5.5H15" />
      <path d="M20.2 14.5A8.5 8.5 0 1 1 18.1 6l2.4 2.4" />
    </>
  ),
  plus: () => <path d="M12 5v14M5 12h14" />,
  close: () => <path d="M18 6 6 18M6 6l12 12" />,
  check: () => <path d="m5 12.5 4.5 4.5L19 7.5" />,
  arrow: () => <path d="M5 12h14M13 6l6 6-6 6" />,
  clock: () => (
    <>
      <circle cx="12" cy="12" r="9.5" />
      <path d="M12 7v5l3 2" />
    </>
  ),
  link: () => (
    <>
      <path d="M10 14a4.5 4.5 0 0 0 6.4 0l3-3a4.5 4.5 0 0 0-6.4-6.4l-1 1" />
      <path d="M14 10a4.5 4.5 0 0 0-6.4 0l-3 3a4.5 4.5 0 0 0 6.4 6.4l1-1" />
    </>
  ),
  phone: () => (
    <>
      <rect x="6.5" y="2.5" width="11" height="19" rx="2.5" />
      <path d="M10.5 18.5h3" />
    </>
  ),
  key: () => (
    <>
      <circle cx="8" cy="15" r="4.5" />
      <path d="m11.5 11.5 8-8M16 7l2.5 2.5M18.5 4.5l2 2" />
    </>
  ),
  doc: () => (
    <>
      <path d="M6 2.5h8l4 4v15H6Z" />
      <path d="M14 2.5v4h4M9 12h6M9 16h6" />
    </>
  ),
  sliders: () => (
    <>
      <path d="M4 7h10M18 7h2M4 17h2M10 17h10" />
      <circle cx="16" cy="7" r="2.2" />
      <circle cx="8" cy="17" r="2.2" />
    </>
  ),
  server: () => (
    <>
      <rect x="3.5" y="4" width="17" height="7" rx="2" />
      <rect x="3.5" y="13" width="17" height="7" rx="2" />
      <path d="M7.5 7.5h.01M7.5 16.5h.01" />
    </>
  ),
  warning: () => (
    <>
      <path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z" />
      <path d="M12 9v4M12 17h.01" />
    </>
  ),
  info: () => (
    <>
      <circle cx="12" cy="12" r="9.5" />
      <path d="M12 16v-4.5M12 8h.01" />
    </>
  ),
  share: () => (
    <>
      <path d="M8.5 9H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8a2 2 0 0 0-2-2h-2.5" />
      <path d="M12 2.5v12M8.5 6 12 2.5 15.5 6" />
    </>
  ),
  bell: () => (
    <>
      <path d="M18 8.5a6 6 0 1 0-12 0c0 7-3 9-3 9h18s-3-2-3-9" />
      <path d="M13.7 21a2 2 0 0 1-3.4 0" />
    </>
  ),
  more: () => (
    <>
      <circle cx="5.5" cy="12" r="1.4" fill="currentColor" stroke="none" />
      <circle cx="12" cy="12" r="1.4" fill="currentColor" stroke="none" />
      <circle cx="18.5" cy="12" r="1.4" fill="currentColor" stroke="none" />
    </>
  ),
  eyeOff: () => (
    <>
      <path d="m3 3 18 18" />
      <path d="M10.6 5.1A10 10 0 0 1 12 5c5 0 9 4.5 10 7a13 13 0 0 1-2.6 3.6M6.6 6.6A13 13 0 0 0 2 12c1 2.5 5 7 10 7a10 10 0 0 0 4.4-1" />
      <path d="M9.9 9.9a3 3 0 0 0 4.2 4.2" />
    </>
  ),
  eye: () => (
    <>
      <path d="M2 12c1-2.5 5-7 10-7s9 4.5 10 7c-1 2.5-5 7-10 7S3 14.5 2 12Z" />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  external: () => <path d="M7 17 17 7M8.5 7H17v8.5" />,
  shield: () => (
    <>
      <path d="M12 2.8 4.5 5.6v6c0 4.6 3.1 8.2 7.5 9.6 4.4-1.4 7.5-5 7.5-9.6v-6L12 2.8Z" />
      <path d="m8.8 12 2.3 2.3 4.3-4.6" />
    </>
  ),
  wrench: () => (
    <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.8-3.8a6 6 0 0 1-7.9 7.9l-6.9 6.9a2.1 2.1 0 0 1-3-3l6.9-6.9a6 6 0 0 1 7.9-7.9l-3.8 3.8Z" />
  ),
  globe: () => (
    <>
      <circle cx="12" cy="12" r="9.5" />
      <path d="M2.5 12h19M12 2.5c2.5 2.6 3.8 5.8 3.8 9.5s-1.3 6.9-3.8 9.5c-2.5-2.6-3.8-5.8-3.8-9.5S9.5 5.1 12 2.5Z" />
    </>
  ),
  inbox: () => (
    <>
      <path d="M21.5 12.5h-6l-2 3h-3l-2-3h-6" />
      <path d="M5.5 5.6 2.5 12.5v5a2 2 0 0 0 2 2h15a2 2 0 0 0 2-2v-5l-3-6.9a2 2 0 0 0-1.8-1.1H7.3a2 2 0 0 0-1.8 1.1Z" />
    </>
  ),
} satisfies Record<string, () => JSX.Element>;

export type IconName = keyof typeof PATHS;

export function Icon({
  name,
  size = 24,
  class: className,
  strokeWidth = 2,
}: {
  name: IconName;
  size?: number;
  class?: string;
  strokeWidth?: number;
}) {
  return (
    <svg
      class={className ? `icon ${className}` : "icon"}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={strokeWidth}
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      {PATHS[name]()}
    </svg>
  );
}
