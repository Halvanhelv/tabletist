---
title: Download
description: Download Tabletist for macOS, Windows or Linux, with the install steps for each.
nav_order: 1
---

The newest stable version is on the
[latest release](https://github.com/igor-alexandrov/tabletist/releases/latest)
page. Every version, pre-releases included, is on the
[releases page](https://github.com/igor-alexandrov/tabletist/releases).
Each release lists its files with a `checksums.txt`, which you can use to
check that a download was not damaged or changed.

## macOS

Download `tabletist-v<version>-macos-universal.dmg`. It is one file for both
Apple Silicon and Intel. Open it and drag **Tabletist** to **Applications**.

### First open on macOS

Releases are signed and notarized only when they are built with the Apple
signing secrets, and macOS blocks an unsigned build on first launch. For an
unsigned build only: try to open it once, then click **Open Anyway** in
**System Settings → Privacy & Security**, or run:

```sh
xattr -dr com.apple.quarantine /Applications/Tabletist.app
```

## Windows

Download and run `tabletist-v<version>-x86_64-pc-windows-msvc-setup.exe`, or
the `aarch64` one on an ARM PC. The installer does not need administrator
rights.

To run Tabletist without an installer, download the `.zip` for your PC,
extract it and open `tabletist.exe`.

## Linux

### Arch Linux and Omarchy

The AUR packages (`tabletist-bin` and `tabletist`) are not published yet.
Until they are, use the release archive below.

### Other distributions

Download `tabletist-v<version>-x86_64-unknown-linux-gnu.tar.gz`, or the
`aarch64` one. It holds the `tabletist` binary, a `.desktop` file and the
icon.

The Linux binaries are built on Ubuntu 24.04 and need glibc 2.39 or newer
(Ubuntu 24.04, Debian 13, Fedora 40 or later). On an older system,
[build from source]({% link _reference/build-from-source.md %}).

## Updates

Tabletist looks for a newer release when it starts and every six hours after
that. It downloads nothing by itself. When there is a newer release, the
About window says so:

1. Open **About Tabletist** (from the keyboard shortcuts window, or the app
   menu on macOS).
2. Click **Download update**. The download is checked against the
   publisher's signature before anything is installed.
3. Click **Restart to update**. Tabletist closes, the new version is
   installed and it opens again. If the new version does not start, the
   previous one comes back.

Save or discard pending changes first: Tabletist does not restart over them.
**Check for updates** in the same window looks at once.

A copy updates itself only where it can replace itself: the app in
**Applications** on macOS, a copy installed by the Windows installer, and a
copy unpacked from the `.zip` or `.tar.gz` with the `tabletist-portable.txt`
file still beside it. A copy that a package manager installed (the AUR,
Homebrew) is updated by that package manager, and the About window says
which version is out. Versions 0.2.0 and earlier do not update themselves:
install the next one by hand.

## Next

[Open Tabletist and make your first connection]({% link _guide/getting-started.md %}).
