# Making a release

A release is built by GitHub, one machine per system, and signed here, on the
maintainer's machine. The two halves are separate on purpose: the signing key
is what an installed copy checks before it runs anything it downloaded, so it
stays where it was made rather than in a secret a workflow could hand around.

## The key, once

```sh
npx tauri signer generate -w ~/.tauri/codenotch.key --ci
```

Its public half belongs in `src-tauri/tauri.conf.json`, under
`plugins.updater.pubkey`, and is already there. Changing it makes every copy
already installed refuse every future update, because the key each one trusts
is the one it was built with.

**Losing the private key means no one can update again** — everyone would have
to install the next version by hand. Keep a copy of `~/.tauri/codenotch.key`
in a password manager.

## Per release

1. Add an entry at the top of `src/lib/changelog.ts`, in all three languages:
   it is what the app shows the first time the new version opens.
2. Bump the version in `package.json`, `src-tauri/tauri.conf.json` and both
   `Cargo.toml` files, then build once so `Cargo.lock` follows.
3. Commit and push to `main`.
4. Publish a release on GitHub whose tag is `v` and that version. The notes
   you write there are what the update card shows before installing, so lead
   with a sentence saying what changed.

Publishing it starts the `Release` workflow, which builds the `.exe`, the
`.deb` and the `.dmg` — plus `CodeNotch_<version>_universal.app.tar.gz`, which
is what an installed Mac app replaces itself with — and attaches all four. It
stops before it starts if the tag and the version in the app disagree.

5. When the workflow finishes, sign what it attached:

```sh
bash packaging/sign-release.sh v0.2.0
```

That signs each installer with the key, writes `latest.json` from the
release's own notes, and uploads both. Until it runs, the release is one to
install by hand; after it, every installed copy finds it.

`gh workflow run release.yml -f tag=v0.2.0` builds a release again, replacing
its files; sign it again afterwards.

## What reaches whom

An installed copy asks GitHub for
`releases/latest/download/latest.json` every six hours, and on the settings'
"Check now". It downloads the file for its own system, checks the signature
against the key built into it, and waits for a click on the update ring to
install. On Linux that click asks for the administrator's password, because
installing a `.deb` does.

Anyone on a version from before the updater existed (0.1.0) installs the next
one by hand, once.
