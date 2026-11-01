# Changelog

All notable changes to Cleanshot are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A release's notes on GitHub are its section below, so a version is tagged only once its
section exists.

## [Unreleased]

## [0.1.2] - 2026-11-01

Fixes for black or garbled captures on some PCs, and the spotlight tool. Still an unsigned
pre-release.

### Fixed

- A capture could come out black on some PCs, often laptops or PCs with several monitors,
  when Windows handed over a frame with no image in it (F-01). Cleanshot now waits for a
  frame that holds the desktop, and otherwise captures the screen another way (D-28).
- A capture of an HDR monitor could come out garbled and dark, and one of a portrait
  (rotated) monitor could read past the end of the captured image. Both are now captured
  correctly (D-28).
- The first hotkey press after Cleanshot starts is no longer slow to show the overlay
  (P-01, D-76).

### Added

- Spotlight (F-43): S, or the Spotlight button, draws a box that keeps its area bright and
  dims the rest of the capture. Several spotlights share one bright area, and arrows, text
  and other shapes are never dimmed.
- `cleanshot.exe --log` also records how each screen was captured (D-79).

## [0.1.1] - 2026-11-01

A fix for copying while clipboard history is on. Still an unsigned pre-release.

### Fixed

- Copy no longer fails when Windows clipboard history (Win+V) or a clipboard manager reads
  the clipboard while Cleanshot is writing it (F-20). Before, such a reader could make a copy
  fail with "the clipboard refused the image"; now Cleanshot holds the clipboard until the
  image is written, and waits briefly if another program has it open.

### Added

- The exe's Properties, Details tab, shows its product name and version (Cleanshot, 0.1.1).
- `cleanshot.exe --log` also records how long the editor takes to show its first frame
  after you press Edit.

## [0.1.0] - 2026-10-31

The first public build: a pre-release for trying Cleanshot day to day. It is not code-signed
yet, so Windows SmartScreen may warn before it runs. `SHA256SUMS` and the build-provenance
attestation beside the exe show it was built by this repository's release workflow.

### Added

Capture

- Region capture (F-01 to F-04): PrtSc freezes every monitor under a dimmed overlay. Drag to
  select, across monitors if you like; drag a handle to resize, drag inside to move, and use
  the arrow keys to nudge by 1 pixel (Shift: 10). A label under the selection shows its size
  in pixels, and while you drag a loupe shows the pixels under the pointer with their
  position and colour. Esc cancels and keeps nothing.
- Overlay toolbar (F-05): Copy, Save, Edit and Cancel, under or over the selection. Tab
  moves between them and Enter or Space presses one; focus starts on Copy, so Enter copies.
  Ctrl+S saves, Ctrl+Shift+S asks where to save it (F-22), E opens the editor, Esc cancels.
- Last region (F-06): Shift+PrtSc copies the rectangle you last copied or saved again, with
  no overlay. It is kept in memory only, so it is gone after a restart.
- Full screen (F-07): Ctrl+PrtSc selects the monitor under the pointer and shows the toolbar,
  or copies it at once if the settings say so.
- Copy (F-20) puts the image on the clipboard as both a bitmap and a PNG.
- Save (F-21) writes to Pictures\Screenshots as `Screenshot {date} {time}.png` (or the format
  in the settings). A name that is taken gets a number, so no file is ever replaced. A save
  that fails is reported from the tray.
- The overlay closes with a short fade unless Windows animation effects are off, and uses
  the system colours under a high-contrast theme (F-13).

Editor

- Edit (F-23) opens the selection in the editor.
- Tools (F-30 to F-37): select and move (V), arrow (A), line (L), rectangle (R), ellipse (O),
  pen (P), highlighter (H), redact (X), crop (C) and text (T). Shift keeps lines and arrows
  to 45-degree steps and rectangles and ellipses square. Delete removes the selected shape.
- A strip under the toolbar sets colour, line width, fill, text size, and pixelate or blur,
  for the selected shape or for the tool's next one.
- Text (F-34) is multi-line, works with input methods, and handles Ctrl+A, Ctrl+C, Ctrl+X
  and Ctrl+V like a text box.
- Redaction (F-36) pixelates or blurs, and the export keeps none of the original pixels under
  it. The editor shows the pixelation or blur once you let go of the mouse.
- Crop (F-37) can be changed until you export.
- Undo and redo (F-38) with Ctrl+Z and Ctrl+Y, for every edit since the editor opened.
- Zoom and pan (F-39): Ctrl+wheel zooms about the pointer, Space-drag pans, Ctrl+0 fits.
- Copy, Save and Save As (F-40, F-22): Ctrl+C, Ctrl+S and Ctrl+Shift+S. The editor exports
  exactly as the overlay does (F-24).
- The editor follows the Windows light or dark mode, and every control can be reached and is
  named for keyboard and screen-reader users (F-58).

App

- A tray icon with Capture region, Capture screen and Exit (F-50).
- Hotkeys (F-51), with Ctrl+Shift+4 standing in for PrtSc while Windows gives PrtSc to
  Snipping Tool, and a tray message saying so (F-52). A hotkey another program holds is
  reported, not ignored.
- One copy runs at a time; starting Cleanshot again starts a region capture in the running
  one (F-53).
- Settings live in `cleanshot.toml`: beside `cleanshot.exe` if that file exists there
  (portable use), otherwise in `%APPDATA%\Cleanshot`. They set the hotkeys, save folder, save
  format (PNG, JPG or BMP) and whether a capture shows the toolbar (the default) or copies at
  once (F-54). A value that cannot be read falls back to its default, and every start-up
  problem is reported together in one tray message.
- `cleanshot.exe --log` writes timing lines to `cleanshot.log` beside the settings file;
  without `--log` nothing is written.

### Known limits

- Not code-signed, and a pre-release.
- No window capture, timer or capture panel yet (F-08, F-10, F-11).
- No settings window: settings are edited in `cleanshot.toml` (F-55).
- No numbered steps, speech bubbles, spotlight or border effects yet (F-41 to F-44).
- x64 only.
