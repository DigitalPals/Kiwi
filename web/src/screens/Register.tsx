import { useEffect, useState } from "preact/hooks";
import { api } from "../api/client";
import type { Me } from "../api/types";
import { StepCell } from "../components/family";
import { KiwiLogo } from "../components/KiwiLogo";
import { Banner, Cell, Spinner } from "../components/ui";
import { gateStep, modelName } from "../lib/family";
import { formatRelative } from "../lib/format";
import { useTask } from "../lib/hooks";
import { pushSupport, subscribe, subscriptionBody, usableSubscription } from "../lib/push";
import { isStandalone } from "../lib/pwa";

/** Polls while the server decides (the owner approves, Apple registers). */
const POLL_MS = 5000;

/** Back from Settings: iOS posted the ID and Safari opened `/?enrolled=1`. */
function enrolledParam(): boolean {
  return new URLSearchParams(location.search).has("enrolled");
}

function Welcome({ me, declined }: { me: Me; declined: boolean }) {
  const owner = me.access.owner_name;
  const [started, setStarted] = useState(false);
  const enroll = useTask(async () => {
    const { url } = await api.enroll();
    setStarted(true);
    // Safari downloads the profile and offers to review it in Settings.
    location.href = url;
  });
  return (
    <>
      <div class="welcome">
        <KiwiLogo size={76} />
        <h1 class="state-title">{declined ? "Not Approved" : "Welcome to Kiwi"}</h1>
        <p class="state-message">
          {declined
            ? `${owner} didn't approve this iPhone. If that was a mistake, ask ${owner} and register again.`
            : `Register this iPhone once to install apps from ${owner}'s Kiwi. It takes about a minute.`}
        </p>
      </div>
      {started && (
        <Banner level="info" icon="settings">
          Now open the <strong>Settings</strong> app, tap <strong>Profile Downloaded</strong>, then <strong>Install</strong>. Safari comes
          back here by itself.
        </Banner>
      )}
      <div class="cells steps">
        <StepCell n={1} state="todo" label="Allow the download" sub="A setup profile tells Kiwi this iPhone's ID. Nothing stays installed." />
        <StepCell n={2} state="todo" label="Install it in Settings" sub="Settings › Profile Downloaded › Install" />
        <StepCell n={3} state="todo" label={`${owner} approves`} sub="Then you can install apps." />
      </div>
      <button type="button" class="button button-block" onClick={() => void enroll.run()} disabled={enroll.pending}>
        {enroll.pending ? <Spinner /> : declined ? "Ask Again" : started ? "Download Again" : "Register This iPhone"}
      </button>
      {enroll.error && <p class="form-error">{enroll.error}</p>}
      {isStandalone() && (
        <p class="fine-print">If nothing happens, open this page in Safari and register there.</p>
      )}
      {me.access.person && <p class="fine-print">Connected through Tailscale as {me.access.person.id}</p>}
    </>
  );
}

/** "Notify me": a subscription now delivers "This iPhone is ready" later. */
function NotifyCell({ me, reload }: { me: Me; reload: () => void }) {
  const [supported, setSupported] = useState(false);
  const [on, setOn] = useState(me.push.subscribed);
  useEffect(() => {
    void pushSupport().then((s) => setSupported(s === "supported"));
    void usableSubscription(me.push.vapid_public_key)
      .then((sub) => sub && setOn(true))
      .catch(() => {});
  }, [me.push.vapid_public_key]);
  const enable = useTask(async () => {
    const sub = await subscribe(me.push.vapid_public_key);
    await api.subscribePush(subscriptionBody(sub));
    setOn(true);
    reload();
  });
  if (!supported || !me.push.vapid_public_key) return null;
  return (
    <section class="section" style={{ marginTop: "18px" }}>
      <div class="cells">
        <Cell label="Notify Me" icon="bell" sub={enable.error ?? "When this iPhone is ready"}>
          {on ? (
            <span class="tone-ok">On</span>
          ) : (
            <button type="button" class="pill pill-filled" onClick={() => void enable.run()} disabled={enable.pending}>
              {enable.pending ? "Enabling…" : "Enable"}
            </button>
          )}
        </Cell>
      </div>
    </section>
  );
}

function Waiting({ me, reload }: { me: Me; reload: () => void }) {
  const owner = me.access.owner_name;
  const reg = me.registration;
  const received = reg.status === "pending" || reg.status === "approved";
  const approved = reg.status === "approved";
  const device = [modelName(reg.product ?? me.device.model), me.device.os_version && `iOS ${me.device.os_version}`]
    .filter(Boolean)
    .join(" · ");
  return (
    <>
      <div class="welcome">
        <KiwiLogo size={60} />
        <h1 class="state-title">Almost There</h1>
        <p class="state-message">
          {approved
            ? `${owner} approved this iPhone. Kiwi is adding it to its signing profile.`
            : `${owner} gets a notification to approve this iPhone.`}
        </p>
      </div>
      <div class="cells steps">
        <StepCell
          n={1}
          state={received ? "done" : "busy"}
          label={received ? "iPhone ID received" : "Waiting for the iPhone's ID"}
          sub={received ? device : "Install the profile in Settings"}
        />
        <StepCell
          n={2}
          state={approved ? "done" : received ? "busy" : "off"}
          label={approved ? `${owner} approved` : `Waiting for ${owner}`}
          sub={
            approved
              ? reg.decided_at && formatRelative(reg.decided_at)
              : reg.requested_at
                ? `Asked ${formatRelative(reg.requested_at)}`
                : undefined
          }
        />
        <StepCell
          n={3}
          state={approved ? "busy" : "off"}
          label="Registered with Apple"
          sub={approved ? `${owner} or Kiwi adds it to the developer account` : "Kiwi does this right after approval"}
        />
      </div>
      {!isStandalone() ? (
        <Banner level="info" icon="share">
          While you wait, tap <strong>Share › Add to Home Screen</strong> and open Kiwi from there. It tells you when this iPhone is
          ready.
        </Banner>
      ) : (
        <NotifyCell me={me} reload={reload} />
      )}
    </>
  );
}

/** What a member sees until their iPhone may install apps: register, then wait for approval. */
export function RegistrationGate({ me, reload }: { me: Me; reload: () => void }) {
  const [enrolled] = useState(enrolledParam);
  const step = gateStep(me.registration, enrolled);
  useEffect(() => {
    if (!enrolled) return;
    // Remembered above; keep the address clean for the Home Screen and later reloads.
    history.replaceState(history.state, "", location.pathname);
    reload();
  }, [enrolled, reload]);
  useEffect(() => {
    if (step !== "waiting") return;
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") reload();
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [step, reload]);
  return (
    <main class="page">
      {step === "waiting" ? <Waiting me={me} reload={reload} /> : <Welcome me={me} declined={step === "declined"} />}
    </main>
  );
}
