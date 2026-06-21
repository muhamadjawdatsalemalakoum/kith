# Releasing

Kith ships as a desktop app for Windows, macOS, and Linux, built in CI by
[`.github/workflows/release.yml`](../.github/workflows/release.yml).

## Cut a release

1. Bump the version if you want it set locally (CI also derives it from the tag):
   `Cargo.toml` `[workspace.package] version` and `apps/desktop/tauri.conf.json` `version`.
2. Tag and push:
   ```sh
   git tag v0.1.0 && git push origin v0.1.0
   ```
3. The **Release** workflow builds installers on macOS, Linux, and Windows runners and
   creates a **draft** GitHub Release with them attached. Review it and click *Publish*.

Installers produced: Windows `_x64-setup.exe` (NSIS), macOS `_universal.dmg`
(Apple Silicon + Intel), Linux `.AppImage` / `.deb` / `.rpm`.

## Code signing (recommended before a public launch)

Unsigned builds trigger SmartScreen (Windows) and Gatekeeper (macOS) warnings. To ship
trusted builds, add these repo secrets and uncomment the matching lines in the `env:`
block of `release.yml`:

- **macOS** (needs an Apple Developer ID, $99/yr): `APPLE_CERTIFICATE`,
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`,
  `APPLE_TEAM_ID`.
- **Windows** (a code-signing certificate): `WINDOWS_CERTIFICATE`,
  `WINDOWS_CERTIFICATE_PASSWORD`.

## Auto-update (optional)

To enable in-app updates via [tauri-plugin-updater](https://v2.tauri.app/plugin/updater/):

1. Generate a signing keypair (one time):
   ```sh
   cargo tauri signer generate -w ~/.kith-updater.key
   ```
2. Put the **public** key in `apps/desktop/tauri.conf.json` under
   `plugins.updater.pubkey`, set `bundle.createUpdaterArtifacts: true`, and add an
   `endpoints` entry pointing at your releases' `latest.json`.
3. Add the **private** key as repo secrets `TAURI_SIGNING_PRIVATE_KEY` (and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if you set one) and uncomment them in
   `release.yml`.
4. Register the plugin in `apps/desktop/src/lib.rs` (`.plugin(tauri_plugin_updater::Builder::new().build())`)
   and add a "Check for updates" action.

The private key is yours alone — never commit it. Until this is configured, releases are
manual-download only.
