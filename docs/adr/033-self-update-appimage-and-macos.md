# ADR 033: The AppImage and the Mac app update themselves

Status: accepted (2026-11-15). Amends ADR 030 (macOS updates no longer only show the download page).

**Context.**
- "Check for updates" installed only the Windows setup program's copy, through Velopack. On Linux and macOS it showed the release page, so every update meant a manual download.
- On Linux, the `.deb`, the archive and Nix are updated by their package manager or by the user, and Laqta must not replace files those own. The AppImage is one file in a folder the user picked, with nothing else managing it.
- On macOS the app is one bundle. Dragged to Applications from the disk image, it sits in a folder an administrator account can write to.

**Decision.**
1. **`laqta-update` decides; `apps/laqta` runs the commands.** `install::Installer::detect` takes the platform, the running executable and the AppImage runtime's variables, and returns Velopack, AppImage, Mac app, or nothing. The download, the staging, the swap and the restart command are in the crate; the app only spawns what the crate builds, as it already did for `velopack_apply_command`. The decisions are tested without a desktop.
2. **What updates itself.** The Windows setup program's copy (`Update.exe` above `current\\laqta.exe`). An AppImage, only when the running executable is inside the runtime's `APPDIR`: a program started from another AppImage's terminal inherits that image's `APPIMAGE`, and would otherwise replace it. A Mac bundle, unless it runs from a disk image (`/Volumes`) or from Gatekeeper's translocated copy (`AppTranslocation`), both read-only. An AppImage or bundle in a folder this user can't write to (`/opt`, or `/Applications` for a standard account) is not offered either, so the offer never ends in a permission error: `detect` creates and removes a probe folder there. The AppImage path is resolved first, so a link to it updates the image itself. Everything else keeps the notice.
3. **Every download is verified as the Windows package is**: its SHA-256 comes from the same release's `SHA256SUMS` (ARCHITECTURE "Supply chain"). The release publishes `laqta-linux-x86_64.AppImage` and, new, `laqta-macos-universal.app.zip` (`ditto -c -k --keepParent`, so the signature survives).
4. **The swap is a rename beside the old copy.** The download goes to `.laqta-update-<user>` in the same folder (`install::Staging`), so the swap never crosses file systems. The running AppImage keeps its mount, and a running bundle keeps its open files. A bundle swap moves the old bundle aside first and puts it back if the new one can't move in. The staging name is per user, because Laqta runs once per user and `/Applications` is shared by every account; the same user's next run clears what an interrupted one left. An old bundle this user can't remove is skipped for a fresh name instead of blocking every later update. Only the downloads run on the HTTP runtime; the file work and the commands run on the caller's executor.
5. **The restart waits for the old process.** A shell waits for Laqta's PID to exit, then starts the new copy (`open` for a bundle) and removes the old bundle. A copy started sooner would find the old one running and hand its request over (single instance). The AppImage runtime's variables are cleared so the new image sets its own.

**Rejected.**
- **Replacing the `.deb` or Nix binaries.** Their package manager owns those files and would see them change underneath it.
- **Unpacking the zip in Rust.** `ditto` is the tool Apple documents for bundles; a zip crate would need to restore symlinks and modes itself, and it adds a dependency for one platform.
- **Velopack on Linux and macOS.** It would mean another package format beside the AppImage and the disk image, and on macOS its own signing step. The two swaps above are a few renames.

**Consequences.**
- A Mac app in `/Applications` updates itself only for an account that can write there (an administrator). Other accounts get the release page, as before.
- An ad-hoc signed build (a release made without the signing secrets) has a new code identity every release, so macOS asks for Screen Recording and Accessibility again after an update, exactly as after a manual one. A Developer ID build keeps the grants.
- A restart that can't start after the swap is logged, not reported as a failure: the update is in place, Laqta quits, and the next start runs it.
- One install runs at a time (`updates::Updater`); a second click while one runs is refused.
- The update zip is not stapled. A download Laqta makes itself carries no quarantine flag, so Gatekeeper does not check it on first open; the disk image stays notarized for first installs.
