# Cleanshot security overview

For IT reviewers deciding whether to allow `cleanshot.exe`. Each row says what the program does
or never does, the rule in [`CHARTER.md`](CHARTER.md) it comes from, and what holds it there:

- **Gate**: a CI check that fails the run. A release is published only from a run in which
  every gate passed.
- **Test**: a named test in the CI `checks` job, which fails the run if it fails.
- **Review**: the code does it, but nothing automatic stops a change from breaking it.

Rows about "Cleanshot's code" cover the five crates built into `cleanshot.exe`;
`cleanshot-fieldtest` is a CI probe that changes display settings and sends input on the CI
runner, and is never published. GPUI, the UI library, and Windows components also run inside
the process; what they do is under "Windows components in the process", with the charter rule
named where one applies. "Known gaps" lists where Cleanshot's own code falls short of a charter
rule today.

To report a vulnerability, see [`SECURITY.md`](../SECURITY.md).

## What it is

Cleanshot is one portable exe for Windows 11 (22H2 or later, x64): a screenshot and markup tool.
There is no installer, and it runs as the user who starts it (C-03; the manifest asks for
`asInvoker`; review). The C runtime is linked in, and the exe imports only Windows system DLLs
from a fixed list in `charter-gates.sh`, plus Windows API-set names (`api-ms-win-*`) (C-02;
gate). The only command-line option it reads is `--log`; other arguments are ignored.

The exe is **not code-signed yet**: releases are unsigned pre-releases until signing through
SignPath Foundation is in place (see the README's code signing policy), so SmartScreen warns
before the first run. Until then, check a download with the attestation and checksums described
under "Checking a download" in `SECURITY.md`.

## What Cleanshot's code never does

The import scan reads import names, so a call reached through `GetProcAddress`, COM or WinRT, or
under a name the scan does not list, would pass it; those rows rest on review as well.

| Cleanshot's code never... | Rule | Proof |
| --- | --- | --- |
| Uses a network API. The exe imports no `ws2_32`, `winhttp` or `wininet`, and the HTTP and TLS crates (`hyper`, `reqwest`, `ureq`, `openssl`, `native-tls`, `rustls`) and tokio's `net` feature are banned from the build. GPUI's HTTP type crates (`http`, `http_client`, `url`) are in the build with no transport. | C-10 | Gate (import scan; `cargo deny`); review |
| Sends telemetry or checks for updates. | C-20 | Gate (as above); review |
| Starts another program by its own calls. The exe imports no `CreateProcess*`, `ShellExecute*` or `WinExec`, and GPUI's own calls that would launch something (open a URL, reveal or open a file, restart) are replaced with calls that do nothing. The Save As dialog is a separate matter: see "Windows components in the process". | C-11 | Gate (import scan); review (`restricted_platform.rs`) |
| Parses a file other than its own settings. It reads `cleanshot.toml`, refused over 64 KB, into fixed fields. An unreadable or oversized file gives every default, a bad value gives that setting's default, and the problem is reported; corrupt, truncated and random input is read safely in the `c13_` tests. | C-13 | Test (the `c13_` tests, among them `c13_a_file_over_64_kb_is_not_read` and `c13_random_bytes_are_read_safely`) |
| Keeps a capture history. Pixels stay in memory until you copy or save them. Each capture copies every monitor into memory, even for a region. | C-21 | Review |

## What Cleanshot's code reads, writes and accepts

| What | Detail | Proof |
| --- | --- | --- |
| Settings | Read from `cleanshot.toml` beside the exe if one is there (portable use), otherwise `%APPDATA%\Cleanshot\cleanshot.toml`. Cleanshot does not write this file. | Review (`settings_file.rs`) |
| Saved images | Saved to the Screenshots known folder (created if missing) or to the configured folder, which must be a full path. Ctrl+S names the file `Screenshot <date> <time>.<ext>`, adding `-2`, `-3` and so on rather than replacing a file. Save As writes the name you type; Windows' Save As asks before replacing an existing file. | Test (`c15_unsafe_templates_are_refused`, `c15_a_save_folder_that_is_not_a_full_path_is_refused`, `f21_never_overwrites_existing_file`); review for Save As |
| Partial files | A save writes `<name>.partialN` in the target folder, then links or moves it into place and deletes it. A crash or power cut mid-save, or a failed delete (an antivirus holding the file, say), leaves it there, and a failed delete is not shown to the user. | Review (`save.rs`) |
| Clipboard, written | Copy puts the image on the clipboard as a bitmap and a PNG; Ctrl+C and Ctrl+X in a text box put text there. Cleanshot does not opt out of clipboard history, so if Windows' clipboard history (Win+V) is on it keeps the copy, and so does cloud clipboard sync if the user turned it on (D-84). | Review (`clipboard.rs`, `text_box.rs`) |
| Clipboard, read | Only when you paste into a text box (Ctrl+V), through GPUI, which reads the clipboard's text, its first image and any file list; Cleanshot uses only the text. | Review (`text_box.rs`) |
| Registry | Reads one per-user value, `HKCU\Control Panel\Keyboard\PrintScreenKeyForSnippingEnabled`. Writes no registry value. | Review (`tray.rs`) |
| Log | Off unless started with `--log`. Then `cleanshot.log` beside the settings file (its folder created if missing), emptied when a `--log` start opens it; a start without `--log` leaves the last log in place. It holds timings, capture events, GPU adapter and display output descriptions, monitor bounds and rotation, Windows error text, and the share of each captured monitor that came out black; never the pixels or clipboard contents. | Test (`c23_the_log_starts_empty_and_holds_one_timed_line_per_event`); review for its contents |
| Hotkeys | Three system-wide hotkeys while it runs, changeable in `cleanshot.toml`: capture a region (PrtSc, or Ctrl+Shift+4 when Snipping Tool holds PrtSc, as it does on a default Windows 11), the screen (Ctrl+PrtSc) and the last region (Shift+PrtSc). While the capture overlay is open it registers 16 more system-wide: Esc, Enter, Space, E, Tab, Shift+Tab, the arrows, Shift with the arrows, Ctrl+S and Ctrl+Shift+S. | Review |
| Other programs | One instance runs per Windows session, through a `Local\Cleanshot` mutex. A second launch posts a message with no data, which the running one treats as "capture a region". No message carries a payload; the tray and the overlay read only small ids from a message's parameters. Any program in the session at the same or higher integrity can post them: start a capture, copy the last region, close Cleanshot, or, while the overlay is open, draw the selection and copy, save or Save As it. A program that creates the mutex first stops Cleanshot from starting. | Test (`c14_a_second_claim_in_the_session_sees_the_first`, `c14_a_second_launch_signal_asks_the_tray_to_capture`); review for the rest |

## Windows components in the process

These run inside `cleanshot.exe` with its rights, or act on its files, and Cleanshot does not
control what they do.

- **Save As** is the Windows file dialog. Like any Save As, it can browse network locations
  (C-10), its right-click menu can open files with other programs (C-11), and it loads the
  shell extensions installed on the PC. Cleanshot does not set the option that keeps a save out
  of Recent items, so Windows records the saved file in Recent items and the dialog's per-user
  history in the registry (C-05, C-21; D-84).
- **GPUI** starts OLE, reads the keyboard layouts under `HKLM`, creates Windows' drag-image and
  DirectManipulation COM objects, turns DirectComposition off when the environment variable
  `GPUI_DISABLE_DIRECT_COMPOSITION` is set, and keeps a hidden window that ends the process
  when Windows ends the session.
- **Text and drawing** use DirectWrite, Direct2D and Direct3D, which read the installed fonts
  and load the graphics driver. Typing loads the user's input methods.
- **Drag and drop**: the editor window accepts drops through GPUI, which reads the dropped file
  names. Cleanshot's code does nothing with a drop.
- **Accessibility**: the windows expose UI Automation, so screen readers and other UI
  Automation clients can read and operate them.
- **Crashes** (C-20): a crash ends the process and goes to Windows Error Reporting like any
  program's. Cleanshot neither turns it off nor configures it, so the PC's WER policy decides
  whether a report, or a memory dump that can hold screen pixels, is kept or sent.
- **OneDrive** (C-30): when OneDrive backs up Pictures (Known Folder Move), the Screenshots
  folder is inside OneDrive and saved images sync to the cloud.
- **Explorer** (C-05) keeps its own per-user registry entry for the tray icon, keyed to the
  exe's path.

## How it is hardened

| Measure | Rule | Proof |
| --- | --- | --- |
| After start-up, a DLL loaded by name is searched for in System32 only: `SetDefaultDllDirectories` is the first call in `main`. DLLs Windows loads by full path (COM servers registered per user or per machine, shell extensions, the graphics driver, input methods) load from wherever they are installed. DLLs resolved before `main` are a gap: see below. | C-12 | Gate (the exe must import `SetDefaultDllDirectories`); review that it runs first |
| ASLR, DEP and Control Flow Guard are on for the release exe. Rust's prebuilt standard library is not instrumented for Control Flow Guard. | C-17 | Gate (exe headers) |
| Cleanshot's code is Rust. `unsafe` code is allowed only in `cleanshot-win`, which holds its Windows API calls; every other crate forbids it, and every `unsafe` block must carry a `// SAFETY:` comment. GPUI and the `windows` crate have their own `unsafe` code. | C-16 | Gate (`unsafe` scan; clippy's `undocumented_unsafe_blocks` denied) |
| Redaction (blur, pixelate) replaces the covered pixels in every copy and save with averages over 16-pixel cells; the original pixels are not kept underneath. Large pixelated text in a known font can sometimes be partly guessed back. | C-22 | Test (the `c22_` tests, among them `c22_a_saved_capture_holds_the_redacted_pixels_not_the_capture`) |

## Known gaps

Each of these is where Cleanshot's own code falls short of a charter rule today. Its entry
changes in the commit that closes it.

- **C-12, DLL planting.** The exe statically imports DLLs that are not on Windows' KnownDLLs
  list, among them `winmm`, `dwmapi`, `d3d11`, `dxgi`, `d2d1`, `dwrite`, `dcomp` and
  `uiautomationcore`. Windows resolves these before `main` runs, searching the exe's own folder
  first, so a DLL of one of these names placed beside the exe (in Downloads, say) can be loaded.
- **C-10, network paths.** The configured save folder may be a network (UNC) path such as
  `\\server\share`. Saving there, or opening Save As on it, makes Windows connect to that
  server, and depending on the PC's network policy sign in to it with the user's Windows
  credentials. A `cleanshot.toml` beside the exe takes precedence over the one in `%APPDATA%`,
  so a crafted file placed next to the exe can set such a folder.

## Supply chain and evidence

- Dependencies come only from crates.io and the GPUI repository and must pass licence, ban,
  source and advisory checks (`cargo deny`, `cargo audit`; gate). Three advisories are ignored
  in `deny.toml`, each for a crate that is unmaintained but not vulnerable (`paste`,
  `ttf-parser`, `rustybuzz`, pulled in by GPUI). Dependencies are chosen from the allow-list in
  CHARTER section 6 (review).
- The release build replaces the runner's checkout, home and Cargo folders in the paths it
  embeds, and the gate fails if the exe's ASCII or UTF-16 strings hold `/Users/`, `/home/`,
  `\Users\`, `\.cargo\` or `:\a\` (D-24; gate).
- A release publishes the exe that the CI gates scanned in the same workflow run, with a
  CycloneDX SBOM, `SHA256SUMS` and a GitHub build-provenance attestation.
- The evidence for every gate above is that release's run on the repository's Actions tab
  (workflow "Release"). Job `ci / checks` holds the clippy, `cargo deny` and `cargo audit`
  steps and the tests. Job `ci / release-exe` holds the step "Release exe passes the binary
  scan and the unsafe scan", which is `charter-gates.sh`.

## What it leaves on the PC

To remove Cleanshot, delete the exe and these:

- the settings folder `%APPDATA%\Cleanshot` (or `cleanshot.toml` beside the exe), and
  `cleanshot.log` in it if `--log` was used;
- any `.partialN` file a failed save left in a save folder;
- Windows' own records: Recent items and the Save As history for saved files, and Explorer's
  tray icon entry.
