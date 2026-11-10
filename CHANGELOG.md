# Changelog

Notable changes in each release. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## 1.0.0-rc.2 (unreleased)

Cleanshot is a screenshot and markup tool for Windows 11, in one portable exe with no
installer, no admin rights and no network access.

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
