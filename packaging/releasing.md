# Making a release

Every release is built by the `Release` workflow, on one machine per system,
and signed with the updater key — which is what installed copies check before
they run anything they downloaded.

## Once, per repository

The key is made with the Tauri CLI and kept in two places: the repository's
secrets, where the workflow reads it, and somewhere safe of your own.

```sh
npx tauri signer generate -w ~/.tauri/codenotch.key --ci
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/codenotch.key
```

The public half belongs in `src-tauri/tauri.conf.json`, under
`plugins.updater.pubkey`; it is already there, and changing it makes every
installed copy refuse every future update, because the key it trusts is the
one it was built with.

**Losing the private key means no one who has CodeNotch can update again** —
they would each have to install the next version by hand. Keep a copy of
`~/.tauri/codenotch.key` in a password manager.

## Per release

1. Add an entry at the top of `src/lib/changelog.ts`, in all three languages:
   it is what the app shows the first time the new version opens.
2. Bump the version in `package.json`, `src-tauri/tauri.conf.json` and both
   `Cargo.toml` files, then build once so `Cargo.lock` follows.
3. Commit and push to `main`.
4. Publish a release on GitHub whose tag is `v` and that version — the notes
   you write there are what the update card shows before installing, so lead
   with a sentence saying what changed.

The workflow then builds the `.exe`, the `.deb` and the `.dmg`, signs each,
and attaches them to the release along with `latest.json`, the file every
installed copy reads to find the next version. It refuses to start if the tag
and the version in the app disagree.

`gh workflow run release.yml -f tag=v0.2.0` runs it again for a release that
already exists, replacing its files.

## What reaches whom

An installed copy asks GitHub for
`releases/latest/download/latest.json` every six hours and on the settings'
"Check now", downloads the file for its own system, checks the signature, and
waits for a click on the update ring to install it. On Linux that click asks
for the administrator's password, because installing a `.deb` does.

Anyone on a version from before the updater existed (0.1.0) installs the next
one by hand, once.
