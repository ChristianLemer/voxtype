# Agent guide for this fork

This repository is **ChristianLemer/voxtype**, a fork of
[peteonrails/voxtype](https://github.com/peteonrails/voxtype). It extends
voxtype, a Linux push-to-talk dictation daemon, to **Windows and macOS** as
desktop apps, for a team that dictates on all three systems.

Read this file together with `CLAUDE.md`. Where they disagree, this file wins.

## About CLAUDE.md

`CLAUDE.md` is **upstream's file**, inherited with the fork and never edited
here: it is upstream's version as of the last upstream merge, so merging
upstream never conflicts on it. It describes upstream's project, which
is why parts of it do not hold here.

**Still applies:** project principles, architecture and design decisions, code
style, error handling and logging, best practices, GPG-signed commits and
contributor credit, version bumping with `Cargo.lock`, release-note style
(no em-dashes and the other AI patterns it lists).

**Does not apply to the fork:**

- *Building Release Binaries*, *AUR Packages*, *Release Notes and Website
  News*, *Website*: upstream's release process (nine Docker-built Linux
  binaries, deb/rpm, AUR, voxtype.io). The fork releases through CI instead,
  see [Releases](#releases) below.
- *Roadmap*: upstream's roadmap, not the fork's.
- *Non-Goals*: "Windows support" and "GUI configuration" are exactly what the
  fork adds.
- The module tree is partly out of date: `src/cli/` and `src/config/` are
  directories now, and the fork's own modules are listed below.

If `CLAUDE.md` feels heavy, the fix belongs upstream, not here.

## Fork rules

- **Branch.** Work lands on `desktop`, the fork's long-running branch, based
  on upstream's `dev`. Feature work happens on a branch that `desktop` can
  fast-forward to.
- **Fast-forward only.** No merge commits for the fork's own work. Rebase
  onto `desktop`, check with `git merge-base --is-ancestor origin/desktop
  <branch>`, then `git push origin <branch>:desktop`, which refuses a
  non-fast-forward by itself. Merging upstream releases into `desktop` is the
  one exception.
- **Add, don't rewrite.** Prefer new files and narrow, gated changes over
  edits to upstream code, so upstream merges stay cheap. Windows and macOS
  code sits behind `cfg` or a Cargo feature, and upstream's scripts gain
  opt-in switches (for example `VOXTYPE_SKIP_SETUP_APP`) rather than new
  defaults.
- **English** for everything in the repository: code, comments, commits,
  docs, issues and release notes.
- **Issues are closed, not collected.** Fix rather than file. An issue that
  gets opened is worked to closure straight away; only questions that need a
  human decision stay open.

## What the fork adds

| Path | What it is |
|---|---|
| `src/win/` | Windows shims for the Unix APIs the daemon uses: named pipes in place of Unix sockets, signals, process and filesystem helpers |
| `src/hotkey_windows.rs` | Low-level keyboard hook (`WH_KEYBOARD_LL`) |
| `src/hotkey_macos.rs` | rdev listener, plus the macOS key codes rdev does not name (`kvk`) |
| `src/menubar.rs` | Menu bar icon (macOS) and notification-area icon (Windows) |
| `src/osd/desktop/` | The recording card on Windows and macOS: `voxtype osd-card`, drawn on the CPU with tiny-skia |
| `src/settings_gui/` | The settings window: `voxtype settings [--section NAME]`, egui, generated from the config schema, with Models and Permissions panes |
| `src/setup/{start,autostart,permissions}.rs` | `voxtype start`, `restart`, `autostart`, `permissions`: unattended install and repair |
| `.github/workflows/build-desktop.yml` | Windows, macOS and Linux builds for the fork |

Cargo features of note: `settings-gui` (the settings window), `parakeet` and
`parakeet-coreml` (Parakeet on ONNX, CoreML on macOS).

## Releases

- **CI.** `build-desktop.yml` runs on every push to `desktop`, and by hand
  (`gh workflow run build-desktop.yml --ref <branch>`) for a branch. It builds
  `voxtype.exe`, the macOS app (DMG, `.app.zip`, binary with ONNX Runtime) and
  a Linux binary, all with `settings-gui`.
- **Tags** are `desktop-<version>-<n>`, for example `desktop-1.1.1-4`. Never
  `v*`, which would fire upstream's release workflows. `<n>` restarts at 1
  when the version changes.
- **Publishing: tag first, then build, then release.** Tag the commit on
  `desktop` (`git tag -a desktop-<version>-<n>`) and push the tag: the CI
  runs on `desktop-*` tags, and `build.rs` stamps the tag into the binaries,
  which is what About and the menu show (`cli::RELEASE_TAG`). Check it made
  it in (`strings voxtype | grep desktop-<version>-<n>`), name the tag run's
  artifacts as earlier releases do (`Voxtype-<version>-macos-arm64.dmg`,
  `Voxtype-macos-arm64.app.zip`, `voxtype-<version>-macos-arm64.tar.gz`,
  `voxtype-<version>-windows-x86_64.exe`, `voxtype-<version>-linux-x86_64`,
  `SHA256SUMS.txt`), and publish a **pre-release** on the existing tag with
  `gh release create <tag> --verify-tag --prerelease`. Pushing `desktop` and
  the tag together starts two runs; cancel the branch one.
- **macOS runners run short.** A macOS job that waits 15 minutes without a
  runner is cancelled by GitHub; rerun just that job (`gh run rerun <id>
  --failed`), it usually gets one within the hour.
- **Never replace the files of a published release.** A fix is a new release.
- **Test before releasing** what CI cannot: macOS behaviour on a real Mac,
  Windows on a Windows machine or VM. Prefer releasing the exact artifacts
  that were tested.

## Platform pitfalls

**macOS**

- Builds are signed **ad hoc**. macOS ties Input Monitoring and
  Accessibility to the code identity, so an update can leave the switch on in
  System Settings while macOS refuses the new binary. `voxtype permissions
  --reset`, or Repair in the Permissions pane, clears the grants so macOS asks
  again. A Developer ID signature would end this (issue #3).
- **Read permissions as the app, never from a terminal**: macOS attributes a
  grant to the responsible app, so a probe started from a terminal reports the
  terminal's grants.
  `open -n --stdout f --stderr f -a /Applications/Voxtype.app --args permissions`
- An eframe window **does not end its process when it closes** on macOS
  unless `run_and_return` is `false`. The settings window and the recording
  card both set it.
- The daemon logs to `~/Library/Logs/voxtype/stdout.log`; `stderr.log` stays
  empty.
- CI runs on `macos-15`: GitHub retired the macOS 14 images on 2026-11-02.

**Windows**

- The settings window tries OpenGL, then relaunches itself on Direct3D
  (wgpu, rendered by WARP when there is no GPU driver), then falls back to
  the text interface. winit cannot build a second event loop in one process,
  hence the relaunch.
- `Cargo.lock` deliberately points `gpu-allocator` at `windows 0.62.2`.
  Resolved freely it shares the `windows 0.54` that `cpal` pins, and wgpu
  stops compiling on Windows. Redo this if the lockfile is regenerated.
- The hotkey does not fire while an elevated window has focus.
- Windows 11 Notepad scrambles text typed with `SendInput`; other apps
  receive it intact.
- Garbled or doubled text usually means two daemons: check for a second
  `voxtype.exe` first.
- `output.pre_recording_command` and the other hooks run through `sh -c`,
  which Windows does not have.
