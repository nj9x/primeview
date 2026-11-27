# Packaging PrimeView

Desktop installers are built with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager)
from an already-compiled `primeview` binary. The web build is a static `dist/` folder.

| Platform | Formats | Output (in `target/packager/`) |
|---|---|---|
| macOS 11+ | `app`, `dmg` | `PrimeView.app`, `PrimeView_<ver>_<arch>.dmg` (`aarch64`, `x64` or `universal`) |
| Windows 10/11 | `nsis`, `wix` | `primeview_<ver>_x64-setup.exe` (per-user, no UAC), `primeview_<ver>_x64_en-US.msi` |
| Linux | `deb`, `appimage` | `primeview_<ver>_amd64.deb`, `primeview_<ver>_<arch>.AppImage` |
| Web | zip of `dist/` | `PrimeView_<ver>_web.zip` (release workflow only) |

## Files

| Path | What |
|---|---|
| `Packager.toml` | cargo-packager config: name, identifier `com.primeview.app`, publisher, copyright, category, icons, per-format settings |
| `package.sh` | wrapper around `cargo packager` (bash; works in Git Bash on Windows) |
| `LICENSE.txt` | license notice shown as the license page of the NSIS and MSI installers |
| `icons/generate_icons.py` | draws the icon (Python 3 stdlib only) and writes every size below |
| `icons/icon-{32,64,128,256,512,1024}.png` | full-bleed PNGs: Linux packages, and `icon-256.png` is the window icon to embed in the app |
| `icons/icon.ico` | Windows icon, 16 to 256 px (BMP entries + PNG for 256) |
| `icons/icon.icns`, `icons/macos/icon-1024.png` | macOS icon with Big Sur grid padding and shadow |

To change the icon, edit `generate_icons.py` and run `python3 packaging/icons/generate_icons.py`.
`iconutil` is needed for the `.icns`, so on Linux or Windows the script skips it.

## Building installers locally

One-time setup: `cargo install cargo-packager --locked` (the workflow pins 0.11.8).

```bash
cargo build --release -p primeview
packaging/package.sh            # formats for this OS -> target/packager/
```

`package.sh` options: `-f app,dmg` picks formats, `--target <triple>` packages
`target/<triple>/release/primeview`, `--binaries-dir`/`-o` override the paths. It honours
`CARGO_TARGET_DIR` (for example `CARGO_TARGET_DIR=target/agent-pkg packaging/package.sh`).
It writes a temporary `packaging/.packager.generated.toml`, which is gitignored. That copy
takes its `version` from `cargo pkgid -p primeview`, so installers never drift from the crate.
It also sets the absolute `binaries-dir` and adds signing settings from the environment.
`cargo packager -c packaging/Packager.toml -f <formats>` also works for a plain
`target/release` build.

### macOS

```bash
cargo build --release -p primeview
packaging/package.sh                       # PrimeView.app + PrimeView_0.1.0_aarch64.dmg
```

Universal binary (what CI ships):

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
export MACOSX_DEPLOYMENT_TARGET=11.0
cargo build --release -p primeview --target aarch64-apple-darwin
cargo build --release -p primeview --target x86_64-apple-darwin
mkdir -p target/universal-apple-darwin/release
lipo -create -output target/universal-apple-darwin/release/primeview \
  target/{aarch64,x86_64}-apple-darwin/release/primeview
packaging/package.sh --target universal-apple-darwin   # PrimeView_0.1.0_universal.dmg
```

* **Finder permission:** the DMG step drives Finder with AppleScript to lay out the window.
  The first time, macOS asks whether the terminal may control Finder. If nobody answers,
  it fails with `AppleEvent timed out (-1712)`. To skip the layout step, run with
  `CI=true packaging/package.sh`, as CI does. You still get a working DMG with the
  `Applications` link, just without custom icon positions.
* Without `APPLE_SIGNING_IDENTITY`, the bundle gets an **ad-hoc** signature (`codesign -s -`).
  The bundle must be sealed to run on Apple Silicon. Without a seal, Gatekeeper calls a
  downloaded copy "damaged".
* There is no license click-through on the DMG: `package.sh` drops `license-file` on macOS.

To check a build:

```bash
plutil -p target/packager/PrimeView.app/Contents/Info.plist
codesign --verify --deep --strict target/packager/PrimeView.app
hdiutil attach -nobrowse -readonly target/packager/PrimeView_*.dmg   # then hdiutil detach /Volumes/PrimeView
```

### Windows (Git Bash)

```bash
cargo build --release -p primeview
packaging/package.sh            # nsis,wix
```

cargo-packager downloads NSIS 3.09 and the WiX 3 toolset into its cache on first use.
The NSIS installer installs per user (`%LOCALAPPDATA%\PrimeView`) and needs no admin rights.
The MSI installs per machine. On some newer Windows images, light.exe ICE validation fails
with `LGHT0217` because VBScript is missing. If you hit that, enable the VBScript optional
feature, or build on `windows-2022`.

### Windows installer from macOS or Linux

The NSIS installer can also be cross-built. The MSI can't, because the WiX tools only run on Windows.

```bash
brew install mingw-w64 makensis          # Linux: apt install mingw-w64 nsis
rustup target add x86_64-pc-windows-gnu
cargo build --release -p primeview --target x86_64-pc-windows-gnu
packaging/package.sh --target x86_64-pc-windows-gnu   # -> target/packager/primeview_<ver>_x64-setup.exe
```

This build uses the GNU toolchain rather than MSVC. It only depends on DLLs that ship
with Windows 10 and 11. `package.sh` sets `LC_ALL=en_US.UTF-8` for this step, because
`makensis` crashes with `std::bad_alloc` under the C locale. Code signing is skipped when
cross-building. Sign on Windows, or set `[windows] sign-command` (for example to
`osslsigncode`).

### Linux (Debian/Ubuntu)

```bash
sudo apt-get install -y libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libwayland-dev file
cargo build --release -p primeview
APPIMAGE_EXTRACT_AND_RUN=1 packaging/package.sh   # deb,appimage
```

The binary links only libc. winit/wgpu `dlopen` X11/Wayland, xkbcommon and Vulkan/EGL,
so the `.deb` declares them in `depends`, and the AppImage expects them on the host, which
is normal for GPU apps. Build on the oldest distro you want to support (CI uses
Ubuntu 22.04), because glibc is forward-compatible only. cargo-packager downloads
linuxdeploy for the AppImage, and `APPIMAGE_EXTRACT_AND_RUN=1` avoids needing FUSE.

### Web

```bash
cd app && trunk build --release     # -> dist/
```

## Release workflow (`.github/workflows/release.yml`)

Triggers:

* Pushing a tag `vX.Y.Z`. The tag must equal the `primeview` crate version, or the
  `version` job fails.
* **Run workflow** (workflow_dispatch). With a `tag` input it does the same as a tag push.
  Without one it only builds and uploads workflow artifacts.

Jobs run in parallel after the version check:

| Job | Runner | Does |
|---|---|---|
| macOS | `macos-latest` | builds aarch64 and x86_64, `lipo`s a universal binary, packages `.app` + `.dmg`, signs and notarizes if secrets exist, verifies the plist/signature, runs a 10 s launch smoke test (warning only: CI VMs may lack a GPU) |
| Windows | `windows-latest` | builds x86_64-pc-windows-msvc, imports the signing cert if present, packages NSIS `.exe` + `.msi` |
| Linux | `ubuntu-22.04` | installs eframe's system deps, packages `.deb` + `.AppImage`, prints the deb metadata |
| Web | `ubuntu-latest` | `trunk build --release`, zips `dist/` |
| Release | `ubuntu-latest` | only with a tag: downloads everything, writes `SHA256SUMS.txt`, creates a **draft** release with generated notes (softprops/action-gh-release) |

Review the draft on GitHub and publish it by hand.

### Signing secrets (all optional)

Add them under *Settings → Secrets and variables → Actions*. If a secret is missing, that
step is skipped and the build stays unsigned. Nothing fails.

**macOS** (needs an Apple Developer Program membership):

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | `base64 -i DeveloperIDApplication.p12`: a *Developer ID Application* cert + private key exported from Keychain Access |
| `APPLE_CERTIFICATE_PASSWORD` | the .p12 export password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID1234)`, from `security find-identity -v -p codesigning` |
| `APPLE_ID` | Apple ID email used for notarization |
| `APPLE_TEAM_ID` | 10-character team ID |
| `APPLE_APP_PASSWORD` | an [app-specific password](https://support.apple.com/102654) for that Apple ID. The workflow passes it to cargo-packager as `APPLE_PASSWORD` |

With the first three set, cargo-packager imports the cert into a temporary keychain. It then
signs the binary and the `.app` with the hardened runtime, and signs the `.dmg`. When
`APPLE_ID`/`APPLE_TEAM_ID`/`APPLE_APP_PASSWORD` are also set, it notarizes and staples the
`.app`, and the workflow notarizes and staples the `.dmg` (`xcrun notarytool` + `stapler`).
To sign locally, export `APPLE_SIGNING_IDENTITY` (a cert in your login keychain is enough),
plus the notarization variables if you want notarization.

**Windows**:

| Secret | Value |
|---|---|
| `WINDOWS_CERTIFICATE` | base64 of a code-signing `.pfx` (`base64 -w0 cert.pfx`, or `[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))`) |
| `WINDOWS_CERTIFICATE_PASSWORD` | the .pfx password |

The workflow imports the .pfx into `Cert:\CurrentUser\My` and exports its thumbprint as
`WINDOWS_CERTIFICATE_THUMBPRINT`. `package.sh` then puts it into the config, and
cargo-packager signs `primeview.exe`, the uninstaller and both installers with `signtool`
(sha256, DigiCert timestamp). Certificates that live only in an HSM or cloud service, like
most EV certs and Azure Trusted Signing, cannot be exported as a .pfx. For those, set
`[windows] sign-command = "<tool> ... %1"` in `Packager.toml` instead.

## Unsigned builds: what users see

**macOS Gatekeeper.** An ad-hoc signed, un-notarized app downloaded from the internet is
blocked on first launch with *"PrimeView" cannot be opened / Apple could not verify…*.
To allow it:

* macOS 15+: try to open the app once, then go to *System Settings → Privacy & Security*
  and choose **Open Anyway**.
* macOS 11–14: right-click the app, choose **Open**, then **Open**.
* Or, for testers: `xattr -dr com.apple.quarantine /Applications/PrimeView.app`.

A Developer ID signature plus notarization removes these prompts.

**Windows SmartScreen.** An unsigned `setup.exe`/`.msi` shows *Windows protected your PC*.
Users click **More info → Run anyway**. Signing with a standard OV certificate shows the
publisher name. SmartScreen can still warn until the certificate has built up download
reputation. EV certificates and Azure Trusted Signing get reputation faster.

**Linux.** No signing is needed. Users run `chmod +x` on the AppImage, or install the deb
with `sudo apt install ./primeview_*.deb`.

## Auto-update: recommended approach (not wired in yet)

[`cargo-packager-updater`](https://docs.rs/cargo-packager-updater) (0.2.x) is the companion
crate to cargo-packager. It fits this setup. It is not suitable for the wasm build, which
updates on page reload anyway.

* **Check:** `cargo_packager_updater::check_update(current_version, Config { endpoints, pubkey, .. })`.
  If an update is available, call `update.download_and_install()`. Run it from a
  background thread, because the API is blocking, and show an "Update available" toast in
  egui.
* **Update formats it can install:** macOS `app` (a signed `PrimeView.app.tar.gz` replaces
  the bundle), Windows `nsis` or `wix` (it runs the installer: `Passive` by default, or
  `BasicUi`/`Quiet`, then exits), and Linux `appimage`. It cannot update `.deb` installs;
  those users update through apt or a manual download. Apart from the WiX path, it does
  not relaunch the app, so the app must restart itself or ask the user to.
* **Signing keypair** (minisign; separate from Apple and Windows code signing):
  `cargo packager signer generate -w packaging/updater.key`. Keep the private key and its
  password out of git: store them as secrets `CARGO_PACKAGER_SIGN_PRIVATE_KEY` and
  `CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD`, and embed the public key in the app.
  When those env vars are set, `cargo packager` writes a `.sig` next to every package. On
  macOS it also creates and signs `PrimeView.app.tar.gz`. To enable this in CI, add the two
  secrets to the packaging steps' `env`, and upload `*.sig` + `*.app.tar.gz` too.
* **Endpoint:** for example
  `https://github.com/<owner>/primeview/releases/latest/download/latest.json` (static), or a
  small server at `https://updates.example.com/{{target}}/{{arch}}/{{current_version}}`
  (dynamic: return `204` when there is nothing new). The static JSON looks like this. The
  platform keys are `{os}-{arch}`, with os `macos`, `windows` or `linux`. The crate's own
  doc example says `darwin`, but its code uses `macos`.

  ```json
  {
    "version": "0.2.0",
    "notes": "Bug fixes",
    "pub_date": "2026-12-01T12:00:00Z",
    "platforms": {
      "macos-aarch64":   { "url": ".../PrimeView.app.tar.gz",            "signature": "<.sig contents>", "format": "app" },
      "macos-x86_64":    { "url": ".../PrimeView.app.tar.gz",            "signature": "<.sig contents>", "format": "app" },
      "windows-x86_64":  { "url": ".../primeview_0.2.0_x64-setup.exe",   "signature": "<.sig contents>", "format": "nsis" },
      "linux-x86_64":    { "url": ".../primeview_0.2.0_x86_64.AppImage", "signature": "<.sig contents>", "format": "appimage" }
    }
  }
  ```

  The universal `.app.tar.gz` serves both macOS keys. A final release-job step would
  generate `latest.json` from the `.sig` files and attach it to the release.
* **macOS caveat:** an update can only replace the bundle in place if the app has a stable
  Developer ID signature. Ad-hoc builds lose their Gatekeeper approval with every update,
  so turn on Apple signing before shipping auto-update.
