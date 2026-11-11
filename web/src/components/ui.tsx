import type { ComponentChildren } from "preact";
import { useLayoutEffect, useRef, useState } from "preact/hooks";
import type { Warning } from "../api/types";
import { goBack } from "../lib/router";
import { Icon, type IconName } from "./Icon";

/**
 * "‹ Settings": back in history, or to `fallback` when the page was opened directly;
 * `onBack` replaces both (e.g. the previous step of the setup).
 */
export function BackBar({ label, fallback = "/", onBack }: { label: string; fallback?: string; onBack?: () => void }) {
  return (
    <div class="back-bar">
      <button type="button" class="back-button" onClick={onBack ?? (() => goBack(fallback))}>
        <Icon name="back" size={24} strokeWidth={2.2} />
        {label}
      </button>
    </div>
  );
}

export function Page(props: {
  title: string;
  actions?: ComponentChildren;
  /** Line under the title, e.g. when data was last checked. */
  summary?: ComponentChildren;
  children: ComponentChildren;
}) {
  // Tab pages: a soft kiwi glow behind the large title.
  return (
    <main class="page page-glow">
      <header class="page-header">
        <h1 class="large-title">{props.title}</h1>
        {props.actions && <div class="page-actions">{props.actions}</div>}
      </header>
      {props.summary && <p class="page-summary">{props.summary}</p>}
      {props.children}
    </main>
  );
}

export function Spinner({ large }: { large?: boolean }) {
  return <span class={large ? "spinner spinner-lg" : "spinner"} aria-hidden="true" />;
}

export function Loading({ label = "Loading…" }: { label?: string }) {
  return (
    <div class="state" role="status">
      <Spinner large />
      <p class="state-message">{label}</p>
    </div>
  );
}

export function SkeletonRows({ count = 6 }: { count?: number }) {
  return (
    <ul class="app-list" aria-busy="true" aria-label="Loading">
      {Array.from({ length: count }, (_, i) => (
        <li key={i} class="app-row skeleton-row">
          <span class="app-icon skeleton" style={{ width: "48px", height: "48px" }} />
          <span class="skeleton-lines">
            <span class="skeleton skeleton-line" style={{ width: `${55 + ((i * 17) % 30)}%` }} />
            <span class="skeleton skeleton-line short" />
          </span>
        </li>
      ))}
    </ul>
  );
}

export function Empty(props: {
  icon: IconName;
  title: string;
  message?: ComponentChildren;
  /** Replaces the icon, e.g. with the Kiwi logo. */
  art?: ComponentChildren;
  children?: ComponentChildren;
}) {
  return (
    <div class="state">
      {props.art ?? (
        <span class="state-icon">
          <Icon name={props.icon} size={36} strokeWidth={1.8} />
        </span>
      )}
      <h2 class="state-title">{props.title}</h2>
      {props.message && <p class="state-message">{props.message}</p>}
      {props.children}
    </div>
  );
}

export function ErrorState({ error, onRetry, title = "Couldn't load" }: {
  error: Error;
  onRetry?: () => void;
  title?: string;
}) {
  return (
    <Empty icon="warning" title={title} message={error.message}>
      {onRetry && (
        <button type="button" class="button" onClick={onRetry}>
          Try Again
        </button>
      )}
    </Empty>
  );
}

const LEVEL_ICON: Record<Warning["level"], IconName> = { info: "info", warning: "warning", error: "warning" };

export function Banner(props: {
  level: Warning["level"];
  children: ComponentChildren;
  icon?: IconName;
  onDismiss?: () => void;
  /** Makes the whole banner a link, e.g. to the screen that fixes the problem. */
  href?: string | null;
}) {
  const body = (
    <>
      <Icon name={props.icon ?? LEVEL_ICON[props.level]} size={20} class="banner-icon" />
      <div class="banner-body">{props.children}</div>
      {props.href && <Icon name="chevron" size={16} class="banner-chevron" />}
      {props.onDismiss && (
        <button type="button" class="banner-close" onClick={props.onDismiss} aria-label="Dismiss">
          <Icon name="close" size={16} />
        </button>
      )}
    </>
  );
  if (props.href) {
    return (
      <a class={`banner banner-${props.level}`} href={props.href}>
        {body}
      </a>
    );
  }
  return (
    <div class={`banner banner-${props.level}`} role={props.level === "error" ? "alert" : "status"}>
      {body}
    </div>
  );
}

export function Toggle(props: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <input
      type="checkbox"
      role="switch"
      class="switch"
      checked={props.checked}
      disabled={props.disabled}
      aria-label={props.label}
      onChange={(e) => props.onChange(e.currentTarget.checked)}
    />
  );
}

export function Section(props: { title?: string; footer?: ComponentChildren; children: ComponentChildren }) {
  return (
    <section class="section">
      {props.title && <h2 class="section-title">{props.title}</h2>}
      <div class="cells">{props.children}</div>
      {props.footer && <div class="section-footer">{props.footer}</div>}
    </section>
  );
}

export function Cell(props: {
  label: ComponentChildren;
  children?: ComponentChildren;
  sub?: ComponentChildren;
  /** A glyph on a coloured tile in front of the label. */
  icon?: IconName;
}) {
  return (
    <div class={props.icon ? "cell has-icon" : "cell"}>
      {props.icon && <CellIcon name={props.icon} />}
      <div class="cell-main">
        <div class="cell-label">{props.label}</div>
        {props.sub && <div class="cell-sub">{props.sub}</div>}
      </div>
      {props.children !== undefined && <div class="cell-value">{props.children}</div>}
    </div>
  );
}

/** Tile colours per glyph, like iOS Settings; anything else gets the accent. */
const TILE_COLORS: Partial<Record<IconName, string>> = {
  phone: "#007aff",
  bell: "#ff3b30",
  shield: "#2fae5f",
  globe: "#32ade6",
  apps: "#5e5ce6",
  refresh: "#30b0c7",
  key: "#f2a900",
  doc: "#8e8e93",
  server: "#636a74",
  close: "#ff3b30",
  wrench: "#1e6b4d",
};

/** A white glyph on a coloured tile in front of a cell; `color` overrides the glyph's tile colour. */
export function CellIcon({ name, color }: { name: IconName; color?: string }) {
  return (
    <span class="cell-icon" style={{ "--tile": color ?? TILE_COLORS[name] ?? "var(--accent)" }} aria-hidden="true">
      <Icon name={name} size={17} strokeWidth={2} />
    </span>
  );
}

/** A row that opens another page: a tile or picture, the label, a value and a chevron. */
export function LinkCell(props: {
  href: string;
  label: ComponentChildren;
  sub?: ComponentChildren;
  icon?: IconName;
  /** Replaces the glyph, e.g. with an app icon. */
  art?: ComponentChildren;
  value?: ComponentChildren;
}) {
  const art = props.art ?? (props.icon && <CellIcon name={props.icon} />);
  return (
    <a class={art ? "cell has-icon cell-link" : "cell cell-link"} href={props.href}>
      {art}
      <div class="cell-main">
        <div class="cell-label">{props.label}</div>
        {props.sub && <div class="cell-sub">{props.sub}</div>}
      </div>
      {props.value !== undefined && props.value !== null && props.value !== false && (
        <div class="cell-value">{props.value}</div>
      )}
      <Icon name="chevron" size={16} class="cell-chevron" />
    </a>
  );
}

/** "All Sources ⌄": the chosen option's label; tapping it opens the native picker. */
export function MenuPicker<T extends string>(props: {
  label: string;
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  const current = props.options.find((o) => o.value === props.value)?.label ?? "";
  return (
    <label class="menu-picker">
      <span class="menu-picker-text">{current}</span>
      <Icon name="down" size={12} strokeWidth={2.6} />
      <select aria-label={props.label} value={props.value} onChange={(e) => props.onChange(e.currentTarget.value as T)}>
        {props.options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

/** "1.0 → 2.0": the installed version and the version an update installs. */
export function VersionChange(props: { from: string; to: string; large?: boolean }) {
  return (
    <span
      class={props.large ? "version-change large" : "version-change"}
      aria-label={`Version ${props.from} to ${props.to}`}
    >
      <span class="from">{props.from}</span>
      <Icon name="arrow" size={props.large ? 14 : 12} strokeWidth={2.4} />
      <span class="to">{props.to}</span>
    </span>
  );
}

/** Plain text (never HTML) with preserved line breaks, collapsed to a few lines with "more". */
export function ExpandableText({ text, lines = 4 }: { text: string; lines?: number }) {
  const ref = useRef<HTMLParagraphElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [overflows, setOverflows] = useState(false);

  useLayoutEffect(() => {
    const el = ref.current;
    if (el && !expanded) setOverflows(el.scrollHeight > el.clientHeight + 1);
  }, [text, expanded]);

  return (
    <div class="expandable">
      <p ref={ref} class={expanded ? "prose" : "prose clamped"} style={{ "--lines": String(lines) }}>
        {text}
      </p>
      {overflows && !expanded && (
        <button type="button" class="more-text" onClick={() => setExpanded(true)}>
          more
        </button>
      )}
    </div>
  );
}

/** A command to run in Terminal, selectable with one tap; lines break between its words. */
export function CommandText({ command, class: className = "error-command" }: { command: string; class?: string }) {
  return (
    <code class={className}>
      {command.split(" ").flatMap((part, i) => [i > 0 ? " " : null, <span key={i}>{part}</span>])}
    </code>
  );
}
