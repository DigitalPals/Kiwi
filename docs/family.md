# Family

Kiwi can serve everyone on your tailnet. You (the **owner**) keep the sources, signing,
Kiwi Builds and settings; everyone else is a **member** who sees only the apps you pick
for them, once their iPhone is registered.

## Who is who

The owner is the Tailscale user who owns the Mac (or the login given with
`./install.sh --owner`). Every other Tailscale user who opens Kiwi is a member. Kiwi
recognizes people and devices through Tailscale, so there are no accounts or passwords.

## Adding someone

1. **Give them access to the Mac in Tailscale.** In the admin console, open Machines › your
   Mac › **Share…**, or invite them to your tailnet. They install Tailscale on their iPhone
   and sign in.
2. **They register their iPhone.** They open Kiwi's address in Safari and tap **Register
   This iPhone**, allow the download, then install it under Settings › **Profile
   Downloaded**. iOS sends the iPhone's UDID, model and name to Kiwi. The profile only does
   that; nothing stays installed.
3. **You approve.** You get a notification ("Register Noah's iPhone?"). Open Settings ›
   Family, pick their apps and approve.
   - **Apps You Add Later** lets apps that appear in your sources later through
     automatically.
   - You can also switch an app per person from the app's page (**Who Can Install**).
4. **Kiwi registers the iPhone with Apple** (see below), then tells it that it's ready.
5. **They install.** They add Kiwi to their Home Screen and install their apps. The first
   time they open one, iOS asks for Developer Mode once (Settings › Privacy & Security ›
   Developer Mode, restart, **Turn On**). Kiwi shows these steps.

## Registering iPhones with Apple

iOS only installs an app when the iPhone is in the profile it was signed with. There are
two ways to get a member's iPhone into your profile.

### With an App Store Connect key (automatic)

Kiwi registers approved iPhones in your developer account and creates its own development
profiles, named "Kiwi * …", that include every registered device.

1. In [App Store Connect](https://appstoreconnect.apple.com/access/integrations/api), open
   Users and Access › Integrations › App Store Connect API and create a **Team Key** with
   the **Admin** role.
2. Download the `.p8` file. Apple lets you download it only once, so keep a copy somewhere
   safe.
3. In Kiwi, open Settings › Family › **App Store Connect** and enter the key with its
   issuer ID and key ID. Kiwi checks it with Apple before it saves it.

The key is stored as `appstoreconnect.json` (mode 0600) in Kiwi's data directory, and the
profiles Kiwi makes go to `profiles/` there. Kiwi never deletes devices or profiles at Apple.

### By hand

Open the member's iPhone under Settings › Family. Its page shows the UDID. Add it in the
[developer portal](https://developer.apple.com/account/resources/devices/list) (Devices
› **+**), edit your wildcard development profile to include it, download the profile and
double-click it on the Mac. Then tap **Rescan** on the device's page (or Settings › Signing
› **Rescan Profiles and Identities**).

## Limits and removing people

- An Apple Developer membership can register 100 iPhones per year. A removed device frees
  its slot only when the membership renews.
- Removing a person, or turning an app off for them, stops updates. Installed copies keep
  working until their signature expires.
- Removing a person forgets their registration; their iPhone has to register again to come
  back.
