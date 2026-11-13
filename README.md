<p align="center">
  <img src="assets/cleanshot.svg" width="96" height="96" alt="Cleanshot icon">
</p>

<h1 align="center">Cleanshot</h1>

<p align="center">
  A fast, private screenshot and markup tool for Windows 11.<br>
  One portable exe: no installer, no admin rights, no network access.
</p>

<p align="center">
  <a href="https://github.com/IconRunner05/cleanshot/actions/workflows/ci.yml"><img src="https://github.com/IconRunner05/cleanshot/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="#licence"><img src="https://img.shields.io/badge/licence-MIT%20or%20Apache--2.0-blue" alt="Licence: MIT or Apache-2.0"></a>
  <img src="https://img.shields.io/badge/Windows%2011-22H2%2B%20x64-0A84FF" alt="Windows 11 22H2 or later, x64">
</p>

## Features

- **Capture** a region, a window, a whole monitor or the last region again, from a hotkey, the
  tray or the capture panel, with an optional 3, 5 or 10 second countdown. The screen freezes
  while you choose, so menus and tooltips stay put.
- **Thumbnail** after each capture: click it to edit, drag it straight into another app, or let
  it slide away.
- **Mark up** with arrows, lines, rectangles, ellipses, a pen, text, speech bubbles, numbered
  steps, a highlighter, a spotlight and crop. Shapes stay editable until you copy or save, and
  every edit can be undone.
- **Redact** by pixelating or blurring. Redaction is destructive: a copied or saved image holds
  only the redacted pixels, never what was under them.
- **Copy or save** as PNG, JPG or BMP. Saves are named from the date and time and never replace
  an existing file unless you say so.
- **Private by design.** No network code, no telemetry, no update checks, no temp files and no
  capture history. Pixels stay in memory until you copy or save them.
- **Accessible.** Follows light and dark mode, works with high-contrast themes, Narrator and
  the keyboard, at any display scale and across several monitors.
- **Manageable.** Group Policy can turn saving off, fix the save folder or force PNG.

## Install

1. Download `cleanshot-X.Y.Z-x64.exe` from the
   [latest release](https://github.com/IconRunner05/cleanshot/releases).
2. Put it anywhere you like, for example in a `Tools` folder or on a USB stick, and run it.
   Cleanshot sits in the tray, ready for a hotkey.

Releases are not code-signed yet (see [Code signing policy](#code-signing-policy)), so
Windows SmartScreen may warn on the first run. Choose **More info**, then **Run anyway**.
To check that a download is the one this repository built, see
[Checking a download](SECURITY.md#checking-a-download).

To start Cleanshot when you sign in, turn on **Start with Windows** in its settings.

## Use

### Hotkeys

| Hotkey | Captures |
| --- | --- |
| PrtSc | A region you select |
| Alt+PrtSc | A window |
| Ctrl+PrtSc | The monitor under the pointer |
| Shift+PrtSc | The last region again, with no overlay |
| Ctrl+Shift+5 | Opens the capture panel: mode, countdown and options |

Windows 11 normally gives PrtSc to the Snipping Tool. While that is on, Cleanshot uses
**Ctrl+Shift+4** for a region instead and says so in a tray message. To give PrtSc to
Cleanshot, turn off **Use the Print screen key to open screen capture** in Windows Settings >
Accessibility > Keyboard, then restart Cleanshot.

Every hotkey can be changed in the settings window. Double-clicking the tray icon also captures
a region.

### Selecting

Drag to select a region, or click to pick the window under the pointer. Then:

| Key | Action |
| --- | --- |
| Drag a handle, or inside | Resize or move the selection |
| Arrows | Nudge the selection 1 px (with Shift: 10 px) |
| C or Enter | Copy |
| S or Ctrl+S | Save |
| Shift+S or Ctrl+Shift+S | Save As |
| E | Open in the editor |
| Esc | Cancel |

The toolbar follows the pointer to whichever monitor it is on. It sits at the bottom of the
screen, or at the top if you choose that in Settings, and moves to the other edge rather than
cover the selection. With **After capture** set to Copy or Open in editor, a region is copied
or opened as soon as you let go of the mouse.

### Editing

| Key | Tool |
| --- | --- |
| V | Select, move and resize |
| A, L | Arrow, line (Shift snaps to 45 degrees) |
| R, O | Rectangle, ellipse |
| P | Pen |
| T | Text |
| B | Speech bubble: drag the box to move it, drag the tip to re-point it |
| N | Numbered step |
| H | Highlighter |
| S | Spotlight |
| X | Redact (pixelate or blur) |
| C | Crop (Enter applies, Esc cancels) |

| Key | Action |
| --- | --- |
| Ctrl+C | Copy the image |
| Ctrl+S / Ctrl+Shift+S | Save / Save As |
| Ctrl+Z / Ctrl+Y | Undo / redo |
| Delete | Delete the selected shape |
| Ctrl+] / Ctrl+[ | Bring forward / send back (with Shift: to front / to back) |
| Ctrl+wheel, Space+drag | Zoom, pan |
| Ctrl+0 / Ctrl+1 | Fit to window / actual size |

The property strip beside the tools sets colour, stroke, fill, shadow, border, text size and
alignment, and redaction strength. Each tool remembers its own look, across restarts too.

## Settings

Open **Settings** from the tray menu. Changes are kept at once in `cleanshot.toml`:

- `%APPDATA%\Cleanshot\cleanshot.toml` normally, or
- `cleanshot.toml` beside `cleanshot.exe`, if one is there. Put an empty file there to run
  Cleanshot portably, for example from a USB stick. A file there that Windows marks as
  downloaded from the internet is ignored until you choose **Unblock** in its Properties.

The file can also be edited by hand while Cleanshot is closed:

```toml
save_folder = 'D:\Screenshots'   # default: Pictures\Screenshots
save_format = "png"              # png, jpg or bmp
after_capture = "toolbar"        # toolbar, copy, or editor
toolbar_position = "bottom"      # bottom or top
show_thumbnail = true
thumbnail_ms = 5000              # 1000 to 60000
show_pointer = false
remember_last_selection = false

[hotkeys]
region = "PrtSc"
last_region = "Shift+PrtSc"
full_screen = "Ctrl+PrtSc"
window = "Alt+PrtSc"
capture_panel = "Ctrl+Shift+5"
```

A hotkey is any of `Ctrl`, `Alt`, `Shift` and `Win`, then one key (`A`-`Z`, `0`-`9`,
`F1`-`F24` or `PrtSc`), joined by `+`. A value Cleanshot cannot use falls back to its default,
and a tray message names it.

## For IT administrators

- **Group Policy.** Each release includes `Cleanshot.admx` and `Cleanshot.adml`. Copy them to
  `C:\Windows\PolicyDefinitions` (the `.adml` into `en-US`) or your central store. The policies
  under **Cleanshot** fix the save folder, allow copying only, force PNG, or keep the settings
  window closed. They are read from `HKLM\SOFTWARE\Policies\Cleanshot`.
- **Security review.** [docs/security.md](docs/security.md) lists everything the exe reads,
  writes and accepts, and which CI gate or test holds each claim.
- **Removal.** Exit Cleanshot from the tray, delete the exe, and delete `%APPDATA%\Cleanshot`
  (or the `cleanshot.toml` beside the exe). If Start with Windows was on, turn it off first.

## Building from source

See [ARCHITECTURE.md](ARCHITECTURE.md) for the crate layout, the threat model and how to build
and test. Release builds come only from GitHub Actions on `windows-latest`; each release has
`SHA256SUMS`, a CycloneDX SBOM and a build-provenance attestation.

## Security

Please report vulnerabilities privately, as [SECURITY.md](SECURITY.md) describes, not in a
public issue.

## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by
[SignPath Foundation](https://signpath.org). Signing via SignPath Foundation is pending: releases
are unsigned pre-releases until the first signed release.

Team roles:

- Authors and committers: the [repository's contributors](https://github.com/IconRunner05/cleanshot/graphs/contributors)
- Reviewers: the [repository's contributors](https://github.com/IconRunner05/cleanshot/graphs/contributors)
- Approvers: [IconRunner05](https://github.com/IconRunner05)

## Privacy policy

This program will not transfer any information to other networked systems unless specifically
requested by the user or the person installing or operating it.

## Licence

MIT or Apache-2.0, at your option ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
Third-party assets are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
