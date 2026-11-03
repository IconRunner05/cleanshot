# Changelog

All notable changes to Cleanshot are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A release's notes on GitHub are its section below, so a version is tagged only once its
section exists.

## [Unreleased]

### Changed

- The editor opens sized round the capture at 100%, with the toolbar above it, instead of
  at 80% of the primary display (F-23). A capture too large for the screen opens fitted to
  it. The capture, fitted or not, now sits below the toolbar instead of under it.
- A colour picked on the property strip is now the colour every tool draws in next, not
  only the tool it was picked for. Stroke width, fill, redaction and text size are still
  kept for each tool.
- With the crop tool picked, the editor shows the whole capture with the crop's handles:
  drag inside the crop to move it, or a handle to resize it (F-37). A press outside it
  still starts a new crop.
- Each drawing tool can now select, move and resize the shapes it draws without
  switching to Select (F-30): for example, drag a speech bubble's tail with the bubble
  tool. A press anywhere else still draws a new shape, and a click on a text or bubble
  with its tool still opens it for typing.
- Esc in the editor goes back to the Select tool and clears the selection, from any tool
  (F-30). With Select already picked and nothing selected it does nothing.

### Fixed

- A crop showed in the editor only while it was being dragged, so the Crop tool looked
  as if it did nothing (F-37). Now the view fits the crop and everything outside it is
  dimmed. Copy and Save were already cropped.
- Closing the settings window or the editor when no other Cleanshot window was open quit
  Cleanshot too. It now closes only that window, and Cleanshot stays in the tray (F-55).
- On a laptop with two graphics chips, a capture no longer wakes the one that drives no
  screen, which made the overlay take a second or more to appear (P-01).

## [1.0.0-rc.1] - 2026-11-03

### Added

- Window capture (F-08): while selecting a region, press W to pick a window instead. The
  window under the pointer is highlighted by its visible frame, without its shadow, and a
  click selects it; Tab and Enter pick one from the keyboard. A click on bare desktop selects
  that monitor. A screen reader names each window as the highlight moves to it. Press W
  again to draw a region. Alt+PrtSc opens the capture picking a
  window from the start; change it in Settings like the other hotkeys.
- The Options menu (F-12): Options on the capture panel and on the toolbar over a selection
  sets the save folder, the save format, Show pointer, Show a thumbnail and Remember last
  selection, saved at once as in Settings. What your organisation sets is greyed with the
  reason.
- Show pointer (F-12): with show_pointer on, every capture holds the pointer as it looked
  and where it was when you captured, and the frozen screen shows it before you copy or save.
  It is off by default; switch it on the Options menu.
- Remember last selection (F-12): with remember_last_selection on, a region capture opens
  with your last selection already drawn, ready to copy or adjust, as long as it is still
  on screen. It is kept only until Cleanshot quits. It is off by default; switch it on the
  Options menu.
- The capture panel (F-11): Ctrl+Shift+5 shows a toolbar before you capture, where you
  choose Region, Window or Screen and a timer, then press Capture. It takes the keyboard
  focus, gives it back when it closes, and remembers your choices until Cleanshot quits.
  Change its hotkey in Settings like the others.
- A capture timer (F-10): set Timer on the capture panel to 3, 5 or 10 seconds, and Capture
  counts down in the panel's place before it takes the screen, leaving the focus where it
  was so you can open a menu to capture. Esc stops it. Neither the panel nor the countdown
  is ever in a capture.
- After you copy or save a capture from the toolbar, a thumbnail of it slides in at the
  bottom right for 5 seconds (F-09): click it to edit, drag it into an app as a PNG file or an
  image, or use its Save and Dismiss. It never takes the keyboard focus; while it shows, the
  tray menu also has Edit last capture and Save last capture. Clearing Show a thumbnail in
  the settings window turns it off.
- An organisation can set Cleanshot's policy under `HKLM\SOFTWARE\Policies\Cleanshot` (F-57):
  a fixed save folder, which may be a network share and which Save As cannot leave; copying
  only, which greys out Save on the capture toolbar and in the tray menu and says why, and
  makes the editor's Ctrl+S and Ctrl+Shift+S say why instead of saving; PNG only; and no Settings item in the tray menu.
  A value set wrongly makes Cleanshot stricter, never looser.
- Numbered steps (F-41): N, or the Numbered step button, places a filled circle with its
  number where you click. Steps count 1, 2, 3 in the order they were placed, and removing
  one renumbers the rest.
- Speech bubbles (F-42): B, or the Speech bubble button, opens a text box like the text
  tool, and the text sits in a filled rounded box with a tail. Drag the tail's tip to point
  it; T or B on a bubble edits its text.
- Group Policy template for the F-57 policy values: `Cleanshot.admx` and `Cleanshot.adml`,
  released beside the exe, under Computer Configuration > Administrative Templates >
  Cleanshot. Copy the `.admx` to `PolicyDefinitions` and the `.adml` to
  `PolicyDefinitions\en-US`.
- A winget manifest for `Cleanshot.Cleanshot` (three `.yaml` files), released beside the
  exe with the exe's SHA-256 filled in and listed in `SHA256SUMS`. It is not submitted to
  the winget repository yet.

### Changed

- `cleanshot.exe --log` also records each failure and each tray warning (D-109), as fixed
  words with the Windows error code where there is one. A log line never holds a path, a
  window title or your user name; the tray still shows the full warning.

### Fixed

- When a window, the overlay or the thumbnail could not open, the warning said Windows had
  refused to create "the tray window", whichever window it was. It now says a window could
  not open, or that the monitor layout could not be read, or that the preview is too large.

## [0.1.2] - 2026-11-02

A settings window, the spotlight and border tools, fixes for black or garbled captures on
some PCs, and security hardening. Still an unsigned pre-release.

### Security

- A DLL placed beside `cleanshot.exe` is never loaded in place of the Windows one (C-12).
  Before, Windows could load such a copy as Cleanshot started, so running the exe from a
  folder like Downloads could run someone else's code.
- The save folder must be a folder on a drive (D-83). A `save_folder` in `cleanshot.toml`
  that names a network share or a device path is refused, falls back to Pictures\Screenshots
  and is reported, so a settings file placed beside the exe cannot send your screenshots to
  another machine. To save to a share, use Save As.

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

- Settings window (F-55), from the tray's Settings item: what happens after a capture, the
  save folder, the save format and the three capture hotkeys, each change saved at once. A
  hotkey another program holds is named, and the old one stays.
- Start with Windows (F-56): a switch in the settings window. Cleanshot adds or removes its
  entry in Windows' startup list only when you flip it; the entry is the only record of the
  choice, and the `start_with_windows` key in `cleanshot.toml` is no longer used.
- Spotlight (F-43): S, or the Spotlight button, draws a box that keeps its area bright and
  dims the rest of the capture. Several spotlights share one bright area, and arrows, text
  and other shapes are never dimmed.
- Border and shadow (F-44): the Border and Shadow toggles on the property strip put a thin
  border and a soft drop shadow around the whole image, on the canvas and in what is copied
  or saved. The image grows to hold them; PNG keeps the shadow's edge transparent, and
  JPG, BMP and the copied bitmap put it over white.
- Cleanshot's own icon on the exe, its windows and the taskbar (D-91), and a one-colour
  tray icon that is dark on a light taskbar and white on a dark one (D-92).
- A security overview for IT reviewers, [`docs/security.md`](docs/security.md): what
  Cleanshot does and never does, and which CI gate, test or review holds each claim.
- `cleanshot.exe --log` also records how each screen was captured (D-79), and the memory
  Cleanshot holds of its own 10 seconds after the last capture, which is meant to stay
  under 10 MB (P-04, D-82).

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
