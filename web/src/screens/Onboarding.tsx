import type { ComponentChildren } from "preact";
import { useEffect, useState } from "preact/hooks";
import { api } from "../api/client";
import type { AndroidApp, Me, Platform, Signing } from "../api/types";
import { KiwiAppIcon } from "../components/AppIcon";
import { Icon, type IconName } from "../components/Icon";
import { KiwiLogo } from "../components/KiwiLogo";
import { BackBar, Cell, CellIcon, Section, Spinner } from "../components/ui";
import { formatBytes } from "../lib/format";
import { useResource, useTask } from "../lib/hooks";
import { useMe } from "../lib/me";
import { initialChoice, isUndetected, setupPage, signingRow, type SetupPage } from "../lib/onboarding";
import { blockedMessage, pushSupport, subscribe, subscriptionBody, type PushSupport } from "../lib/push";
import { isStandalone } from "../lib/pwa";

const host = (me: Me) => me.server.public_url.replace(/^https?:\/\//, "").replace(/\/$/, "");

/** One of the two device types, as a large radio card. */
function Choice(props: {
  on: boolean;
  icon: IconName;
  color: string;
  title: string;
  sub: string;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={props.on}
      class={props.on ? "choice on" : "choice"}
      onClick={props.onSelect}
    >
      <span class="choice-tile" style={{ "--tile": props.color }} aria-hidden="true">
        <Icon name={props.icon} size={26} />
      </span>
      <span class="choice-text">
        <b>{props.title}</b>
        <span class="choice-sub">{props.sub}</span>
      </span>
      <span class="choice-radio" aria-hidden="true">
        {props.on && <Icon name="check" size={15} strokeWidth={3} />}
      </span>
    </button>
  );
}

/** The first question: iPhone or Android. A desktop browser gets it too, in a wider layout. */
function ChooseDevice(props: { me: Me; pending: boolean; error: string | null; onContinue: (platform: Platform) => void }) {
  const { me } = props;
  const [choice, setChoice] = useState<Platform | null>(() => initialChoice(me.device));
  const undetected = isUndetected(me.device);
  const detected = me.device.detected_platform;
  let question: [string, string];
  if (undetected) question = ["Which device do you use Kiwi on?", "Kiwi can’t tell from this browser. It shows the apps for the device you pick."];
  else if (me.device.platform_source === "chosen") question = ["Which device is this?", "Kiwi shows the apps that work on it."];
  else question = [detected === "android" ? "This looks like an Android device" : "This looks like an iPhone", "Kiwi shows the apps that work on it."];
  const kiwiApp = me.android_app !== null;

  return (
    <main class={undetected ? "page page-glow onboarding welcome welcome-wide" : "page page-glow onboarding welcome"}>
      <KiwiLogo size={undetected ? 76 : 92} />
      <h1 class="welcome-title">Welcome to Kiwi</h1>
      {!undetected && (
        <p class="welcome-lead">
          Your own app store on <span class="welcome-host">{host(me)}</span>
        </p>
      )}
      <p class="welcome-question">
        <b>{question[0]}</b>
        {question[1]}
      </p>
      <div class="choices" role="radiogroup" aria-label="Device type">
        <Choice
          on={choice === "ios"}
          icon="phone"
          color="#007aff"
          title="iPhone or iPad"
          sub={undetected ? "Apps from sources and Kiwi Builds" : "Apps from sources and Kiwi Builds, signed for this device"}
          onSelect={() => setChoice("ios")}
        />
        <Choice
          on={choice === "android"}
          icon="android"
          color="#2f9e5e"
          title="Android phone or tablet"
          sub={
            !kiwiApp
              ? "Android apps from Kiwi Builds"
              : undetected
                ? "Android apps and the Kiwi app"
                : "Android apps from Kiwi Builds and the Kiwi app"
          }
          onSelect={() => setChoice("android")}
        />
      </div>
      <button
        type="button"
        class="button button-block welcome-continue"
        disabled={!choice || props.pending}
        onClick={() => choice && props.onContinue(choice)}
      >
        {props.pending ? <Spinner /> : "Continue"}
      </button>
      {props.error && <p class="form-error welcome-error">{props.error}</p>}
      <p class="fine-print">You can change this later in Settings.</p>
    </main>
  );
}

/** A numbered step; `children` may hold `Key`s. */
function Step({ n, children }: { n: number; children: ComponentChildren }) {
  return (
    <div class="cell step">
      <span class="step-number" aria-hidden="true">
        {n}
      </span>
      <span class="step-text">{children}</span>
    </div>
  );
}

/** A button the steps refer to, e.g. Share. */
function Key({ icon, label, title }: { icon: IconName; label?: string; title?: string }) {
  return (
    <span class="key" title={title}>
      <Icon name={icon} size={label ? 14 : 15} />
      {label}
      {!label && title && <span class="visually-hidden">{title}</span>}
    </span>
  );
}

/** Notifications in the setup: the Home Screen app can turn them on; Safari can't. */
function NotificationsRow({ me, homeScreen }: { me: Me; homeScreen: boolean }) {
  const { reload } = useMe();
  const [support, setSupport] = useState<PushSupport | null>(null);
  useEffect(() => {
    if (homeScreen) void pushSupport().then(setSupport);
  }, [homeScreen]);
  const enable = useTask(async () => {
    const sub = await subscribe(me.push.vapid_public_key);
    await api.subscribePush(subscriptionBody(sub));
    reload();
  });

  let sub: ComponentChildren = "Turn them on in the Home Screen app: updates, ready installs, expiry";
  let value: ComponentChildren = undefined;
  if (homeScreen) {
    sub = "Updates, ready installs and expiry warnings";
    if (me.push.subscribed) value = <span class="tone-ok">On</span>;
    else if (!me.push.vapid_public_key) sub = "The server has no push key configured.";
    else if (support === null) value = <Spinner />;
    else if (support === "denied") sub = blockedMessage();
    else if (support === "supported") {
      value = (
        <button type="button" class="pill" onClick={() => void enable.run()} disabled={enable.pending}>
          {enable.pending ? <Spinner /> : "Turn On"}
        </button>
      );
    } else if (support === "no-service-worker") sub = "Kiwi's service worker isn't running yet. Turn them on later in Settings.";
    else sub = "This browser can't show notifications from Kiwi.";
  }
  return (
    <Cell icon="bell" label="Notifications" sub={enable.error ? <span class="danger">{enable.error}</span> : sub}>
      {value}
    </Cell>
  );
}

/** Whether this Kiwi can sign apps for the iPhone, and until when. */
function SigningRow({ me }: { me: Me }) {
  const signing = useResource<Signing>("signing", api.signing);
  // Unknown (couldn't load) reads like "checking" without a spinner.
  const active = signing.data ? signing.data.active : undefined;
  const row = signingRow(me.warnings, active);
  const warn = row.tone === "warn";
  return (
    <div class="cell has-icon">
      <CellIcon name={warn ? "warning" : "shield"} color={warn ? "#ff9500" : undefined} />
      <div class="cell-main">
        <div class={warn ? "cell-label tone-warn" : "cell-label"}>{row.title}</div>
        {row.sub && <div class="cell-sub">{row.sub}</div>}
      </div>
      {row.tone === "off" ? (signing.error ? null : <Spinner />) : <span class={warn ? "dot dot-warn" : "dot"} aria-hidden="true" />}
    </div>
  );
}

/** iPhone: Add to Home Screen, notifications and signing. */
function IphoneSetup(props: { me: Me; onBack: () => void; onDone: () => void; pending: boolean; error: string | null }) {
  const { me } = props;
  const homeScreen = isStandalone();
  return (
    <main class="page page-glow onboarding">
      <BackBar label="Back" onBack={props.onBack} />
      <header class="page-header">
        <h1 class="large-title">Set Up This iPhone</h1>
      </header>
      <p class="page-summary">
        {homeScreen ? "Kiwi is on your Home Screen. Turn on notifications to hear about updates." : "Kiwi works best as an app on your Home Screen."}
      </p>
      {!homeScreen && (
        <Section title="Add to Home Screen">
          <Step n={1}>
            Tap <Key icon="more" title="More" /> in Safari, then <Key icon="share" label="Share" />
          </Step>
          <Step n={2}>
            Choose <Key icon="addSquare" label="Add to Home Screen" />
          </Step>
          <Step n={3}>
            Open <b>Kiwi</b> from your Home Screen
          </Step>
        </Section>
      )}
      <Section title={homeScreen ? undefined : "Then"}>
        <NotificationsRow me={me} homeScreen={homeScreen} />
        <SigningRow me={me} />
      </Section>
      <button type="button" class="button button-block button-secondary" onClick={props.onDone} disabled={props.pending}>
        {props.pending ? <Spinner /> : homeScreen ? "Done" : "Continue in Safari"}
      </button>
      {props.error && <p class="form-error welcome-error">{props.error}</p>}
    </main>
  );
}

/** Android: the Kiwi app first, the browser as the other choice. */
function AndroidSetup(props: {
  app: AndroidApp;
  onBack: () => void;
  onDownload: () => void;
  onDone: () => void;
  pending: boolean;
  error: string | null;
}) {
  const { app } = props;
  return (
    <main class="page page-glow onboarding">
      <BackBar label="Back" onBack={props.onBack} />
      <header class="page-header">
        <h1 class="large-title">Set Up This Phone</h1>
      </header>
      <p class="page-summary">On Android, the Kiwi app installs and updates your apps in the background.</p>
      <div class="get-app">
        <div class="get-app-info">
          <KiwiAppIcon size={60} />
          <div class="cell-main">
            <div class="get-app-name">Get the Kiwi app</div>
            <div class="get-app-detail">
              Kiwi for Android · Version {app.version_name} · {formatBytes(app.size)}
            </div>
            <div class="get-app-detail">Signed by this Kiwi</div>
          </div>
        </div>
        <a class="button button-block" href={app.install_url} onClick={props.onDownload}>
          <Icon name="download" size={20} strokeWidth={2.2} />
          Download Kiwi for Android
        </a>
      </div>
      <Section title="Then">
        <Step n={1}>
          Open <b>kiwi.apk</b> from the download
        </Step>
        <Step n={2}>Allow installs from Chrome when Android asks, then tap Install</Step>
        <Step n={3}>Open Kiwi and connect to this server</Step>
      </Section>
      <button type="button" class="button button-block button-secondary" onClick={props.onDone} disabled={props.pending}>
        {props.pending ? <Spinner /> : "Continue in the Browser"}
      </button>
      {props.error && <p class="form-error welcome-error">{props.error}</p>}
      <p class="fine-print">Tailscale must be on for the app to reach Kiwi.</p>
    </main>
  );
}

/**
 * The first-run setup, shown instead of the tabs until this device is set up: which
 * platform it is (saved on the server for the device, so the Home Screen app and Kiwi's
 * Android app know it too), then that platform's setup.
 */
export function Onboarding({ me }: { me: Me }) {
  const { setMe } = useMe();
  const [page, setPage] = useState<SetupPage | null>(null);

  // The choice is sent even when it matches the detection, so it becomes the owner's.
  const choose = useTask(async (platform: Platform) => {
    const next = setupPage(platform, me);
    const updated = await api.updateMe(next ? { platform } : { platform, onboarded: true });
    setMe(updated);
    setPage(next);
  });
  const finish = useTask(async () => setMe(await api.updateMe({ onboarded: true })));
  // Downloading the Kiwi app completes the setup; the page stays for its steps.
  const downloaded = () => void api.updateMe({ onboarded: true }).catch(() => {});

  useEffect(() => window.scrollTo(0, 0), [page]);

  const back = () => {
    finish.setError(null);
    setPage(null);
  };
  if (page === "iphone") {
    return <IphoneSetup me={me} onBack={back} onDone={() => void finish.run()} pending={finish.pending} error={finish.error} />;
  }
  if (page === "android" && me.android_app) {
    return (
      <AndroidSetup
        app={me.android_app}
        onBack={back}
        onDownload={downloaded}
        onDone={() => void finish.run()}
        pending={finish.pending}
        error={finish.error}
      />
    );
  }
  return <ChooseDevice me={me} pending={choose.pending} error={choose.error} onContinue={(p) => void choose.run(p)} />;
}
