# Signed releases on R2

Release objects live in the dedicated `spotiflex-releases` bucket in the Cloudflare account that owns ClypDat. The `spotiflex-updates` Worker serves `https://updates.clypdat.xyz`. The bucket itself remains private. The Worker exposes only release executables, corresponding source archives, and stable channel manifests. Publishing requires a separate credential. The bucket and Worker keep their SpotiFlex-era names for compatibility.

The desktop checks on startup and every 30 minutes. An update dialog opens automatically with the available version and signed release notes. It downloads new releases automatically by default; this preference is under **Settings > About Lyssna**. With automatic downloads off, the dialog offers **Download update**. Once verification finishes, **Install and restart** applies the executable. **Not now** dismisses the prompt for that version until the next launch; Settings remains available. Checks and downloads run outside the UI thread. Installation always requires an explicit button press.

Every channel manifest is an envelope containing a base64 JSON payload and an Ed25519 signature. The public key is compiled into the app from `config/update-public-key.base64`. The signed payload fixes the version, Windows x86_64 target, relative artifact path, byte count, SHA-256 digest, and release notes. Redirects are disabled. Downloads have signed size limits; verification runs again immediately before replacement. Older versions and other platforms are rejected. From 0.3.1 the app reads `channels/stable/lyssna-windows-x86_64.json` and only accepts `releases/<version>/windows-x86_64/lyssna.exe`.

Copies up to 0.3.0 read `channels/stable/windows-x86_64.json` and only accept `spotiflex.exe`. Version 0.3.1 was published with `--bridge`, which also uploads the same executable as `spotiflex.exe` and signs it into that old channel. Old copies update to 0.3.1 once and follow the Lyssna channel after that. Later releases leave out `--bridge`; the old channel stays on 0.3.1.


The updater backs up the running executable to Lyssna's application data `updates/previous-lyssna.exe`, replaces only its own executable using `self-replace`, and starts the new version. The application must be in a directory the user can write to. Automatic installation currently targets Windows x86_64.

## Publish a version

The GitHub Actions secrets for signing and upload have been configured. Neither secret is committed or bundled into the client. Local copies are in the operating system credential store under `Lyssna/Release`; entries saved under the old `SpotiFlex/Release` name migrate automatically.

1. Update `version` in `Cargo.toml`, run checks, commit, and push.
2. Push a matching `vMAJOR.MINOR.PATCH` tag, or run **Publish signed release to R2** from GitHub Actions with the matching version and release notes.
3. The workflow builds and tests the client, signs the manifest, uploads the immutable executable and source archive, then activates the stable channel last.

Source archives use `git archive HEAD`, excluding ignored credentials and local project instructions. Published executable versions cannot be overwritten. Keep a copy of the release signing credential in an appropriate external secret backup; replacing the public key later requires a migration for already-installed clients.

For a local prepared release without uploading:

```powershell
cargo build --release --locked --bin lyssna --bin lyssna-release
.\target\release\lyssna-release.exe prepare 0.1.0 .\target\release\lyssna.exe .\release-notes.txt
```

Use `publish` in place of `prepare` to upload. `setup-keys` is for initial maintainer provisioning; it preserves existing credentials. `sync-github` sends those credentials directly to the repository's Actions secret store.

## Service checks

```powershell
cd infra/updates
npm ci
npm test
npm run check
npm run dry-run
```

Wrangler generates binding types from `wrangler.jsonc`. The Worker streams executable bodies from R2. Logs and traces are enabled. The publish token is a Worker secret; the signing private key is never deployed to Cloudflare.

The deployed service responds at `/health`. The stable manifest returns 404 until a real signed version is published. An absent channel means no update is available.

Releases up to 0.2.0 shipped under the name SpotiFlex. The first signed release is [SpotiFlex 0.1.0 for Windows x86_64](https://updates.clypdat.xyz/releases/0.1.0/windows-x86_64/spotiflex.exe), with its [corresponding source archive](https://updates.clypdat.xyz/releases/0.1.0/windows-x86_64/source.zip). Each person signs in with their own Spotify Premium account.

The current stable release is [Lyssna 0.3.1 for Windows x86_64](https://updates.clypdat.xyz/releases/0.3.1/windows-x86_64/lyssna.exe) ([source](https://updates.clypdat.xyz/releases/0.3.1/windows-x86_64/source.zip)), published from a local build on 2026-11-06 with `--bridge`. Both downloaded executables matched the local build's SHA-256, and both stable channels report 0.3.1. [Lyssna 0.3.0](https://updates.clypdat.xyz/releases/0.3.0/windows-x86_64/spotiflex.exe) was published on 2026-11-03.

The release signing key was rotated for 0.3.0 after the original was lost from the credential store. Copies on 0.2.0 or earlier trust only the old key and reject 0.3.0, so those installs need a one-time manual download of 0.3.0; later releases update automatically. The GitHub Actions secrets still hold the old keys until `sync-github` replaces them.

[SpotiFlex 0.2.0](https://updates.clypdat.xyz/releases/0.2.0/windows-x86_64/spotiflex.exe) was published from a local build on 2026-10-27. The downloaded executable matched the local build's SHA-256, and the stable channel reports 0.2.0. [SpotiFlex 0.1.1](https://updates.clypdat.xyz/releases/0.1.1/windows-x86_64/spotiflex.exe) was published on 2026-10-22. An installed release updating to a later published one has not been observed yet.

Live verification on 2026-10-22 used a local 0.0.1 baseline and the published 0.1.0 release. Startup automatically detected and downloaded the release, verified its signature and checksum, and opened the update prompt. Installation replaced the running executable and restarted into 0.1.0 with the account still signed in. The installed executable matched the published release byte for byte, the backup reported 0.0.1, and the restarted updater reported Up to date. An upgrade between two separately published releases remains untested.

The fork's periodic version checks and explicit update prompt informed this flow. Its browser reload mechanism is replaced by verified native executable replacement.
