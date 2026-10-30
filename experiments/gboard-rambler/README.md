# Gboard Rambler (experimental)

A native Swift client for the speech service Android Gboard uses for **Rambler**
dictation: speech-to-text that removes filler words and applies spoken corrections
("Tuesday, actually no, Wednesday" becomes "Wednesday"). This directory has:

- **Rambler PoC**, a small standalone iPhone dictation app: record, finish, copy the
  corrected text;
- a macOS command-line client and protocol tests;
- **Gboard Rambler**, native hooks that put the same client into iOS Gboard 2.3.19's
  keyboard.

Kiwi Builds builds both apps as the experimental `rambler` and `gboard` recipes; see
[Kiwi Builds](../../docs/kiwi-builds.md). You don't need anything here to use Kiwi.

> [!WARNING]
> This talks to a private, undocumented Google service with the caller metadata that
> Google ships inside its own Android Gboard app. It is not affiliated with or endorsed
> by Google, may break or be blocked at any time, and may be against Google's terms.
> Your audio is sent to Google. Use it at your own risk, for yourself only.

## Credentials

No key is committed. `extract-caller.py` reads the three caller headers
(`X-Goog-Api-Key`, `X-Android-Package`, `X-Android-Cert`) from one specific APK,
[PixelBoard 1.3.3](https://github.com/Akshayykadam/PixelBoard/releases/tag/1.3.3)
(SHA-256 `7687e2cf5e72f6a6cab33adb94b60fd5af28b8656d30d64373e4f02c02d989c5`), by reading
its DEX bytecode. It doesn't run Android code, accepts no other file, writes the result
with mode 0600, never prints the values and refuses to overwrite a file.

No Google account, personal API key or paid subscription is involved. Keep the APK,
the caller configuration, audio and built apps in a private directory outside the
repository: the built apps contain the configuration, so don't share them.

## Build by hand

Needs an Apple Silicon Mac with full Xcode and Python 3.11 or later. There are no Swift
packages, Android libraries or model files. From this directory:

```sh
private_dir="$HOME/rambler-private"
mkdir -p "$private_dir" && chmod 700 "$private_dir"
curl --fail --location \
  https://github.com/Akshayykadam/PixelBoard/releases/download/1.3.3/PixelBoard-1.3.3.apk \
  --output "$private_dir/PixelBoard-1.3.3.apk"
python3 extract-caller.py "$private_dir/PixelBoard-1.3.3.apk" "$private_dir/caller.json"
./make-sample.sh "$private_dir/sample.wav"     # synthetic speech with a spoken correction

./build.sh test "$private_dir"                 # protocol tests
./build.sh cli "$private_dir"                  # macOS client
"$private_dir/rambler-cli" "$private_dir/caller.json" "$private_dir/sample.wav"
"$private_dir/rambler-cli" "$private_dir/caller.json" "$private_dir/sample.wav" --standard

./build.sh simulator "$private_dir/simulator" "$private_dir/caller.json" "$private_dir/sample.wav"
./build.sh device "$private_dir/device" "$private_dir/caller.json" "$private_dir/sample.wav"
```

With the sample, Rambler returns "Let us meet on Wednesday at 2:00 in the afternoon.
Please bring the budget report." `--standard` (plain recognition) keeps the false start
and the fillers.

`device` produces `RamblerPoC.ipa` (bundle `com.kiwi.ramblerpoc`, iOS 16 or later) with
only an ad-hoc signature. Kiwi re-signs it with your identity and profile; the `rambler`
recipe does all of this for you and publishes the app in the Kiwi Builds source.

## Protocol

- Endpoint `speechs3proto2-pa.googleapis.com`, bidirectional gRPC method
  `/speech.s3.S3StubbyClientConnectorService/S3Bidi`, sent with Foundation over HTTPS.
- Rambler uses the service `beyond-a2a-recognizer` and the routing key
  `gboard_gemini_v3_streaming_voice_edit_mul`.
- Audio is mono 16 kHz, encoded with Apple's Opus encoder and wrapped in Ogg (audio
  encoding 10 is Ogg Opus, not raw PCM).
- A result counts only with a non-empty edited-history extension and the service's
  completion message; HTTP success alone is not enough. Errors, empty speech and
  malformed frames are rejected, unknown protobuf fields ignored.
- The model runs on Google's servers; this is an online client only. Live partial
  results, cursor-aware edits, voice actions and offline recognition are not implemented.

## Gboard integration

`Gboard/Hooks.m` takes over keyboard voice sessions in Gboard's
`GSKSpeechToTextStreamingController`; `Gboard/Session.swift` records in the Gboard app
and hands the result back through Gboard's existing keyboard bridge, which inserts it.
Recording stops when you tap Stop or after a second of silence (an on-device Apple sound
classifier tells speech from noise), with a two-minute limit. The whole recording is sent,
with a 100 Hz high-pass filter against wind and handling rumble.

`inject-gboard.py` accepts only the inspected Gboard 2.3.19 (build 2.3.19.444584732)
images, refuses encrypted or changed code, keeps the keyboard extension and loads the
client into both the app and the keyboard.

**Signing.** Kiwi renames Gboard's App Groups to `group.com.kiwi.gboard.<TEAM>.keyboard`
and `group.com.kiwi.gboard.<TEAM>.common`. The app's and the keyboard's profiles must
both grant both groups (a wildcard profile can't): register the team-suffixed app and
keyboard ids with those App Groups in Xcode or the developer portal, then rescan in
**Settings › Signing** in Kiwi. Signing fails rather than drop the groups. Gboard's URLs
use Kiwi's own scheme, so the App Store Gboard can stay installed next to it.

**On the iPhone.** Install Gboard Rambler through Kiwi and open it once to allow the
microphone. Add its keyboard in **Settings › General › Keyboard › Keyboards** and turn on
**Allow Full Access**. Then tap Gboard's microphone and speak; pause or tap Stop.

- iOS doesn't let keyboards use the microphone, so Gboard opens to record. When iOS
  offers no way back, a **Swipe back to your app** screen says so; swipe back and the
  text is inserted.
- **Rambler** in the Gboard app chooses Rambler or standard recognition, runs a
  synthetic correction test and has **Copy text** to recover the last result.
- **Dictate in keyboard** (off by default) records in the background after the first
  activation, so you can speak while back in your app. It may still need a manual switch
  back for a new recording.
- **Diagnostics › Copy report** records recorder and keyboard hand-off events and audio
  levels, without audio, transcripts or keys, for when an attempt fails.

## Tests

- `./build.sh test` runs the protocol tests: corrected and plain results, missing
  completion, empty results, gRPC and application errors, malformed frames and header
  validation.
- `test-speech.sh OUTPUT SAMPLE.wav [CALLER.json]` tests silence detection, end-of-speech
  timing, the filter and that the full recording is kept, with synthetic audio. With a
  caller configuration it also checks the result against the live service.
- `test-gboard.sh OUTPUT CALLER.json SAMPLE.wav` runs those checks and builds
  `GboardHarness.app`, an iOS Simulator harness around the real client with stand-in
  classes for Gboard's. Install and open it in a simulator; it checks insertion,
  cancellation, App Group remapping, URL routing and clean-up. It cannot run Google's
  device-only Gboard binary, so it doesn't replace a test on an iPhone.

`./build-gboard.sh OUTPUT [iphoneos|iphonesimulator]` builds only the client library.

## Status

Experimental. The Rambler PoC works in the iOS Simulator; recording on an iPhone hasn't
been tried yet. Gboard Rambler's recording, insertion into other apps (also with
**Dictate in keyboard**) and keyboard clean-up work on an iPhone; the swipe-back screen
has only been tested in the simulator.
