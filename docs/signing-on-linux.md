# Signing on Linux

On macOS Kiwi signs with the certificate in your login keychain and the profiles Xcode
downloads. Everywhere else it signs with files you give it: your **Apple Development**
certificate with its private key as a `.p12` file, and a **development provisioning
profile**. Kiwi keeps them in its data directory under `signing/` (the identity and its
password readable by Kiwi's user only) and signs with rcodesign's library instead of
Apple's `codesign`.

You need an [Apple Developer Program](https://developer.apple.com/programs/) membership,
as on a Mac: free accounts can't create the wildcard profile Kiwi signs with. Register
each iPhone's UDID and create the wildcard profile as described in
[Installing Kiwi](install.md#2-signing-certificate-and-profile); the steps on
developer.apple.com are the same. Without a Mac, `idevice_id -l` from
[libimobiledevice](https://libimobiledevice.org) prints the UDID of an iPhone connected
with a cable.

## The certificate as a `.p12`

### With a Mac

1. If you don't have one yet, create the certificate in Xcode › Settings › Accounts ›
   your team › **Manage Certificates…** › **+** › **Apple Development**.
2. Open **Keychain Access**, choose the **login** keychain and **My Certificates**. Find
   **Apple Development: Your Name (…)**; the triangle next to it shows its private key.
3. Right-click the certificate, choose **Export…**, keep the format **Personal
   Information Exchange (.p12)**, save it and set a password.

### Without a Mac

Create the key and a certificate signing request with OpenSSL, let Apple sign it, then
combine both into a `.p12`:

```sh
openssl genrsa -out kiwi-development.key 2048
openssl req -new -key kiwi-development.key -out kiwi-development.csr \
  -subj "/emailAddress=you@example.com/CN=Your Name/C=NL"
```

On [developer.apple.com](https://developer.apple.com/account/resources/certificates/list),
open **Certificates › +**, choose **Apple Development**, upload `kiwi-development.csr` and
download the certificate (`development.cer`). Then:

```sh
openssl x509 -inform DER -in development.cer -out development.pem
openssl pkcs12 -export -inkey kiwi-development.key -in development.pem \
  -out kiwi-development.p12 -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1
```

The last three options choose the encryption Keychain Access uses. OpenSSL 3 encrypts with
AES by default, which Kiwi can't open yet; it says so when you upload such a file. Keep
`kiwi-development.key` safe or delete it once the `.p12` works: it is your signing key.

## The profile

Download the wildcard development profile (the `.mobileprovision` file) from
**Profiles** on developer.apple.com. It must include the certificate above and your
iPhones. When you add an iPhone later, edit the profile there, download it again and
upload it again.

## Uploading

Open Kiwi on your phone or computer and go to **Settings › Signing**. Until both files are
uploaded it says **Signing Isn't Set Up**.

1. Under **Signing Identity**, tap **Upload .p12…**, choose the `.p12` file and enter the
   password you set when you exported it. Kiwi checks the file before it keeps it: a wrong
   password, a certificate without its private key, an expired certificate or a `.p12`
   encrypted with AES (see above) is refused with a message that says what to do.
2. Under **Provisioning Profiles**, tap **Upload Profile…** and choose the
   `.mobileprovision` file.

The page then says **Ready to Sign** and shows the identity (**In Use**) and the profiles.
Uploading another identity replaces the first: when the certificate is about to expire,
the page offers **Upload a New .p12…**. **Remove Identity** and a profile's **Remove**
delete the uploads again (Xcode's profiles on a Mac can't be removed there). A `.p12` may
be at most 64 KB, a profile 1 MB.

### With curl

You can also upload the files on the Kiwi host itself (requests from the host need no
Tailscale identity; `8742` is Kiwi's default port). The script reads the password from the
terminal, so it does not end up in your shell history:

```sh
python3 - kiwi-development.p12 <<'EOF' | curl -sS http://127.0.0.1:8742/api/signing/identity \
    -H 'X-Kiwi: 1' -H 'Content-Type: application/json' --data-binary @-
import base64, getpass, json, sys
p12 = base64.b64encode(open(sys.argv[1], "rb").read()).decode()
print(json.dumps({"p12": p12, "password": getpass.getpass("Password of the .p12: ")}))
EOF

python3 - Kiwi_Wildcard.mobileprovision <<'EOF' | curl -sS http://127.0.0.1:8742/api/signing/profiles \
    -H 'X-Kiwi: 1' -H 'Content-Type: application/json' --data-binary @-
import base64, json, sys
print(json.dumps({"profile": base64.b64encode(open(sys.argv[1], "rb").read()).decode()}))
EOF
```

Each answers with Kiwi's signing state ([`Signing`](api.md#signing-uploads)): `error` is
`null` once Kiwi can sign. `DELETE /api/signing/identity` and
`DELETE /api/signing/profiles/<uuid>` remove the uploads.

A wildcard profile can't grant capabilities such as push notifications or App Groups, as
on a Mac. If you upload an explicit App ID profile for an app, Kiwi prefers it.
