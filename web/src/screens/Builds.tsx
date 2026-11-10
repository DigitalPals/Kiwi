import { useEffect } from "preact/hooks";
import { api, ApiError } from "../api/client";
import type { AutomationState, SetupCheck } from "../api/types";
import { AppIcon } from "../components/AppIcon";
import { Icon } from "../components/Icon";
import { BackBar, Cell, Empty, ErrorState, LinkCell, Loading, Section, Spinner } from "../components/ui";
import { activeJob, BUILD_SOURCE_ID, statusLine } from "../lib/automation";
import { useOnForeground, useResource } from "../lib/hooks";
import { buildPath, sourcePath } from "../lib/routes";

export const SETUP_HELP: Record<string, string> = {
  "Mac build tools": "Install Xcode and Kiwi's build tools on the Mac.",
  "Tweak toolchain": "Install the patch compiler using Kiwi's Mac setup instructions.",
  "App Store sign-in": "Sign in to Kiwi's App Store downloader on the Mac. Apple may ask for a verification code.",
  "Private decryption enrollment": "Install and open an iPhone or iPad app from the Mac App Store, then enroll the Mac.",
  macOS: "iOS apps can only be built on a Mac. This Kiwi doesn't run on macOS, so it builds Android apps only.",
};

export function ExperimentalTag() {
  return <span class="tag tag-warn">Experimental</span>;
}

/** Tells an Android app from the iPhone app of the same name. */
export function AndroidTag() {
  return <span class="tag">Android</span>;
}

function BuildRow({ app }: { app: AutomationState }) {
  const active = activeJob(app);
  return (
    <a class="cell has-icon cell-link build-row" href={buildPath(app.id)}>
      <AppIcon url={app.icon_url} name={app.name} size={40} />
      <div class="cell-main">
        <div class="cell-label">{app.name} {app.platform === "android" && <AndroidTag />} {app.experimental && <ExperimentalTag />}</div>
        <div class="cell-sub">
          {app.added ? (
            <span class="build-status">{active?.status === "running" && <Spinner />}{statusLine(app)}</span>
          ) : app.summary}
        </div>
      </div>
      {app.added ? <Icon name="chevron" size={16} class="cell-chevron" /> : <span class="pill pill-small">Add</span>}
    </a>
  );
}

export function SetupCells({ checks }: { checks: SetupCheck[] }) {
  if (checks.length === 0) return <Cell label="Setup has not been checked" sub="Start a build to check the Mac." />;
  return (
    <>
      {checks.map((item) => (
        <Cell key={item.name} label={item.name} sub={!item.ready ? SETUP_HELP[item.name] : undefined}>
          <span class={item.ready ? "tone-ok" : "danger"}>{item.ready ? "Ready" : "Setup needed"}</span>
        </Cell>
      ))}
    </>
  );
}

export function Builds() {
  const overview = useResource("automation", api.automationOverview);
  const sources = useResource("sources", api.sources);
  const source = sources.data?.find((s) => s.id === BUILD_SOURCE_ID);
  const data = overview.data;
  const active = !!data?.apps.some((a) => activeJob(a));
  useOnForeground(() => void overview.reload());
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void overview.reload();
    }, active ? 3000 : 30_000);
    return () => window.clearInterval(timer);
  }, [active, overview.reload]);

  let content;
  if (overview.error instanceof ApiError && overview.error.code === "builds_off") {
    content = (
      <Empty icon="wrench" title="Kiwi Builds Is Off" message="Turn it on in Settings to patch App Store apps on this Mac.">
        <a class="button" href="/settings">
          Open Settings
        </a>
      </Empty>
    );
  } else if (!data) {
    content = overview.error ? <ErrorState error={overview.error} onRetry={overview.reload} /> : <Loading />;
  } else {
    const added = data.apps.filter((a) => a.added);
    const available = data.apps.filter((a) => !a.added && a.platform !== "android");
    const android = data.apps.filter((a) => !a.added && a.platform === "android");
    const ready = data.checks.length > 0 && data.checks.every((c) => c.ready);
    content = (
      <>
        {added.length > 0 && (
          <Section title="Your Apps" footer="One build runs at a time. Each app keeps its own schedule.">
            {added.map((app) => <BuildRow key={app.id} app={app} />)}
          </Section>
        )}
        {available.length > 0 && (
          <Section title="Add an App" footer="Downloaded with your Apple account, patched from pinned open-source tweaks and signed on the Mac.">
            {available.map((app) => <BuildRow key={app.id} app={app} />)}
          </Section>
        )}
        {android.length > 0 && (
          <Section title="Add an Android App" footer="Downloaded from Google Play, patched with pinned open-source patches and signed with this Kiwi's own key. Installs on Android phones and tablets.">
            {android.map((app) => <BuildRow key={app.id} app={app} />)}
          </Section>
        )}
        {source && (
          <Section title="In Kiwi">
            <LinkCell
              href={sourcePath(source.id)}
              label="Kiwi Builds Source"
              sub="Which built apps Browse shows"
              value={source.enabled ? "On" : "Off"}
            />
          </Section>
        )}
        <Section title="Mac" footer="Apple account sign-in and enrollment stay on the Mac. Android apps need neither.">
          {ready ? (
            <Cell label="Mac Setup" sub="Build tools, App Store sign-in, enrollment" icon="server">
              <span class="tone-ok">Ready</span>
            </Cell>
          ) : <SetupCells checks={data.checks} />}
        </Section>
      </>
    );
  }
  return (
    <main class="page">
      <BackBar label="Sources" fallback="/sources" />
      <header class="page-header">
        <h1 class="large-title">Kiwi Builds</h1>
      </header>
      <p class="page-summary">Patched and signed on your Mac</p>
      {content}
    </main>
  );
}
