# Changelog

Notable changes in each release. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## 1.1.0 (2026-11-13)

Small improvements to capturing, the editor and the settings window. Settings files from 1.0.0
are read as before.

### Added

- In the capture toolbar, C copies, S saves and Shift+S opens Save As.
- After capture can now open the editor: a region opens in it as soon as you let go of the
  mouse.
- The capture toolbar can sit at the top of the screen instead of the bottom, and it follows
  the pointer to whichever monitor it is on.
- Text can have a box behind it, in a colour of its own, with the border drawn round the box.
- Filled shapes, text boxes, speech bubbles and numbered steps have an opacity slider.
- The settings window shows which version of Cleanshot is running.

### Changed

- With After capture set to Copy, a region or a window you click is now copied as soon as you
  let go of the mouse, instead of waiting at the toolbar. Choose Show the toolbar to keep the
  old behaviour for regions.
- The spotlight remembers how dark it was, like every other tool setting.
- The editor window resizes to fit the image when a crop is applied or undone.
- The settings window opens tall enough to show all of its settings, where the screen allows.

## 1.0.0 (2026-11-10)

The first release. Cleanshot is a screenshot and markup tool for Windows 11, in one portable
exe with no installer, no admin rights and no network access. This build is not code-signed
yet, so Windows SmartScreen asks before the first run; it is marked as a pre-release until
signed builds are in place.

- Capture a region, a window, a full screen or the last region from a hotkey, the tray or
  the capture panel, with an optional countdown. The screen freezes while you choose.
- A thumbnail follows each capture, to copy it, save it, drag it into another program or
  open it in the editor.
- Mark up with arrows, lines, rectangles, ellipses, text, numbered steps, speech bubbles,
  the highlighter, the spotlight and crop. Shapes stay editable, and every step can be undone.
- Redaction pixelates or blurs and is destructive: a copied or saved image holds the redacted
  pixels, not what was under them.
- Copy to the clipboard or save to a folder you choose, with a file name from the date and
  time. A save never overwrites a file unless you say so.
- Settings cover hotkeys, the save folder and format, what happens after a capture and
  starting with Windows. A `cleanshot.toml` beside the exe makes it portable.
- Group Policy can turn saving off, tie it to one folder or fix its format.
- Works with high-contrast themes, Narrator and the keyboard, at any display scale and across
  several monitors.
- Writes no temp files and keeps no capture history.
