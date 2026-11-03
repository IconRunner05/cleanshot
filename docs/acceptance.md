# Owner acceptance pass

The hands-on check the owner runs on the reference PC with a release candidate before v1.0.0 is tagged (CHARTER 9, D-102). One step per feature: do what it says, and tick it only if you see what it says. Anything else is a failed step; note what you saw.

Start from a release candidate exe downloaded from its GitHub pre-release, with no `cleanshot.toml` next to it or in `%APPDATA%\Cleanshot` from an earlier run (C-05). Default hotkeys: PrtSc captures a region (Ctrl+Shift+4 if Windows' "Use the Print screen key to open screen capture" is on), Shift+PrtSc the last region, Ctrl+PrtSc the full screen.

F-10, F-11 and F-12 get their steps when they are built.

## Start and tray

- [ ] F-53, F-50: Run `cleanshot.exe`. A Cleanshot icon appears in the tray and no window opens. Run it again: no second icon, and the region overlay opens.
- [ ] F-50: Right-click the tray icon. The menu shows Capture region, Capture screen, Settings and Exit. Double-click the icon: the region overlay opens.
- [ ] F-52: Turn on Settings > Accessibility > Keyboard > "Use the Print screen key to open screen capture" and restart Cleanshot. A tray balloon names that setting, and Ctrl+Shift+4 opens the overlay. Turn the setting off again and restart.

## Capture

- [ ] F-01, F-02: Press PrtSc. Every monitor dims at once, with no flicker of the overlay in the capture. Drag a rectangle: it stays bright, with an accent border, 8 handles and a `W x H` pill in physical pixels.
- [ ] F-04: While dragging, a loupe shows the pixels under the pointer zoomed, with a grid, the coordinates and the colour's hex value. It goes when you let go.
- [ ] F-03: Drag a handle, drag inside the selection, and press the arrow keys (Shift for 10 px). The selection resizes, moves and nudges.
- [ ] F-05: A frosted toolbar sits near the bottom of the monitor (at the top if the selection is there), with Copy focused. Tab moves the focus; Esc closes the overlay and nothing is copied or saved.
- [ ] F-01 on mixed DPI: With two monitors at different scales, capture a region across both and paste it into Paint. It is sharp, at each monitor's physical pixels.
- [ ] F-20: Capture, press Enter, and paste into Outlook, Teams, Word, Paint and Edge. Each shows the capture.
- [ ] F-21: Capture and press Ctrl+S. A file `Screenshot <date> <time>.png` appears in Pictures\Screenshots. Save a second capture: both files are there, and neither replaced the other.
- [ ] F-22: Capture and press Ctrl+Shift+S. The Windows save dialog opens. Pick an existing file: it asks before replacing it.
- [ ] F-06: Press Shift+PrtSc. The last copied or saved rectangle is copied at once, with no overlay.
- [ ] F-07: Press Ctrl+PrtSc. The monitor under the pointer is captured.
- [ ] F-08, F-58: Open a few windows, minimise one, put one on another virtual desktop, and show a click-through overlay (for example a game's FPS counter). Capture, wait for the thumbnail, then press PrtSc and W. Move the pointer: each window lights up by its frame, without its shadow. The minimised window, the one on the other desktop and Cleanshot's own thumbnail never light up, the click-through overlay is passed over for the window under it, and bare desktop, the wallpaper, lights up the whole monitor. Click a window: it becomes the selection. Press W, Tab a few times and Enter: the window lit up is selected. Press Esc, then Alt+PrtSc: the overlay opens already picking a window, and a click selects one. Windows' own Alt+PrtSc copy of the active window does not happen while Cleanshot runs. Turn on Narrator and do it again: Narrator says "Window to capture" and the title of each window the highlight moves to, and "Capture screen" when the pointer rests on bare desktop.
- [ ] F-09: Capture and press Enter. A thumbnail slides in at the bottom-right of that monitor, and the window you were in keeps the focus. Hover it: it stays, with Save and Dismiss along its bottom. Move away: it fades after about 5 s. Capture again and drag the thumbnail into Word or Explorer: the image or a PNG file lands there. Capture again and click it: the editor opens with the capture.
- [ ] F-13: Turn off Settings > Accessibility > Visual effects > Animation effects. The overlay appears with no fade. Turn on a high-contrast theme: the overlay and toolbar use its colours.

## Editor

- [ ] F-23: Capture and press E. The overlay closes and the editor opens with the capture.
- [ ] F-30 to F-35: Draw an arrow (A), a line (L), a rectangle (R), an ellipse (O), a pen stroke (P), text (T, typed with an input method too) and a highlight (H). Click a shape to select it, drag a handle, and press Del.
- [ ] F-41: Add three numbered steps (N): they read 1, 2, 3. Delete the second: the last becomes 2.
- [ ] F-42: Add a speech bubble (B), type in it, and drag its tail's tip. The tail follows and the box stays.
- [ ] F-43: Add a spotlight (S). Everything outside it dims.
- [ ] F-44: Turn on Border and Shadow in the property strip. The exported image gains them, outside the capture's pixels.
- [ ] F-36: Redact (X) some text, copy, and paste into Paint. The text cannot be read.
- [ ] F-37: Crop (C), then copy. Only the cropped part is pasted.
- [ ] F-38, F-39: Ctrl+Z and Ctrl+Y undo and redo each step. Ctrl+wheel zooms, Space-drag pans, Ctrl+0 fits.
- [ ] F-40, F-24: Ctrl+C and Ctrl+S in the editor copy and save the marked-up image.
- [ ] F-58: Switch Windows between light and dark: the editor follows. Every editor control can be reached with Tab.

## Settings and policy

- [ ] F-55: Open Settings from the tray. Change the save folder, the format and a hotkey: each takes effect at once with no Save button. Turn off "Show a thumbnail after Copy or Save" and capture: no thumbnail appears.
- [ ] F-56: Turn on Start with Windows, sign out and back in: Cleanshot starts. Turn it off: it no longer does.
- [ ] F-51: In Settings, set the region hotkey to one another program already holds (Win+E, which Explorer holds, for example). The window says "Another program uses Win+E, so Capture region keeps PrtSc. Press another." and PrtSc still opens the overlay.
- [ ] F-54: Exit Cleanshot. In `cleanshot.toml` (beside the exe, or in `%APPDATA%\Cleanshot`), set `save_format = "gif"` and add a line `colour = "blue"`, then start Cleanshot. One warning says save_format has a value Cleanshot cannot use, so its default applies; nothing is said about `colour`, and a capture saves as PNG. Put the file back as it was.
- [ ] F-57: With the Group Policy template from the release (`Cleanshot.admx`, `Cleanshot.adml`), enable "Copy captures only, never save them": Save and Save As are turned off in the overlay and the editor. Enable "Keep the settings window closed": Settings leaves the tray menu.
