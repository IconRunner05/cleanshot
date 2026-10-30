# CLAUDE.md — Cleanshot

Cleanshot is a fast, simple, secure Windows 11 screenshot + markup tool in Rust. It ships as a single portable `cleanshot.exe`.

**Read `docs/CHARTER.md` before any task.** It is the source of truth. Precedence: Hard constraints (C-xx) > Feature acceptance criteria (F-xx) > Conventions > everything else.

## Non-negotiables (full list: CHARTER §2)

- Windows 11 22H2+ only; no cross-platform code (C-01).
- One static exe, no admin, no installer, no side-by-side DLLs (C-02, C-03).
- **No network code** (C-10). **No process launching** (C-11). **No untrusted deserialisation or IPC payloads** (C-13, C-14).
- `unsafe` only in `crates/cleanshot-win`, each block with `// SAFETY:` (C-16).
- No telemetry, temp files or capture history; redaction is destructive (C-20–C-22).
- Never build sharing/upload, plugins, recording, OCR or an installer (C-30–C-34).
- Only dependencies from the allow-list (CHARTER §6). Never bump the pinned GPUI rev unless the task says so.

## Look and feel (CHARTER §4, "Visual design language")

Use Cleanshot's own clean, macOS Sequoia-inspired style (frosted panels, hairline borders, generous spacing, one accent), not Windows 11 Fluent. Windows conventions still apply where required: caption buttons, Ctrl shortcuts, tray, native file dialogs, high contrast and accessibility. Never ship Apple fonts, icons or artwork.

## Where code goes (CHARTER §5)

| Crate | Owns |
| --- | --- |
| `cleanshot-app` | `main`, lifecycle, tray, hotkeys, wiring (thin) |
| `cleanshot-core` | Document, Shape, Command, undo, settings types (pure Rust, no OS/I/O) |
| `cleanshot-render` | Flatten + redaction + encode (tiny-skia), golden tests |
| `cleanshot-win` | All Win32/Direct2D/DXGI/clipboard/DPI/file code; the only `unsafe` crate |
| `cleanshot-editor` | GPUI editor + settings window; talks to core via Commands only |

## Commands

```powershell
cargo build                                   # debug
cargo build --release                         # release (LTO, static CRT, CFG)
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check && cargo audit
cargo test -p cleanshot-render -- --bless     # only when a golden image change is intended
```

Build on Windows with the MSVC toolchain (`x86_64-pc-windows-msvc`).

## How to work

1. Work in milestone order (CHARTER §10). Do not start a milestone until the previous gate has passed.
2. One task = one small change that passes the Definition of Done (CHARTER §7).
3. Cite IDs in commits (`F-21: atomic save with collision suffix`), test names (`f21_never_overwrites_existing_file`) and comments.
4. Spec silent → pick the simplest option that fits the North Star, implement it, and add a row with `Status: assumed` to the Decisions log (CHARTER §11).
5. Spec ambiguous and hard to reverse (new dependency, file format, setting names, anything touching a C-rule) → stop and ask.
6. If behaviour changes, update `docs/CHARTER.md` in the same change.
7. Check performance budgets (CHARTER §8) whenever you touch capture, overlay or export.
