# Architecture

Cleanshot is a screenshot and markup tool for Windows 11 (22H2 or later, x64). It ships as one
portable `cleanshot.exe`: no installer, no admin rights, no DLLs beside it. It's written in Rust,
with [GPUI](https://github.com/zed-industries/zed) for the editor and direct Win32, Direct2D and
DXGI calls for capture and the overlay.

## Crates

| Crate | Owns |
| --- | --- |
| `cleanshot-core` | The document, shapes, commands and undo, and the settings types. Pure Rust: no OS calls, no I/O. |
| `cleanshot-render` | Flattening a document to pixels, redaction and encoding, drawn with tiny-skia. Golden image tests. |
| `cleanshot-win` | Every Windows call: capture, the overlay, clipboard, DPI, files, registry. The only crate with `unsafe`. |
| `cleanshot-editor` | The GPUI editor and settings windows. |
| `cleanshot-app` | `main`, start-up, the tray, hotkeys, and the wiring between the others. |

`cleanshot-fieldtest` is a probe that CI runs on a real Windows runner. It's never published.

Dependencies point one way. `core` depends on nothing in the workspace. `render` and `win` depend
on `core`, `editor` on `core` and `render`, and `app` on all four.

## How a capture moves through it

On a hotkey, every monitor is copied into memory before any window is created, so the overlay can't
appear in its own capture. The overlay works on that frozen copy. The editor changes a document only
by applying commands to its history, so every edit can be undone.

`render::flatten` is the one source of output pixels. Copy and Save both encode what it returns,
and its result type can't be built anywhere else, so nothing can export pixels that skipped
redaction.

There's no async runtime. Concurrency is GPUI's executor and plain threads with channels.

## Rules that hold everywhere

- No network code. No telemetry, no update checks.
- No launching other programs.
- `unsafe` only in `cleanshot-win`, each block with a `// SAFETY:` comment.
- Redaction is destructive: the covered pixels are replaced in every copy and save, and the
  originals aren't kept underneath.
- No temp files and no capture history. Pixels stay in memory until you copy or save them.
- The only file we parse is our own `cleanshot.toml`, read into fixed fields with a size limit. No
  window message carries a payload.

## Threat model

We defend against a file planted next to the exe, a DLL planted to be loaded by name, and a saved
image being un-redacted. A settings file can't send saves to another computer, and DLLs loaded by
name come from System32 only.

We don't defend against another program already running as the same user. It can post window
messages that start a capture and read our windows through UI Automation, as it can with any
Windows program.

[docs/security.md](docs/security.md) lists what the exe reads, writes and accepts, and what proves
each claim.

## Building and testing

On Windows, with the MSVC toolchain:

```powershell
cargo build
cargo build --release
cargo test --workspace
```

`.cargo/config.toml` sets `x86_64-pc-windows-msvc` as the default target, so on macOS or Linux
`cargo check` and `cargo clippy` see the Windows code but can't link it. The pure crates run on the
host:

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --target host-tuple -p cleanshot-core -p cleanshot-render
```

Everything that needs Windows, including the editor's and the app's tests and the scan of the
release exe, runs in GitHub Actions on `windows-latest`.

Golden images live in `tests/golden/`. After a change to the output that's meant to happen, update
them with:

```sh
cargo test --target host-tuple -p cleanshot-render --test golden -- --bless
```
