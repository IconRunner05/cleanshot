# F-34 Text: layout, display, export and tests

Status: approved by the orchestrator after blind review (2026-10-30). No code yet. Editor side: the editor seat
(core, render, editor). DirectWrite side: the capture seat (win, app).

F-34 asks for multi-line text where IME works and Ctrl+A and Backspace behave like a text box. The
export must match what was on screen, and `cleanshot-render` stays pure Rust with no OS calls
(CHARTER 5).

## What GPUI does on screen, at the pinned rev

The export copies GPUI's own glyph path, so this note starts from it (`gpui_windows/src` at
`14b7b0e`):

- Layout and shaping are DirectWrite with `DWRITE_MEASURING_MODE_NATURAL`
  (`direct_write.rs:788`, `:810`).
- Each glyph run is rasterised with `IDWriteFactory::CreateGlyphRunAnalysis`, not Direct2D. The
  rendering mode comes from `GetRecommendedRenderingMode`, with `OUTLINE` replaced by
  `NATURAL_SYMMETRIC`, and the grid-fit mode comes from the same call (`direct_write.rs:777-813`).
- Colour glyphs (emoji) are split into colour layers with `TranslateColorGlyphRun`, and each layer
  is rasterised as grayscale `NATURAL_SYMMETRIC` with its own colour (`direct_write.rs:1006-1031`).
- Gamma and contrast are applied afterwards, in GPUI's shader, from the system rendering params:
  `gpui::get_gamma_correction_ratios(GetGamma())` and `GetGrayscaleEnhancedContrast()`
  (`directx_renderer.rs:855-862`).

## The shape

```rust
Text(TextBlock)

pub struct TextBlock { at: PointF, text: String, size: f32, em_width: f32 }
```

- `at` is the top-left corner of the first line box, in image pixels.
- `text` keeps the user's line breaks. There is no automatic wrapping: a line ends only where the
  user pressed Enter, so display and export never have to agree on a break.
- `size` is the font size in image pixels. The line height is `LINE_HEIGHT * size`, with
  `LINE_HEIGHT` = 1.25.
- `em_width` is the widest line's layout width (layout box, not ink bounds) at size 1, in ems. It is
  measured with `NATURAL` measuring mode, where advances scale linearly with size, so it stays
  right through any resize. Measuring at size 1 rules out the zoom level at commit time. The height
  is not stored: with hard breaks only it is `lines * LINE_HEIGHT * size`, and `extent()` derives
  both.
- The fields are private. The only constructor is `TextBlock::new(at, text, size, em_width)`, and
  only an edit commit calls it, with a fresh measurement of `text`. A move changes `at`, and a resize
  changes `size`. No operation changes `text` without a new `TextBlock`, so `em_width` cannot go
  stale.
- Core uses `extent()` for `shape_at`, the selection outline and `Drag`. Core has no font engine,
  so the measurement must come in from outside.

## Baseline

Inter's ascent and descent (`hhea`, in font units, with units per em) are constants in core, next
to the font bytes. Core derives once `baseline(line) = at.y + line * LINE_HEIGHT * size +
half_leading + ascent * size`, where `half_leading = (LINE_HEIGHT - (ascent + descent)) * size / 2`.
Win and the editor both place each line's glyph run at that baseline and never at their own
layout's choice. A Windows test checks that DirectWrite's `GetMetrics` on the embedded Inter equals
core's constants, and that a mask's top edge sits at the expected offset from `at`.

## One font, one copy

Inter SemiBold (OFL-1.1, approved in CHARTER 4 and 6) is embedded once, in core
(`cleanshot_core::INTER`), the lowest unit both sides use. The editor registers those bytes with
GPUI (`cx.text_system().add_fonts`). Win loads the same bytes into DirectWrite through an
in-memory font file loader (`IDWriteFactory5::CreateInMemoryFontFileLoader`), so no installed font
decides the Latin glyphs and nothing is written to disk (C-21). The exe size is measured against
P-06 (under 15 MB) in the commit that adds the font. THIRD_PARTY_NOTICES.md gets its row in the
same commit.

## Display and editing (editor, with the pure part in core)

- The canvas shapes each text block with GPUI's text system at `size * fit.scale` and paints it in
  the shape's colour, with emoji in their own colours, as GPUI draws them.
- The edit state is a pure type in core, `TextEdit { text, selection, marked }`, with UTF-16
  ranges. It implements the operations GPUI's `InputHandler` needs
  (`replace_text_in_range`, `replace_and_mark_text_in_range`, `unmark_text`, select all, delete
  backward and forward, caret moves), so host tests cover typing, selection and an IME composition
  sequence. The editor's `BoxInput` forwards the trait's calls to it, and the canvas registers it
  each frame with `window.handle_input` while a box is open (D-52). GPUI's Windows platform turns `WM_IME_COMPOSITION` into those calls
  (`events.rs:152-153`).
- Key routing while editing is part of core's key map: `action_for_key` takes the editor's state
  (idle, dragging, editing, composing) in place of `dragging`. While editing, tool letters, Delete
  and Space return no action, so `t` and `v` reach the input handler. While composing, Esc and a
  press outside the box also return no action, so the IME gets them. Host tests cover each case.
- Committing (Esc or a press outside the box when not composing, or picking another tool) applies
  one `Command::Add` or `Command::Replace`, so a whole text edit is one undo step (F-38). Committing
  text identical to the shape's text applies nothing. Committing empty text adds nothing, or removes
  the shape that was edited.
- Ctrl+C, Ctrl+X and Ctrl+V act on the box's text while editing, not on the image.

## Export (win makes masks, render composites)

1. `deliver` (app) asks win for the masks of every text block:
   `win::text::masks(&Document) -> Result<TextMasks, Error>`.
2. Win lays each block out with `IDWriteTextLayout` (Inter from the in-memory collection, `size`,
   no wrapping, `NATURAL` measuring) and collects its glyph runs through a text renderer callback.
   It places each line at core's baseline, then rasterises each run exactly as GPUI does:
   `CreateGlyphRunAnalysis` with the mode `GetRecommendedRenderingMode` gives (OUTLINE replaced by
   NATURAL_SYMMETRIC), grayscale antialiasing, and `CreateAlphaTexture` for raw 8-bit coverage. Colour
   glyphs go through `TranslateColorGlyphRun`, giving one mask per colour layer with its colour.
3. Raw coverage carries no gamma, so no user setting reaches it: ClearType Tuner and the system
   gamma reach DirectWrite only through rendering params; win, like GPUI, passes none to
   `GetRecommendedRenderingMode` and never calls `CreateRenderingParams`. Render applies a fixed gamma
   correction (D-51) with GPUI's shader formula: contrast enhanced for dark text, then alpha
   corrected by `gpui::get_gamma_correction_ratios` for the one fixed gamma. A Windows editor test
   checks render's ratios against GPUI's function, so they cannot drift.
4. `GlyphMask { left: i32, top: i32, width: u32, height: u32, coverage: Vec<u8>, color:
   Option<Color> }` and `TextMasks` (by `ShapeId`) are plain data in core. A `None` colour means the
   shape's colour, and `Some` is a colour-glyph layer.
5. `render::flatten(&Document, &TextMasks)` paints each text shape in document order, filling its
   colour through each gamma-corrected mask. A text shape with no entry in `TextMasks` is
   `render::Error::MissingTextMask(ShapeId)`; one with no ink has an empty entry. The overlay's documents have no text and pass
   `TextMasks::default()`.

Redaction and crop are unchanged: text above a redaction stays, text below it is destroyed with the
pixels (C-22), and the crop applies last.

On screen and in the file, glyph positions, shapes and colour layers are the same. Edge
antialiasing can differ in two ways:
- the screen uses ClearType where GPUI chooses it, while a saved image uses grayscale, because
  ClearType fringes are wrong in a file;
- the screen uses this machine's gamma, while the file uses the fixed one.

## Charter changes, in the same commit as the code

CHARTER 5 "Export" becomes:

> **Export:** `render::flatten(&Document, &TextMasks) -> RgbaImage` is the single source of output
> pixels (F-24). Text glyph coverage comes from DirectWrite in win as plain masks (core's
> `TextMasks`), made by the same glyph path GPUI draws on screen; render composites them, so render
> stays pure Rust.

The CHARTER 5 dependency line is unchanged, because the masks travel as core types.

M0's unchecked item "text input and IME work in a custom element" (CHARTER 10) is proven in two
parts:
- the composition logic, by core's host tests on `TextEdit`;
- the platform path, by an app test on the runner that opens the editor, starts a text box, and
  drives `ImmSetCompositionStringW` and `ImmNotifyIME(NI_COMPOSITIONSTR, CPS_COMPLETE)` on the
  editor window's input context, then checks the document's text through UI Automation.

That second test needs an IME installed on the runner. If windows-latest has none, the first text
commit says so, records the platform part as not yet proven, and does not tick the M0 box.

## Tests that stay deterministic

- Render's goldens never call DirectWrite. They composite synthetic masks (a solid block, a
  gradient, a mask partly outside the image, a colour layer), and check colour, coverage, gamma
  correction, position, clipping, order against redaction and the crop. They run on every host and
  are byte-exact.
- The DirectWrite path is tested on Windows CI by properties, since the rasteriser changes between
  Windows builds:
  - a mask for "Hg" at 24 px has ink, its top sits at the baseline offset from `at`, and two calls
    give the same mask;
  - a two-line block's layout box is one `LINE_HEIGHT * size` taller than one line's;
  - the same Latin text laid out from the system font collection gives a different mask, which
    shows the embedded Inter was used;
  - an emoji gives at least one mask with `Some` colour.
- Drift: for each sample text, the editor's `em_width` (GPUI, at size 1) and win's layout width at
  size 1 agree within 1/64 em. This compares layout box to layout box.

## Who does what

- Core (editor seat): `TextBlock`, `TextEdit`, the key-map state, Inter's bytes and metrics,
  `GlyphMask`, `TextMasks`.
- Render (editor seat): mask compositing, gamma correction, `MissingTextMask`, the synthetic goldens.
- Editor (editor seat): the text tool, the input handler, painting, measuring, the gamma-port test.
- Win and app (capture seat): `win::text::masks`, the in-memory font loader, the text renderer
  callback, the IME test, and passing masks from `deliver` to `flatten`.

## Decision rows this needs (from D-50..D-59)

- Text keeps hard line breaks only, and is SemiBold Inter at a default 24 px.
- Colour emoji export in colour, as on screen, through colour layers.
- The export's fixed gamma and grayscale contrast: the values `CreateRenderingParams` returns on a
  clean windows-latest runner, measured by a CI probe and written into the row with the run that
  measured them.

## A pure-Rust rasteriser instead

A shaper and rasteriser inside render (for example rustybuzz and a glyph rasteriser) would give
byte-exact text goldens on every host and remove the win-to-render mask path. The note does not
prefer it:
- The screen is DirectWrite, so the export would use a different shaper and rasteriser from what the
  user saw. That is the divergence this design exists to prevent.
- It loses DirectWrite's font fallback for CJK and emoji, or needs bundled fonts that P-06 cannot
  afford and fallback logic of our own.
- The crates are not on the allow-list.

Rejected by the orchestrator (2026-10-30): the design stays within the allow-list, so no owner
decision is needed.

## Review answers taken

- One undo step per committed edit, with the IME and unchanged-text rules above.
- `em_width` stays stored, measured at size 1, behind `TextBlock`'s only constructor.
- SemiBold only.
