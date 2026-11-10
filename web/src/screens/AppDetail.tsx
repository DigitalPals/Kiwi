import type { ComponentChildren } from "preact";
import { useCallback, useEffect, useState } from "preact/hooks";
import { ApiError, api } from "../api/client";
import type { AppDetail, AppPatch, AppState, AppSummary, Platform, Release, StoreApp } from "../api/types";
import { ActionButton, ReinstallButton, VersionButton } from "../components/ActionButton";
import { AppIcon } from "../components/AppIcon";
import { Icon } from "../components/Icon";
import { Banner, Empty, ErrorState, ExpandableText, LinkCell, Loading, Spinner, VersionChange } from "../components/ui";
import { canReinstall, foreignInstallWarning, replacesLabel, versionChange, type ButtonView } from "../lib/action";
import { BUILD_SOURCE_ID } from "../lib/automation";
import {
  deviceName,
  unconfirmedNote,
  expiryInfo,
  formatBytes,
  formatDate,
  formatPercent,
  formatRelative,
  formatRemaining,
  formatVersion,
  osName,
  osRequirement,
  privacyLabel,
  splitBytes,
} from "../lib/format";
import { useFeatures } from "../lib/features";
import { useDebounced, useOnForeground, useResource, useTask } from "../lib/hooks";
import { goBack, previousTab } from "../lib/router";
import { BUILDS_PATH, appPath, buildPath, shownTab, tabsFor } from "../lib/routes";
import { originalNote, searchTerm } from "../lib/store";
import { appColor, heroColors } from "../lib/tint";

function BackBar({ trailing }: { trailing?: ComponentChildren }) {
  const { features } = useFeatures();
  const tabs = tabsFor(features);
  const tab = tabs.find((t) => t.tab === shownTab(previousTab(), tabs));
  const label = tab?.label ?? "Browse";
  const fallback = tab?.path ?? "/";
  return (
    <div class="back-bar">
      <button type="button" class="back-button" onClick={() => goBack(fallback)}>
        <Icon name="back" size={24} strokeWidth={2.2} />
        {label}
      </button>
      {trailing}
    </div>
  );
}

/** What the app is currently matched with, for the menu. */
function originalLabel(app: AppDetail): string {
  const name = app.original?.short_name;
  switch (app.original_match) {
    case "bundle_id":
      return `Now: ${name} (matched by bundle ID)`;
    case "linked":
      return `Now: ${name} (linked by hand)`;
    case "none":
      return "Now: none";
    case "not_found":
      return "Not found in the App Store";
    default:
      return "Not looked up yet";
  }
}

/** "•••": reinstall the app, hide or show it in Browse, change its original app. */
function AppMenu(props: {
  app: AppDetail;
  onChange: (app: AppDetail) => void;
  onLink: () => void;
  onReinstall: (() => void) | null;
}) {
  const { app, onReinstall } = props;
  const [open, setOpen] = useState(false);
  const toggle = useTask(async () => {
    setOpen(false);
    props.onChange(await api.updateApp(app.source_id, app.bundle_id, { hidden: !app.hidden }));
  });
  return (
    <div class="menu-anchor">
      <button
        type="button"
        class={open ? "more-button open" : "more-button"}
        aria-label="More"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        disabled={toggle.pending}
      >
        {toggle.pending ? <Spinner /> : <Icon name="more" size={20} />}
      </button>
      {open && (
        <>
          <div class="menu-scrim" onClick={() => setOpen(false)} />
          <div class="menu" role="menu">
            {onReinstall && (
              <button
                type="button"
                role="menuitem"
                class="menu-item"
                onClick={() => {
                  setOpen(false);
                  onReinstall();
                }}
              >
                <span>
                  Reinstall
                  <span class="menu-sub">If it's no longer on this {deviceName(app.platform)}</span>
                </span>
                <Icon name="refresh" size={19} />
              </button>
            )}
            <button type="button" role="menuitem" class="menu-item" onClick={() => void toggle.run()}>
              <span>{app.hidden ? "Show in Browse" : "Hide from Browse"}</span>
              <Icon name={app.hidden ? "eye" : "eyeOff"} size={19} />
            </button>
            {/* Only iPhone apps are compared with the App Store. */}
            {app.platform !== "android" && (
              <button
                type="button"
                role="menuitem"
                class="menu-item"
                onClick={() => {
                  setOpen(false);
                  props.onLink();
                }}
              >
                <span>
                  Change Original App…
                  <span class="menu-sub">{originalLabel(app)}</span>
                </span>
                <Icon name="link" size={19} />
              </button>
            )}
          </div>
        </>
      )}
      {toggle.error && <p class="menu-error">{toggle.error}</p>}
    </div>
  );
}

/** Asks which App Store app this app is based on: by bundle id, none, or a search result. */
function LinkSheet(props: { app: AppDetail; onChange: (app: AppDetail) => void; onClose: () => void }) {
  const { app, onClose } = props;
  const [term, setTerm] = useState(() => searchTerm(app.original?.short_name ?? app.name));
  const q = useDebounced(term.trim(), 350);
  const results = useResource<StoreApp[]>(`store-search?${q}`, () => (q ? api.searchStore(q) : Promise.resolve([])));
  const choose = useTask(async (original: AppPatch["original"]) => {
    props.onChange(await api.updateApp(app.source_id, app.bundle_id, { original }));
    onClose();
  });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const automatic = app.original_match === "bundle_id" || app.original_match === "not_found" || app.original_match === "pending";
  const linked = app.original_match === "linked" ? app.original?.bundle_id : null;
  const check = <Icon name="check" size={18} strokeWidth={2.6} class="tone-ok" />;
  let list;
  if (!q) list = null;
  else if (results.error) list = <p class="sheet-note form-error">{results.error.message}</p>;
  else if (!results.data || (results.loading && results.data.length === 0)) {
    list = (
      <p class="sheet-note">
        <Spinner /> Searching…
      </p>
    );
  } else if (results.data.length === 0) list = <p class="sheet-note">No apps found for “{q}”.</p>;
  else {
    list = (
      <div class="cells">
        {results.data.map((r) => (
          <button
            key={r.bundle_id}
            type="button"
            class="cell has-icon cell-button"
            onClick={() => void choose.run({ bundle_id: r.bundle_id })}
            disabled={choose.pending}
          >
            <AppIcon url={r.icon_url} name={r.short_name} size={40} />
            <div class="cell-main">
              <div class="cell-label one-line">{r.name}</div>
              <div class="cell-sub">{[r.developer, r.version].filter(Boolean).join(" · ")}</div>
            </div>
            {linked === r.bundle_id && check}
          </button>
        ))}
      </div>
    );
  }
  return (
    <div class="sheet-backdrop" onClick={onClose}>
      <div class="sheet" role="dialog" aria-modal="true" aria-labelledby="link-title" onClick={(e) => e.stopPropagation()}>
        <header class="sheet-header">
          <h2 id="link-title">Original App</h2>
          <button type="button" class="text-button" onClick={onClose}>
            Cancel
          </button>
        </header>
        <p class="sheet-lead">Which App Store app is {app.name} based on?</p>
        <div class="cells">
          <button type="button" class="cell cell-button" onClick={() => void choose.run(null)} disabled={choose.pending}>
            <div class="cell-main">
              <div class="cell-label">Match by Bundle ID</div>
              <div class="cell-sub">The App Store app that uses {app.bundle_id}</div>
            </div>
            {automatic && check}
          </button>
          <button
            type="button"
            class="cell cell-button"
            onClick={() => void choose.run({ bundle_id: null })}
            disabled={choose.pending}
          >
            <div class="cell-main cell-label">Not Based on an App Store App</div>
            {app.original_match === "none" && check}
          </button>
        </div>
        <label class="search sheet-search">
          <Icon name="search" size={18} />
          <input
            type="search"
            placeholder="Search the App Store"
            aria-label="Search the App Store"
            value={term}
            onInput={(e) => setTerm(e.currentTarget.value)}
            enterKeyHint="search"
            autocapitalize="off"
            autocorrect="off"
            spellcheck={false}
          />
        </label>
        {choose.error && <p class="sheet-note form-error">{choose.error}</p>}
        {list}
      </div>
    </div>
  );
}

/** Step and error details that do not fit under the pill. */
function ActionStatus({ view, platform }: { view: ButtonView; platform: Platform }) {
  const android = platform === "android";
  if (view.kind === "progress") {
    const percent = formatPercent(view.progress);
    return (
      <p class="action-status">
        {view.caption ?? "Preparing"}
        {percent && ` · ${percent}`}
      </p>
    );
  }
  if (view.kind === "prepare" && view.error && view.caption) {
    return <p class="action-status danger">{view.caption}</p>;
  }
  if (view.kind === "installing") {
    if (android) return <p class="action-status">Open the downloaded file and choose Install. Android may first ask you to allow installs from your browser.</p>;
    return <p class="action-status">Follow the prompt on your iPhone. Kiwi updates when the install finishes.</p>;
  }
  if (view.kind === "link") {
    if (view.caption === "Re-sign") return <p class="action-status">Re-signed build with a later expiry date.</p>;
    if (android) return <p class="action-status">Ready. Tap {view.label} to download the app, then open the file to install it.</p>;
    return <p class="action-status">Ready. Tap {view.label} and confirm on your iPhone.</p>;
  }
  if (view.kind === "disabled" && view.caption) return <p class="action-status">{view.caption}</p>;
  return null;
}

function requirement(release: Release | null, platform: Platform): [string, string] {
  if (!release) return ["—", ""];
  const os = osName(platform);
  const { min_os: min, max_os: max } = release;
  if (min && max) return [`${os} ${min}`, `to ${max}`];
  if (min) return [`${os} ${min}`, "or later"];
  if (max) return [`${os} ${max}`, "or earlier"];
  return platform === "android" ? [os, "phone or tablet"] : ["Any", "iOS version"];
}

function InfoStrip({ app, release }: { app: AppDetail; release: Release | null }) {
  const [size, unit] = splitBytes(release?.size ?? app.state.build?.size ?? null);
  const [os, osSub] = requirement(release, app.platform);
  const expiresAt = app.state.installed?.expires_at;
  const expiry = expiryInfo(expiresAt);
  const original = app.original;
  const items: { label: string; value: string; sub: string; tone?: string }[] = [
    { label: "Version", value: release?.version ?? "—", sub: release?.date ? formatRelative(release.date) : "" },
    // For patched apps the App Store version matters more than the minimum iOS version,
    // which Information still lists.
    original
      ? {
          label: "App Store",
          value: original.version,
          sub: original.released_at ? formatRelative(original.released_at) : "",
          tone: original.status === "behind" ? "tone-warn" : "",
        }
      : { label: "Size", value: size, sub: unit },
    original ? { label: "Size", value: size, sub: unit } : { label: "Requires", value: os, sub: osSub },
    expiry
      ? {
          label: "Signed",
          value: formatRemaining(expiresAt) ?? "—",
          sub: formatDate(expiresAt),
          tone: expiry.tone === "ok" ? "" : `tone-${expiry.tone}`,
        }
      : { label: "Source", value: app.source_name, sub: app.category ?? "" },
  ];
  return (
    <dl class="info-strip">
      {items.map((item) => (
        <div key={item.label} class="info-item">
          <dt>{item.label}</dt>
          <dd class={item.tone || undefined}>
            {item.value}
            <span class="info-sub">{item.sub || " "}</span>
          </dd>
        </div>
      ))}
    </dl>
  );
}

/** The App Store app this app is based on, compared with this app's version. */
function OriginalSection({ app, onLink }: { app: AppDetail; onLink: () => void }) {
  const original = app.original;
  if (!original) {
    if (app.original_match !== "not_found") return null;
    return (
      <section class="detail-section">
        <h2 class="section-heading">Original App</h2>
        <div class="cells">
          <div class="cell">
            <div class="cell-main">
              <div class="cell-label">Not in the App Store</div>
              <div class="cell-sub">No App Store app uses {app.bundle_id}</div>
            </div>
            <button type="button" class="text-button" onClick={onLink}>
              Link…
            </button>
          </div>
        </div>
      </section>
    );
  }
  const released = app.state.target?.date;
  const tone = original.status === "behind" ? "tone-warn" : "";
  return (
    <section class="detail-section">
      <h2 class="section-heading">Original App</h2>
      <div class="cells">
        <a class="cell has-icon cell-link" href={original.store_url} target="_blank" rel="noreferrer">
          <AppIcon url={original.icon_url} name={original.short_name} size={42} />
          <div class="cell-main">
            <div class="cell-label one-line">{original.name}</div>
            <div class="cell-sub">{[original.developer, "App Store"].filter(Boolean).join(" · ")}</div>
          </div>
          <span class="cell-action">
            View <Icon name="external" size={14} strokeWidth={2.4} />
          </span>
        </a>
        <div class="cell">
          <div class="cell-main cell-label">This app</div>
          <div class="cell-value">
            {original.app_version ?? "—"}
            {released && ` · ${formatDate(released)}`}
          </div>
        </div>
        <div class="cell">
          <div class="cell-main cell-label">App Store</div>
          <div class={`cell-value ${tone}`}>
            {original.version}
            {original.released_at && ` · ${formatDate(original.released_at)}`}
          </div>
        </div>
      </div>
      <p class="section-footer">{originalNote(app, app.original_match)}</p>
    </section>
  );
}

/** The same app from other sources (e.g. other YouTube tweaks). */
function OtherVersions({ app }: { app: AppDetail }) {
  if (app.other_versions.length === 0) return null;
  const replace = app.other_versions.some((o) => o.bundle_id === app.bundle_id);
  return (
    <section class="detail-section">
      <h2 class="section-heading">{app.original ? `Other ${app.original.short_name} Versions` : "Other Versions"}</h2>
      <div class="cells">
        {app.other_versions.map((o) => (
          <a key={`${o.source_id}/${o.bundle_id}`} class="cell has-icon cell-link" href={appPath(o.source_id, o.bundle_id)}>
            <AppIcon url={o.icon_url} name={o.name} tint={o.tint_color} size={42} />
            <div class="cell-main">
              <div class="cell-label one-line">{o.name}</div>
              <div class="cell-sub">{[o.source_name, o.version].filter(Boolean).join(" · ")}</div>
            </div>
            {o.installed ? (
              <span class="tag">Installed</span>
            ) : o.hidden ? (
              <span class="tag">Hidden</span>
            ) : (
              <Icon name="chevron" size={16} class="cell-chevron" />
            )}
          </a>
        ))}
      </div>
      {replace && (
        <p class="section-footer">Versions with the same bundle ID replace each other when you install one.</p>
      )}
    </section>
  );
}

function InstalledCard({ app }: { app: AppDetail }) {
  const installed = app.state.installed;
  if (!installed) return null;
  const expiry = expiryInfo(installed.expires_at);
  const unconfirmed = unconfirmedNote(installed, app.platform);
  return (
    <section class="detail-section">
      <h2 class="section-heading">Installed</h2>
      <div class="cells">
        <div class="cell">
          <div class="cell-main cell-label">Version</div>
          <div class="cell-value">{formatVersion(installed)}</div>
        </div>
        <div class="cell">
          <div class="cell-main cell-label">Installed</div>
          <div class="cell-value">{formatRelative(installed.installed_at)}</div>
        </div>
        {expiry && (
          <div class="cell">
            <div class="cell-main cell-label">Signature</div>
            <div class={expiry.tone === "ok" ? "cell-value" : `cell-value tone-${expiry.tone}`}>
              {expiry.tone === "ok" ? `Until ${formatDate(installed.expires_at)}` : expiry.label}
            </div>
          </div>
        )}
        <div class="cell">
          <div class="cell-main cell-label">Installed by</div>
          <div class="cell-value">
            {installed.origin === "kiwi" ? "Kiwi" : "Another tool"}
            {installed.verified && ` · seen on ${deviceName(app.platform)}`}
          </div>
        </div>
      </div>
      {unconfirmed && <p class="section-footer">{unconfirmed}</p>}
    </section>
  );
}

/** Screenshots that couldn't be loaded are left out rather than shown broken; without any, so is the section. */
function Screenshots({ urls }: { urls: string[] }) {
  const [failed, setFailed] = useState<ReadonlySet<string>>(new Set());
  const shown = urls.filter((url) => !failed.has(url));
  if (shown.length === 0) return null;
  return (
    <section class="detail-section">
      <h2 class="section-heading">Preview</h2>
      <div class="shots">
        {shown.map((url) => (
          <img
            key={url}
            class="shot"
            src={url}
            alt={`Screenshot ${urls.indexOf(url) + 1}`}
            loading="lazy"
            referrerpolicy="no-referrer"
            onError={() => setFailed((f) => new Set(f).add(url))}
          />
        ))}
      </div>
    </section>
  );
}

/** Kiwi Builds apps link to the page with their patches and schedule. */
function BuildLink({ bundleId }: { bundleId: string }) {
  const overview = useResource("automation", api.automationOverview);
  const build = overview.data?.apps.find((a) => a.bundle_id === bundleId);
  return (
    <section class="detail-section">
      <div class="cells">
        <LinkCell
          href={build ? buildPath(build.id) : BUILDS_PATH}
          icon="wrench"
          label="Patches and Schedule"
          sub="Built on your Mac with Kiwi Builds"
        />
      </div>
    </section>
  );
}

function Releases({ app, onState }: { app: AppDetail; onState: (app: AppSummary, state: AppState) => void }) {
  if (app.releases.length === 0) return null;
  return (
    <section class="detail-section" id="versions">
      <h2 class="section-heading">Version History</h2>
      <ul class="release-list">
        {app.releases.map((release) => (
          <li key={`${release.version}/${release.build ?? ""}/${release.revision ?? ""}`} class="release">
            <div class="release-head">
              <div>
                <div class="release-version">{formatVersion(release)}</div>
                <div class="release-meta">
                  {[release.date && formatDate(release.date), release.size && formatBytes(release.size), osRequirement(release, app.platform)]
                    .filter(Boolean)
                    .join(" · ")}
                </div>
              </div>
              <VersionButton app={app} release={release} onState={onState} />
            </div>
            {!release.compatible && release.incompatibility && (
              <p class="release-note tone-warn">{release.incompatibility}</p>
            )}
            {release.notes && <ExpandableText text={release.notes} lines={3} />}
          </li>
        ))}
      </ul>
    </section>
  );
}

function Details(props: {
  app: AppDetail;
  onState: (app: AppSummary, state: AppState) => void;
  onChange: (app: AppDetail) => void;
  onLink: () => void;
  reinstalling: boolean;
  onReinstallEnd: () => void;
}) {
  const { app, onState, reinstalling, onReinstallEnd } = props;
  const show = useTask(async () =>
    props.onChange(await api.updateApp(app.source_id, app.bundle_id, { hidden: false })),
  );
  const target = app.state.target ?? app.releases[0] ?? null;
  const dropped = app.state.build?.dropped_entitlements.length
    ? app.state.build.dropped_entitlements
    : app.dropped_entitlements;
  const replaces = replacesLabel(app.state);
  const change = versionChange(app.state);
  const android = app.platform === "android";
  const foreign = foreignInstallWarning(app);

  return (
    <article class="detail">
      <div class="app-hero">
        <header class="detail-header">
          <AppIcon url={app.icon_url} name={app.name} tint={app.tint_color} size={104} />
          <div class="detail-title">
            <h1>{app.name}</h1>
            {app.subtitle && <p class="detail-subtitle">{app.subtitle}</p>}
            <p class="detail-source">{[app.developer, app.source_name].filter(Boolean).join(" · ")}</p>
          </div>
        </header>
        {reinstalling ? (
          <ReinstallButton
            app={app}
            onState={onState}
            onDone={onReinstallEnd}
            large
            caption={false}
            render={(view, button) => (
              <div>
                <div class="detail-action">{button}</div>
                <ActionStatus view={view} platform={app.platform} />
                {foreign && <p class="action-status warn">{foreign}</p>}
                <p class="action-status">
                  <button type="button" class="text-button small" onClick={onReinstallEnd}>
                    Cancel Reinstall
                  </button>
                </p>
              </div>
            )}
          />
        ) : (
          <ActionButton
            app={app}
            onState={onState}
            large
            caption={false}
            render={(view, button) => (
              <div>
                <div class="detail-action">
                  {button}
                  {change && <VersionChange from={change.from} to={change.to} large />}
                </div>
                {/* Why it is unavailable shows once, below. */}
                {!(view.kind === "disabled" && view.caption === app.unavailable) && <ActionStatus view={view} platform={app.platform} />}
                {replaces && <p class="action-status">{replaces}</p>}
                {foreign && <p class="action-status warn">{foreign}</p>}
                {app.unavailable && <p class="action-status warn">{app.unavailable}</p>}
              </div>
            )}
          />
        )}
        {app.hidden && (
          <p class="hidden-note">
            <Icon name="eyeOff" size={15} />
            <span>Hidden from Browse</span>
            <button type="button" class="text-button small" onClick={() => void show.run()} disabled={show.pending}>
              Show
            </button>
            {show.error && <span class="danger">{show.error}</span>}
          </p>
        )}
      </div>

      <InfoStrip app={app} release={target} />

      {target?.notes && (
        <section class="detail-section">
          <div class="section-heading-row">
            <h2 class="section-heading">What's New</h2>
            {app.releases.length > 1 && (
              <button
                type="button"
                class="text-button small"
                onClick={() => document.getElementById("versions")?.scrollIntoView({ behavior: "smooth" })}
              >
                Version History
              </button>
            )}
          </div>
          <p class="muted small">
            Version {formatVersion(target)}
            {target.date && ` · ${formatRelative(target.date)}`}
          </p>
          <ExpandableText text={target.notes} lines={3} />
        </section>
      )}

      <Screenshots urls={app.screenshots} />

      {app.description && (
        <section class="detail-section">
          <h2 class="section-heading">About</h2>
          <ExpandableText text={app.description} lines={4} />
        </section>
      )}

      {app.source_id === BUILD_SOURCE_ID && <BuildLink bundleId={app.bundle_id} />}
      <OriginalSection app={app} onLink={props.onLink} />
      <OtherVersions app={app} />

      <InstalledCard app={app} />

      <Releases app={app} onState={onState} />

      {app.permissions.privacy.length > 0 && (
        <section class="detail-section">
          <h2 class="section-heading">Privacy</h2>
          <div class="cells">
            {app.permissions.privacy.map((p) => (
              <div key={p.key} class="cell cell-stacked">
                <div class="cell-label">{privacyLabel(p.key)}</div>
                <div class="cell-sub">{p.description}</div>
              </div>
            ))}
          </div>
        </section>
      )}

      <section class="detail-section">
        <h2 class="section-heading">Information</h2>
        <div class="cells">
          {app.developer && (
            <div class="cell">
              <div class="cell-main cell-label">Developer</div>
              <div class="cell-value">{app.developer}</div>
            </div>
          )}
          <div class="cell">
            <div class="cell-main cell-label">Source</div>
            <div class="cell-value">{app.source_name}</div>
          </div>
          {app.category && (
            <div class="cell">
              <div class="cell-main cell-label">Category</div>
              <div class="cell-value">{app.category}</div>
            </div>
          )}
          <div class="cell">
            <div class="cell-main cell-label">{android ? "Package" : "Bundle ID"}</div>
            <div class="cell-value mono">{app.bundle_id}</div>
          </div>
          {target && (
            <div class="cell">
              <div class="cell-main cell-label">Compatibility</div>
              <div class="cell-value">{target.compatible ? osRequirement(target, app.platform) : (target.incompatibility ?? "Incompatible")}</div>
            </div>
          )}
          {dropped.length > 0 && (
            <div class="cell cell-stacked">
              <div class="cell-label tone-warn">Removed When Signing</div>
              <div class="cell-sub">
                Your provisioning profile can't grant {dropped.length === 1 ? "this entitlement" : "these entitlements"}, so
                features that need {dropped.length === 1 ? "it" : "them"} may not work.
              </div>
              <ul class="mono-list">
                {dropped.map((e) => (
                  <li key={e}>{e}</li>
                ))}
              </ul>
            </div>
          )}
          {app.permissions.entitlements.length > 0 && (
            <details class="cell cell-stacked cell-details">
              <summary class="cell-label">Entitlements ({app.permissions.entitlements.length})</summary>
              <ul class="mono-list">
                {app.permissions.entitlements.map((e) => (
                  <li key={e} class={dropped.includes(e) ? "tone-warn" : undefined}>
                    {e}
                  </li>
                ))}
              </ul>
            </details>
          )}
        </div>
        {android ? (
          <p class="section-footer">
            Signed with this Kiwi's own key. Android installs updates over it only when they carry the same signature.
          </p>
        ) : (
          <p class="section-footer">
            Kiwi will install this app as <span class="mono">{app.signed_bundle_id}</span>, so it can live alongside the
            App Store version.
          </p>
        )}
      </section>
    </article>
  );
}

export function AppDetailScreen({ sourceId, bundleId }: { sourceId: string; bundleId: string }) {
  const detail = useResource(`app/${sourceId}/${bundleId}`, () => api.app(sourceId, bundleId));
  const { mutate } = detail;
  const onState = useCallback(
    (_app: AppSummary, state: AppState) => mutate((d) => d && { ...d, state }),
    [mutate],
  );
  const [linking, setLinking] = useState(false);
  const closeLink = useCallback(() => setLinking(false), []);
  const [reinstalling, setReinstalling] = useState(false);
  const endReinstall = useCallback(() => setReinstalling(false), []);
  useOnForeground(detail.reload);

  let content;
  if (detail.data) {
    content = (
      <Details
        app={detail.data}
        onState={onState}
        onChange={mutate}
        onLink={() => setLinking(true)}
        reinstalling={reinstalling}
        onReinstallEnd={endReinstall}
      />
    );
  } else if (detail.error instanceof ApiError && detail.error.status === 404) {
    content = (
      <Empty icon="inbox" title="App Not Found" message="It may have been removed from its source.">
        <a class="button" href="/">
          Browse Apps
        </a>
      </Empty>
    );
  } else if (detail.error) content = <ErrorState error={detail.error} onRetry={detail.reload} />;
  else content = <Loading />;

  // The top of the page takes the app's colour, darkened where white text needs it.
  const hero = detail.data ? heroColors(appColor(detail.data.name, detail.data.tint_color)) : null;
  return (
    <main class="page page-detail page-app" style={hero ? { "--hero": hero.base, "--hero-deep": hero.deep } : undefined}>
      <BackBar
        trailing={
          detail.data && (
            <AppMenu
              app={detail.data}
              onChange={mutate}
              onLink={() => setLinking(true)}
              onReinstall={!reinstalling && canReinstall(detail.data) ? () => setReinstalling(true) : null}
            />
          )
        }
      />
      {/* The last copy stays visible, but not as if it were current. */}
      {detail.data && detail.error && (
        <Banner level="error">
          {detail.error instanceof ApiError && detail.error.status === 404
            ? "This app is no longer in its source."
            : `Couldn't refresh: ${detail.error.message}`}{" "}
          <button type="button" class="text-button small" onClick={() => void detail.reload()}>
            Try Again
          </button>
        </Banner>
      )}
      {content}
      {linking && detail.data && <LinkSheet app={detail.data} onChange={mutate} onClose={closeLink} />}
    </main>
  );
}
