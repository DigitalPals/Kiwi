# Using Kiwi

Kiwi has four tabs: **Browse**, **My Apps**, **Sources** and **Settings**. Use it from the
Home Screen (Safari › Share › Add to Home Screen); only the Home Screen app gets badges
and notifications.

## Sources

An AltStore source is a JSON feed of apps that someone publishes, usually as `apps.json` in
a GitHub repository. In **Sources**, tap **+** and paste its HTTPS URL. Raw GitHub links and
GitHub file (`blob`) links both work, as do modern feeds (`versions` arrays) and legacy ones
(app-level `version`, `downloadURL`, `versionDate`, `versionDescription`). Kiwi imports
every valid app in the feed and checks the source again on a schedule (Settings › Updates ›
**Check Sources Every**) or when you tap **Check Now**.

Open a source to choose what Browse shows from it:

- **All Apps**: everything except the apps you turn off.
- **Apps I Pick**: only the apps you turn on. Apps the source adds later wait there,
  marked New.

The source's page also turns it off (its apps disappear without being removed), refreshes
it, or removes it.

## Browse

Browse lists the apps from all your sources: **Fresh This Week** at the top, then every
app, filtered by source and sorted by name or by when they were last updated. Search finds apps by name and
developer, hidden ones included.

To hide an app, open it and choose **•••** › **Hide from Browse**. Hidden apps still show up
in searches and under "N apps hidden", and installed ones keep getting updates.

### An app's page

The page shows the release Kiwi would install on this iPhone (the newest one that supports
its iOS version), what's new, screenshots, version history, permissions and entitlements.
Under Information, **Removed When Signing** lists entitlements your profile can't grant;
features that need them may not work in the re-signed app.

**Original apps.** Patched apps show the App Store app they're based on (matched by bundle
ID, for example YouTube for YouTubePlus) and its current version, in amber when the App
Store is ahead. Kiwi looks the versions up in the App Store of your Mac's region; choose
another under Settings › **App Store Country**. When a patch uses its own bundle ID, link
the original by hand: **•••** › **Change Original App…**.

## Installing and updating

Tap **Get** (or **Install**/**Update**). Kiwi downloads the IPA, checks it, re-signs it for
this iPhone and hands it to iOS, which asks you to confirm. Progress shows on the button;
you can leave the page while Kiwi prepares the app.

- **Updates** appear in **My Apps** with a badge on the tab and on the Home Screen icon.
  iOS asks you to confirm each install, so **Update All** prepares them all and you tap
  **Update** on each one that's ready.
- **Prepare Updates Automatically** (Settings › Updates) downloads and signs updates in the
  background, so they install right away.
- **Remove Apple Watch Apps** (on by default) drops watch apps, which can't be re-signed for
  the phone alone, and makes installs smaller.
- Apps Kiwi signed carry a small ring around their icon in My Apps, which empties as their
  signature gets closer to expiring.
- **Edit** in My Apps lets you forget an app: Kiwi stops tracking it, and the app stays on
  the iPhone until you delete it there.
- **Reinstall** (**•••** on an app's page) installs an app again that Kiwi still lists as
  installed, for example after you deleted it on the iPhone. Kiwi only notices deleted apps
  when the iPhone is linked to the Mac (see [This iPhone and devicectl](#this-iphone-and-devicectl)).

## Signing and expiry

Re-signed apps work until the earlier of your certificate's and your profile's expiry,
which is usually about a year for a paid developer account. Settings › **Signing** shows
the identity and profile Kiwi uses, every profile it found and why a profile can't be used.
After you change certificates or profiles on the Mac, tap **Rescan Profiles and
Identities**.

When an app Kiwi signed has less than 30 days left and your current certificate and profile
last longer, Kiwi offers to **Re-sign** the same version with the later expiry date. With **Notify Before Apps
Expire** on, you get a notification 30, 7 and 1 days before an app expires.

## Notifications

In the Home Screen app, open Settings › **Notifications** and turn them on (iOS 16.4 or
later). Kiwi notifies you about updates, installs that are ready and apps that are about to
expire. **Send a Test** checks that it works.

## This iPhone and devicectl

Settings › This iPhone shows the name Kiwi uses, the iOS version (from Safari, the
installer, devicectl or typed in by hand) and the Tailscale name. Kiwi offers only the
releases that support that iOS version.

If the iPhone is paired with the Mac (connected once with a cable and trusted, with full
Xcode installed), link it under **Linked Mac Device**. Kiwi then reads its iOS version and
installed apps with `devicectl`, every 10 minutes by default (Settings › **Sync with
devicectl**). Installs from other stores show up in My Apps as "installed outside Kiwi".

## Kiwi Builds

Kiwi can also patch App Store apps on your Mac: YouTube, YouTube Music, Twitch, X and
Reddit, plus the experimental Gboard Rambler keyboard and Rambler PoC dictation app. It's
off by default; read [Kiwi Builds](kiwi-builds.md) before you turn it on under Settings ›
**Kiwi Builds**.
