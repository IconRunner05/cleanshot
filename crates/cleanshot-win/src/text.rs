use std::cell::RefCell;

use cleanshot_core::{Color, Document, GlyphMask, INTER, PointF, Shape, TextBlock, TextMasks};
use windows::Win32::Foundation::{DWRITE_E_NOCOLOR, E_INVALIDARG, RECT};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_NONE, ID2D1Brush, ID2D1RenderTarget,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_COLOR_GLYPH_RUN, DWRITE_FACTORY_TYPE_ISOLATED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_GLYPH_RUN,
    DWRITE_GLYPH_RUN_DESCRIPTION, DWRITE_GRID_FIT_MODE, DWRITE_GRID_FIT_MODE_DEFAULT,
    DWRITE_MATRIX, DWRITE_MEASURING_MODE, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_OUTLINE_THRESHOLD_ANTIALIASED, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_RENDERING_MODE1, DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
    DWRITE_RENDERING_MODE1_OUTLINE, DWRITE_STRIKETHROUGH, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE, DWRITE_TEXTURE_ALIASED_1x1, DWRITE_UNDERLINE,
    DWriteCreateFactory, IDWriteFactory2, IDWriteFactory3, IDWriteFactory5, IDWriteFontCollection,
    IDWriteFontFace3, IDWriteInMemoryFontFileLoader, IDWriteInlineObject,
    IDWritePixelSnapping_Impl, IDWriteRenderingParams, IDWriteTextFormat, IDWriteTextRenderer,
    IDWriteTextRenderer_Impl,
};
use windows::Win32::System::SystemServices::LOCALE_NAME_MAX_LENGTH;
use windows::core::{BOOL, ComObject, HSTRING, IUnknown, Ref, implement};

use crate::Error;

/// F-34: the glyph coverage of every text shape in `document`, made by the glyph path GPUI
/// draws text with on screen: Inter from core's bytes, laid out by DirectWrite with natural
/// measuring, each line on core's baseline, raw grayscale coverage with no gamma, and one
/// layer per colour for colour glyphs.
///
/// # Errors
///
/// [`Error::Text`] if DirectWrite refuses the font, a layout or a glyph run.
pub fn masks(document: &Document) -> Result<TextMasks, Error> {
    let fonts = Fonts::inter().map_err(Error::Text)?;
    let mut masks = TextMasks::default();
    for entry in document.shapes() {
        let Shape::Text(block) = &entry.shape else {
            continue;
        };
        let layers = fonts
            .layers(block)
            .map_err(Error::Text)?
            .into_iter()
            .map(|layer| {
                GlyphMask::new(
                    layer.left,
                    layer.top,
                    layer.width,
                    layer.height,
                    layer.coverage,
                    layer.color,
                )
            })
            .collect::<Result<_, _>>()?;
        masks.insert(entry.id, layers);
    }
    Ok(masks)
}

/// The fonts one text shape is laid out with: a family from a collection, with DirectWrite's
/// system fallback for characters the family lacks.
pub(crate) struct Fonts {
    factory: IDWriteFactory5,
    collection: IDWriteFontCollection,
    family: HSTRING,
    locale: HSTRING,
    _loader: InterLoader,
}

/// Inter's in-memory loader, registered on its isolated factory until this drops. Inter's
/// font file is valid only while the loader is registered.
struct InterLoader {
    factory: IDWriteFactory5,
    loader: IDWriteInMemoryFontFileLoader,
}

impl Drop for InterLoader {
    fn drop(&mut self) {
        // SAFETY: the loader was registered on this factory in Fonts::inter, and is
        // unregistered once, here.
        let unregistered = unsafe { self.factory.UnregisterFontFileLoader(&self.loader) };
        debug_assert!(unregistered.is_ok(), "Inter's loader unregisters once");
    }
}

/// A text style the overlay draws with, sealed with the Fonts it reads Inter through, so no
/// DirectWrite object leaves it: the Fonts' Drop unregisters Inter's loader, after which a
/// format drawing Inter reads freed memory (F-05).
pub(crate) struct Label {
    format: IDWriteTextFormat,
    _fonts: Fonts,
}

impl Label {
    /// The same style with its text centred both ways in the box it is drawn into.
    pub(crate) fn centred(self) -> windows::core::Result<Self> {
        // SAFETY: the format is owned by `self`.
        unsafe {
            self.format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            self.format
                .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        }
        Ok(self)
    }

    /// Draws `text` in this style into `area` of `target` with `brush`.
    pub(crate) fn draw(
        &self,
        target: &ID2D1RenderTarget,
        text: &str,
        area: &D2D_RECT_F,
        brush: &ID2D1Brush,
    ) {
        let text: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: every argument is borrowed for the call, and the format's Fonts live in
        // `self`.
        unsafe {
            target.DrawText(
                &text,
                &self.format,
                area,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
}

impl Fonts {
    /// C-21: Inter is read from core's bytes in memory, never from a file, through a loader
    /// registered on an isolated factory until this value drops.
    pub(crate) fn inter() -> windows::core::Result<Self> {
        // SAFETY: creating a factory takes no memory of ours.
        let factory: IDWriteFactory5 =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) }?;
        let length =
            u32::try_from(INTER.len()).map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
        // SAFETY: creating and registering a loader takes no memory of ours.
        let loader = unsafe {
            let loader = factory.CreateInMemoryFontFileLoader()?;
            factory.RegisterFontFileLoader(&loader)?;
            InterLoader {
                factory: factory.clone(),
                loader,
            }
        };
        // SAFETY: INTER is 'static, so the in-memory file reference never outlives its bytes.
        let collection = unsafe {
            let file = loader.loader.CreateInMemoryFontFileReference(
                &factory,
                INTER.as_ptr().cast(),
                length,
                None,
            )?;
            let builder = factory.CreateFontSetBuilder()?;
            builder.AddFontFile(&file)?;
            factory.CreateFontCollectionFromFontSet(&builder.CreateFontSet()?)?
        };
        // SAFETY: the collection holds Inter alone, so it has a family 0 with a name 0, and
        // the name buffer is sized from the length DirectWrite reports.
        let family = unsafe {
            let names = collection.GetFontFamily(0)?.GetFamilyNames()?;
            let mut name = vec![0; names.GetStringLength(0)? as usize + 1];
            names.GetString(0, &mut name)?;
            HSTRING::from_wide(&name[..name.len() - 1])
        };
        Ok(Self {
            factory,
            collection: collection.into(),
            family,
            locale: user_locale()?,
            _loader: loader,
        })
    }

    /// The family in its semibold weight at `size` pixels, sealed with these fonts.
    pub(crate) fn label(self, size: f32) -> windows::core::Result<Label> {
        Ok(Label {
            format: self.format(size)?,
            _fonts: self,
        })
    }

    /// The family in its semibold weight at `size` pixels, the one text style Cleanshot draws.
    fn format(&self, size: f32) -> windows::core::Result<IDWriteTextFormat> {
        // SAFETY: every argument is owned by this call or by `self`.
        unsafe {
            self.factory.CreateTextFormat(
                &self.family,
                &self.collection,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                &self.locale,
            )
        }
    }

    /// Every glyph layer of `block`, in painting order.
    fn layers(&self, block: &TextBlock) -> windows::core::Result<Vec<Layer>> {
        let format = self.format(block.size())?;
        let mut layers = Vec::new();
        for (line, baseline) in block.lines() {
            let text: Vec<u16> = line.encode_utf16().collect();
            let runs = ComObject::new(GlyphRuns {
                factory: self.factory.clone(),
                baseline,
                layers: RefCell::default(),
            });
            // SAFETY: the layout and the renderer live until Draw returns, and Draw calls the
            // renderer on this thread only.
            unsafe {
                let layout =
                    self.factory
                        .CreateTextLayout(&text, &format, f32::INFINITY, f32::INFINITY)?;
                layout.Draw(None, &runs.to_interface::<IDWriteTextRenderer>(), 0.0, 0.0)?;
            }
            layers.append(&mut runs.layers.borrow_mut());
        }
        Ok(layers)
    }
}

fn user_locale() -> windows::core::Result<HSTRING> {
    let mut name = [0; LOCALE_NAME_MAX_LENGTH as usize];
    // SAFETY: the buffer is writable and as long as the call is told.
    let length = unsafe { GetUserDefaultLocaleName(&mut name) };
    let Some(without_nul) = usize::try_from(length)
        .ok()
        .and_then(|length| length.checked_sub(1))
    else {
        return Err(windows::core::Error::from_thread());
    };
    Ok(HSTRING::from_wide(&name[..without_nul]))
}

/// One glyph layer's coverage, before core checks its shape.
#[derive(Debug, PartialEq)]
struct Layer {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    coverage: Vec<u8>,
    color: Option<Color>,
}

#[expect(
    clippy::inline_always,
    clippy::ref_as_ptr,
    reason = "in the code #[implement] generates"
)]
mod renderer {
    use super::{
        BOOL, DWRITE_GLYPH_RUN, DWRITE_GLYPH_RUN_DESCRIPTION, DWRITE_MATRIX, DWRITE_MEASURING_MODE,
        DWRITE_STRIKETHROUGH, DWRITE_UNDERLINE, IDWriteFactory5, IDWriteInlineObject,
        IDWritePixelSnapping_Impl, IDWriteTextRenderer, IDWriteTextRenderer_Impl, IUnknown, Layer,
        PointF, Ref, RefCell, implement, rasterise,
    };

    /// The renderer DirectWrite draws one line's glyph runs to. Each run is placed on that line's
    /// baseline from core, not on the layout's own line placement.
    #[implement(IDWriteTextRenderer)]
    pub(super) struct GlyphRuns {
        pub(super) factory: IDWriteFactory5,
        pub(super) baseline: PointF,
        pub(super) layers: RefCell<Vec<Layer>>,
    }

    #[allow(non_snake_case)]
    impl IDWriteTextRenderer_Impl for GlyphRuns_Impl {
        fn DrawGlyphRun(
            &self,
            _context: *const core::ffi::c_void,
            baseline_x: f32,
            _layout_baseline_y: f32,
            _measuring: DWRITE_MEASURING_MODE,
            run: *const DWRITE_GLYPH_RUN,
            _description: *const DWRITE_GLYPH_RUN_DESCRIPTION,
            _effect: Ref<IUnknown>,
        ) -> windows::core::Result<()> {
            // SAFETY: DirectWrite passes a glyph run that stays valid for this call.
            let run = unsafe { &*run };
            let layers = rasterise(
                &self.factory,
                run,
                self.baseline.x + baseline_x,
                self.baseline.y,
            )?;
            self.layers.borrow_mut().extend(layers);
            Ok(())
        }

        fn DrawUnderline(
            &self,
            _context: *const core::ffi::c_void,
            _x: f32,
            _y: f32,
            _underline: *const DWRITE_UNDERLINE,
            _effect: Ref<IUnknown>,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn DrawStrikethrough(
            &self,
            _context: *const core::ffi::c_void,
            _x: f32,
            _y: f32,
            _strikethrough: *const DWRITE_STRIKETHROUGH,
            _effect: Ref<IUnknown>,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn DrawInlineObject(
            &self,
            _context: *const core::ffi::c_void,
            _x: f32,
            _y: f32,
            _object: Ref<IDWriteInlineObject>,
            _sideways: BOOL,
            _right_to_left: BOOL,
            _effect: Ref<IUnknown>,
        ) -> windows::core::Result<()> {
            Ok(())
        }
    }

    #[allow(non_snake_case)]
    impl IDWritePixelSnapping_Impl for GlyphRuns_Impl {
        /// GPUI places glyphs at their unsnapped advances, so runs keep their exact origins.
        fn IsPixelSnappingDisabled(
            &self,
            _context: *const core::ffi::c_void,
        ) -> windows::core::Result<BOOL> {
            Ok(true.into())
        }

        fn GetCurrentTransform(
            &self,
            _context: *const core::ffi::c_void,
            transform: *mut DWRITE_MATRIX,
        ) -> windows::core::Result<()> {
            // SAFETY: DirectWrite passes a writable matrix.
            unsafe {
                *transform = DWRITE_MATRIX {
                    m11: 1.0,
                    m12: 0.0,
                    m21: 0.0,
                    m22: 1.0,
                    dx: 0.0,
                    dy: 0.0,
                };
            }
            Ok(())
        }

        fn GetPixelsPerDip(
            &self,
            _context: *const core::ffi::c_void,
        ) -> windows::core::Result<f32> {
            Ok(1.0)
        }
    }
}

use renderer::GlyphRuns;

/// The layers of one glyph run with its baseline at (`x`, `y`) in image pixels: one per colour
/// layer for a run with colour glyphs (GPUI `direct_write.rs:1006-1031`), otherwise one in the
/// shape's colour, rendered in the mode DirectWrite recommends (`direct_write.rs:777-813`).
fn rasterise(
    factory: &IDWriteFactory5,
    run: &DWRITE_GLYPH_RUN,
    x: f32,
    y: f32,
) -> windows::core::Result<Vec<Layer>> {
    let colour_factory: &IDWriteFactory2 = factory;
    // SAFETY: `run` is valid for this call; DirectWrite copies what it keeps.
    let colour_runs = unsafe {
        colour_factory.TranslateColorGlyphRun(
            x,
            y,
            run,
            None,
            DWRITE_MEASURING_MODE_NATURAL,
            None,
            0,
        )
    };
    let colour_runs = match colour_runs {
        Ok(colour_runs) => colour_runs,
        Err(error) if error.code() == DWRITE_E_NOCOLOR => {
            let Some(face) = run.fontFace.as_ref() else {
                return Ok(Vec::new());
            };
            let face: IDWriteFontFace3 = windows::core::Interface::cast(face)?;
            let (mut mode, mut grid) = (
                DWRITE_RENDERING_MODE1::default(),
                DWRITE_GRID_FIT_MODE::default(),
            );
            // SAFETY: both outputs are writable locals.
            unsafe {
                face.GetRecommendedRenderingMode(
                    run.fontEmSize,
                    96.0,
                    96.0,
                    None,
                    run.isSideways.as_bool(),
                    DWRITE_OUTLINE_THRESHOLD_ANTIALIASED,
                    DWRITE_MEASURING_MODE_NATURAL,
                    None::<&IDWriteRenderingParams>,
                    &raw mut mode,
                    &raw mut grid,
                )
            }?;
            if mode == DWRITE_RENDERING_MODE1_OUTLINE {
                mode = DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC;
            }
            return Ok(coverage(factory, run, mode, grid, x, y, None)?
                .into_iter()
                .collect());
        }
        Err(error) => return Err(error),
    };
    let mut layers = Vec::new();
    // SAFETY: each current run is read before the next MoveNext, which is when it is freed.
    while unsafe { colour_runs.MoveNext() }?.as_bool() {
        // SAFETY: GetCurrentRun points at the run MoveNext just made current.
        let layer: &DWRITE_COLOR_GLYPH_RUN = unsafe { &*colour_runs.GetCurrentRun()? };
        let color = (layer.paletteIndex != u16::MAX).then(|| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a channel clamped to 0..=1 scales into 0..=255"
            )]
            let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            Color {
                r: channel(layer.runColor.r),
                g: channel(layer.runColor.g),
                b: channel(layer.runColor.b),
                a: channel(layer.runColor.a),
            }
        });
        layers.extend(coverage(
            factory,
            &layer.glyphRun,
            DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
            DWRITE_GRID_FIT_MODE_DEFAULT,
            layer.baselineOriginX,
            layer.baselineOriginY,
            color,
        )?);
    }
    Ok(layers)
}

/// Raw 8-bit grayscale coverage of `run`, or `None` for a run with no ink.
fn coverage(
    factory: &IDWriteFactory3,
    run: &DWRITE_GLYPH_RUN,
    mode: DWRITE_RENDERING_MODE1,
    grid: DWRITE_GRID_FIT_MODE,
    x: f32,
    y: f32,
    color: Option<Color>,
) -> windows::core::Result<Option<Layer>> {
    // SAFETY: `run` is valid for this call, and the texture buffer is sized from its bounds.
    unsafe {
        let analysis = factory.CreateGlyphRunAnalysis(
            run,
            None,
            mode,
            DWRITE_MEASURING_MODE_NATURAL,
            grid,
            DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
            x,
            y,
        )?;
        let bounds: RECT = analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)?;
        let (Ok(width @ 1..), Ok(height @ 1..)) = (
            u32::try_from(bounds.right - bounds.left),
            u32::try_from(bounds.bottom - bounds.top),
        ) else {
            return Ok(None);
        };
        let mut coverage = vec![0; width as usize * height as usize];
        analysis.CreateAlphaTexture(
            DWRITE_TEXTURE_ALIASED_1x1,
            &raw const bounds,
            &mut coverage,
        )?;
        Ok(Some(Layer {
            left: bounds.left,
            top: bounds.top,
            width,
            height,
            coverage,
            color,
        }))
    }
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cleanshot_core::{
        Command, History, INTER_ASCENT, INTER_DESCENT, INTER_UNITS_PER_EM, LINE_HEIGHT, RgbaImage,
        Style,
    };
    use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_METRICS, IDWriteFontCollection1};

    use super::*;

    const SIZE: f32 = 24.0;

    /// A text block whose line box starts well below and right of the origin, so a glyph run
    /// placed from the layout's own origin lands far from where core puts it.
    fn block(text: &str) -> TextBlock {
        TextBlock::new(PointF { x: 10.5, y: 20.25 }, text.to_owned(), SIZE, 1.0)
            .expect("a positive size and a measured width")
    }

    fn document(text: &str) -> Document {
        let image = RgbaImage::new(1, 1, vec![0; 4]).expect("1x1 RGBA");
        let mut history = History::new(Document::new(Arc::new(image)));
        history
            .apply(Command::Add {
                shape: Shape::Text(block(text)),
                style: Style {
                    color: Color {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    },
                    stroke_width: 1.0,
                },
            })
            .expect("adding a shape always applies");
        history.document().clone()
    }

    fn metrics(fonts: &Fonts) -> DWRITE_FONT_METRICS {
        let mut metrics = DWRITE_FONT_METRICS::default();
        // SAFETY: `metrics` is writable, and Inter's collection has a family 0 with a font 0.
        unsafe {
            fonts
                .collection
                .GetFontFamily(0)
                .expect("Inter's family")
                .GetFont(0)
                .expect("Inter SemiBold")
                .CreateFontFace()
                .expect("Inter's face")
                .GetMetrics(&raw mut metrics);
        }
        metrics
    }

    fn top(layers: &[Layer]) -> f64 {
        f64::from(
            layers
                .iter()
                .map(|layer| layer.top)
                .min()
                .expect("the text has ink"),
        )
    }

    #[test]
    fn f34_directwrite_reads_the_embedded_inter_with_cores_metrics() {
        let metrics = metrics(&Fonts::inter().expect("Inter loads"));
        assert_eq!(
            (metrics.designUnitsPerEm, metrics.ascent, metrics.descent),
            (INTER_UNITS_PER_EM, INTER_ASCENT, INTER_DESCENT)
        );
    }

    #[test]
    fn f34_the_cap_height_of_hg_sits_on_cores_baseline_below_at() {
        let fonts = Fonts::inter().expect("Inter loads");
        let block = block("Hg");
        let layers = fonts.layers(&block).expect("Hg is drawn");
        assert!(
            layers.iter().all(|layer| layer.color.is_none()),
            "Inter has no colour glyphs"
        );
        assert!(
            layers
                .iter()
                .any(|layer| layer.coverage.iter().any(|&c| c > 0)),
            "Hg has ink"
        );
        let (_, baseline) = block.lines().next().expect("one line");
        let metrics = metrics(&fonts);
        let cap_height = f32::from(metrics.capHeight) / f32::from(metrics.designUnitsPerEm) * SIZE;
        let expected = f64::from(baseline.y - cap_height).floor();
        assert!(
            (top(&layers) - expected).abs() <= 1.0,
            "the ink starts at row {} where H's cap height puts it at {expected}",
            top(&layers)
        );
    }

    #[test]
    fn f34_each_line_is_drawn_one_line_height_below_the_last() {
        let fonts = Fonts::inter().expect("Inter loads");
        let first = fonts.layers(&block("H")).expect("H is drawn");
        let both = fonts.layers(&block("H\nH")).expect("two lines are drawn");
        let second: Vec<Layer> = both.into_iter().skip(first.len()).collect();
        let step = top(&second) - top(&first);
        assert!(
            (step - f64::from(LINE_HEIGHT * SIZE)).abs() <= 1.0,
            "the second line is {step} px below the first"
        );
    }

    #[test]
    fn f34_a_documents_masks_are_the_same_on_every_call() {
        let document = document("Hg");
        let id = document.shapes()[0].id;
        let first = masks(&document).expect("the masks are made");
        assert!(first.layers(id).is_some_and(|layers| !layers.is_empty()));
        assert_eq!(masks(&document).expect("the masks are made again"), first);
    }

    #[test]
    fn f34_a_text_with_no_ink_still_has_its_layers_recorded() {
        let document = document("  ");
        let id = document.shapes()[0].id;
        assert_eq!(
            masks(&document).expect("the masks are made").layers(id),
            Some(&[][..])
        );
    }

    #[test]
    fn f34_the_same_text_from_the_system_fonts_gives_a_different_mask() {
        let inter = Fonts::inter().expect("Inter loads");
        let mut system: Option<IDWriteFontCollection1> = None;
        // SAFETY: `system` is writable.
        unsafe {
            inter
                .factory
                .GetSystemFontCollection(false, &raw mut system, false)
        }
        .expect("the system font collection");
        let system = Fonts {
            collection: system.expect("a system font collection").into(),
            ..Fonts::inter().expect("Inter loads")
        };
        let block = block("Hg");
        assert_ne!(
            system
                .layers(&block)
                .expect("Hg is drawn from system fonts"),
            inter.layers(&block).expect("Hg is drawn from Inter")
        );
    }

    #[test]
    fn f34_an_emoji_gives_a_layer_in_its_own_colour() {
        let layers = Fonts::inter()
            .expect("Inter loads")
            .layers(&block("\u{1F600}"))
            .expect("the emoji is drawn");
        assert!(
            layers.iter().any(|layer| layer.color.is_some()),
            "the emoji's layers: {:?}",
            layers.iter().map(|layer| layer.color).collect::<Vec<_>>()
        );
    }
}
