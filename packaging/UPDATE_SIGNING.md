# Update signatures

The app installs an update only when the release's `checksums.txt.sig` is a
valid Ed25519 signature, over the exact bytes of `checksums.txt`, by the key
compiled into it from `assets/update-public-key.hex`.
[fastframe-update](https://github.com/crmne/fastframe/tree/main/crates/fastframe-update)
does the checking, before it parses a checksum or downloads a package. A
missing or invalid signature fails closed: there is no unsigned fallback and
no key taken from the release being checked. The file names in
`checksums.txt` bind each checksum to its version and platform.

The private half is `UPDATE_SIGNING_KEY`, an Ed25519 PKCS#8 PEM key, a secret
of the `release-signing` environment. Set the environment to take jobs from
`v*` tags only, so no other workflow run, branch or pull request can read it.
Only the release workflow's publish job reads the key, and that job builds
nothing. There is no approval step: whoever can push a `v*` tag can publish
to every installed copy. The pinned `sign-release` action writes the
manifest, checks that the secret matches the committed public key, signs, and
verifies what it wrote.

Without the secret the release is still published, with plain checksums and a
warning: no installed copy updates itself to it.

## Making the key

    openssl genpkey -algorithm ed25519 -out update-signing-key.pem
    openssl pkey -in update-signing-key.pem -pubout -outform DER | tail -c 32 | xxd -p -c 64 > assets/update-public-key.hex

The PEM file is the secret. The hex file is committed.

## Keeping the key

GitHub never shows a secret again. Keep a copy of the private key in a
password manager, outside any checkout. Never commit one, including a retired
one.

Installed copies trust only the key compiled into them. To change the key,
first ship a release, signed by the old key, whose app also trusts the new one
(`UpdateConfig::additional_publisher_keys`), and only then sign with the new
key. Replacing the secret alone cuts every installed copy off from updates.

If the key is lost or leaked: stop publishing, remove the secret, and publish
a new version with a new key for people to install by hand. Old copies cannot
learn a new key from a release they cannot verify.

## Trying an update without publishing

`TABLETIST_UPDATE_FEED=http://127.0.0.1:<port>` makes the app read a feed on
this computer instead of GitHub. fastframe-update takes only a loopback
address, and the release it serves must carry the same signature. The folder
served holds `latest.json`, shaped like GitHub's release (`tag_name`,
`html_url`, `assets` with `name`, `size` and `browser_download_url`), and the
files it names: the package for this platform, `checksums.txt` and
`checksums.txt.sig`.

## What this is not

Not Apple notarization and not Windows Authenticode: it says the update came
from whoever holds this key, not that macOS or Windows trusts the publisher.
And a key held by CI is only as safe as the repository and its workflow.
