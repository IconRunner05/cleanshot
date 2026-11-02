use std::path::{Path, PathBuf};
use std::sync::Arc;

use cleanshot_core::{
    Delivered, DeliveryError, Document, Output, RgbaImage, SaveFormat, Saving, Settings,
};
use cleanshot_render::{Flattened, Format, encode, flatten};
use cleanshot_win::clipboard::copy_image;
use cleanshot_win::save::{self, FolderError, SaveError, Stamp};
use cleanshot_win::text;
use cleanshot_win::thumbnail::Dragged;

const FILE_NAME_TEMPLATE: &str = "Screenshot {date} {time}";

/// Flattens `document`, redaction included (C-22), and copies it (F-20) or saves it in the
/// configured folder and format (F-21). The overlay and the editor both finish here (D-48).
/// A save the organisation's policy turns off is refused before anything is drawn (F-57).
pub(crate) fn deliver(
    output: Output,
    document: &Document,
    settings: &Settings,
    saving: &Saving,
) -> Result<Delivered, DeliveryError> {
    if let (Output::Save, Saving::Off(off)) = (output, saving) {
        return Err(DeliveryError::SavingOff(*off));
    }
    let image = flattened(document)?;
    match output {
        Output::Copy => {
            let bitmap =
                encode(&image, Format::ClipboardBitmap).map_err(|error| from_render(&error))?;
            let png = encode(&image, Format::Png).map_err(|error| from_render(&error))?;
            copy_image(&bitmap, &png).map_err(|cause| {
                eprintln!("the clipboard refused the image: {cause}");
                DeliveryError::ClipboardBusy
            })?;
            Ok(Delivered::Copied)
        }
        Output::Save => {
            let (format, extension) = encoding(settings.save_format);
            let bytes = encode(&image, format).map_err(|error| from_render(&error))?;
            let folder = save::save_folder(settings.save_folder.as_deref()).map_err(from_folder)?;
            save::save(&folder, FILE_NAME_TEMPLATE, Stamp::now(), extension, &bytes)
                .map(Delivered::Saved)
                .map_err(|error| from_save(error, &folder))
        }
    }
}

/// D-94: what a drag from the thumbnail offers: `image` as a PNG named as Save names one
/// (F-21), and as the clipboard's bitmap, made in memory.
pub(crate) fn dragged(image: &Arc<RgbaImage>) -> Result<Dragged, DeliveryError> {
    let image = flattened(&Document::new(Arc::clone(image)))?;
    Ok(Dragged {
        name: offered_name("png"),
        png: encode(&image, Format::Png).map_err(|error| from_render(&error))?,
        bitmap: encode(&image, Format::ClipboardBitmap).map_err(|error| from_render(&error))?,
    })
}

fn flattened(document: &Document) -> Result<Flattened, DeliveryError> {
    let masks = text::masks(document).map_err(|cause| {
        eprintln!("DirectWrite could not draw the text: {cause}");
        DeliveryError::TextNotDrawn
    })?;
    flatten(document, &masks).map_err(|error| from_render(&error))
}

fn encoding(format: SaveFormat) -> (Format, &'static str) {
    match format {
        SaveFormat::Png => (Format::Png, "png"),
        SaveFormat::Jpg => (Format::Jpeg, "jpg"),
        SaveFormat::Bmp => (Format::Bmp, "bmp"),
    }
}

/// F-22 (D-54): the folder Save As opens on and the name it offers, Save's own at this moment,
/// or why the organisation's policy opens no dialog (F-57, D-95).
pub(crate) fn save_as_start(
    settings: &Settings,
    saving: &Saving,
) -> Result<(PathBuf, String), DeliveryError> {
    if let Saving::Off(off) = saving {
        return Err(DeliveryError::SavingOff(*off));
    }
    // A folder that cannot be named leaves the dialog on its own default; the user picks one
    // there, and a save into it reports its own failure.
    let folder = save::save_folder(settings.save_folder.as_deref()).unwrap_or_default();
    let (_, extension) = encoding(settings.save_format);
    Ok((folder, offered_name(extension)))
}

fn offered_name(extension: &str) -> String {
    save::offered_name(FILE_NAME_TEMPLATE, Stamp::now(), extension)
        .unwrap_or_else(|error| panic!("the file-name template is a safe constant: {error}"))
}

/// F-22 (D-66): flattens `document` and writes it to `path`, the name the user confirmed in
/// Save As, replacing a file there. The extension picks the format; any other name is written
/// in the configured format, under exactly that name. The organisation's policy may refuse it
/// (F-57): when saving is off, or for a path outside the policy's folder.
pub(crate) fn save_to(
    path: &Path,
    document: &Document,
    settings: &Settings,
    saving: &Saving,
) -> Result<Delivered, DeliveryError> {
    match saving {
        Saving::Off(off) => return Err(DeliveryError::SavingOff(*off)),
        Saving::Allowed {
            folder: Some(folder),
            ..
        } if !save::is_inside(path, Path::new(folder)) => {
            return Err(DeliveryError::OutsidePolicyFolder {
                folder: folder.clone(),
            });
        }
        Saving::Allowed { .. } => {}
    }
    let image = flattened(document)?;
    // F-57: ForcePng wins over the typed extension.
    let format = match saving {
        Saving::Allowed { png_only: true, .. } => SaveFormat::Png,
        Saving::Allowed { .. } | Saving::Off(_) => {
            typed_format(path).unwrap_or(settings.save_format)
        }
    };
    let (format, _) = encoding(format);
    let bytes = encode(&image, format).map_err(|error| from_render(&error))?;
    let folder = path.parent().unwrap_or(path);
    match save::replace(path, &bytes) {
        Ok(()) => Ok(Delivered::Saved(path.to_owned())),
        Err(SaveError::Write(cause)) => {
            eprintln!("{} could not be written: {cause}", folder.display());
            Err(DeliveryError::NotWritten {
                folder: folder.to_owned(),
            })
        }
        Err(error @ (SaveError::UnsafeName(_) | SaveError::NoFreeName(_))) => {
            eprintln!("{} could not be written: {error}", path.display());
            Err(DeliveryError::NotWritten {
                folder: folder.to_owned(),
            })
        }
        Err(error @ SaveError::UnknownField(_)) => {
            panic!("save::replace expands no file-name template: {error}")
        }
    }
}

fn typed_format(path: &Path) -> Option<SaveFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some(SaveFormat::Png),
        "jpg" | "jpeg" => Some(SaveFormat::Jpg),
        "bmp" => Some(SaveFormat::Bmp),
        _ => None,
    }
}

fn from_render(error: &cleanshot_render::Error) -> DeliveryError {
    match error {
        &cleanshot_render::Error::TooLarge { width, height } => {
            DeliveryError::TooLarge { width, height }
        }
        cleanshot_render::Error::MissingTextMask(_) => DeliveryError::TextNotDrawn,
        cleanshot_render::Error::CropOutsideImage(_)
        | cleanshot_render::Error::Output(_)
        | cleanshot_render::Error::Encode(_) => {
            panic!("a document from core flattens and encodes: {error}")
        }
    }
}

fn from_folder(error: FolderError) -> DeliveryError {
    match error {
        FolderError::NoFolder(cause) => {
            eprintln!("the screenshots folder is unknown: {cause}");
            DeliveryError::NoSaveFolder
        }
        FolderError::RelativeFolder(configured) => DeliveryError::SaveFolderNotFull { configured },
    }
}

fn from_save(error: SaveError, folder: &Path) -> DeliveryError {
    match error {
        SaveError::NoFreeName(name) => DeliveryError::NoFreeName { name },
        SaveError::Write(cause) => {
            eprintln!("{} could not be written: {cause}", folder.display());
            DeliveryError::NotWritten {
                folder: folder.to_owned(),
            }
        }
        SaveError::UnknownField(_) | SaveError::UnsafeName(_) => {
            panic!("the file-name template is a safe constant: {error}")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use cleanshot_core::{
        Color, Command, History, PointF, RectF, RedactMode, RgbaImage, Shape, Style, TextBlock,
    };

    use cleanshot_core::SavingOff;

    use super::*;

    const NO_POLICY: Saving = Saving::Allowed {
        folder: None,
        png_only: false,
    };

    fn redacted_document() -> Document {
        let pixels = (0..=255u8)
            .flat_map(|i| [i.wrapping_mul(37), 0, 255, 255])
            .collect();
        let image = RgbaImage::new(16, 16, pixels).expect("16x16 RGBA");
        let mut history = History::new(Document::new(Arc::new(image)));
        history
            .apply(Command::Add {
                shape: Shape::Redact {
                    rect: RectF {
                        x: 0.0,
                        y: 0.0,
                        width: 16.0,
                        height: 16.0,
                    },
                    mode: RedactMode::Pixelate,
                },
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
            .expect("the redaction is added");
        history.document().clone()
    }

    fn settings_saving_to(folder: &str) -> Settings {
        Settings {
            save_folder: Some(folder.to_owned()),
            save_format: SaveFormat::Bmp,
            ..Settings::default()
        }
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "P-02 budgets the release build: cargo test --release -p cleanshot-app p02_"
    )]
    fn p02_a_4k_region_copy_prints_its_median_for_the_budget_judge() {
        let (width, height) = (3840, 2160);
        let pixels = (0..height)
            .flat_map(|y: u32| {
                (0..width).flat_map(move |x: u32| {
                    let [x0, y0] = [x.to_le_bytes()[0], y.to_le_bytes()[0]];
                    [x0, y0, x0 ^ y0, 255]
                })
            })
            .collect();
        let image = RgbaImage::new(width, height, pixels).expect("3840x2160 RGBA");
        let document = Document::new(Arc::new(image));
        let mut copies: Vec<Duration> = (0..5)
            .map(|_| {
                let started = Instant::now();
                let copied = deliver(Output::Copy, &document, &Settings::default(), &NO_POLICY);
                assert_eq!(copied, Ok(Delivered::Copied));
                started.elapsed()
            })
            .collect();
        copies.sort();
        let median = copies[2];
        // cleanshot-fieldtest --budgets --copy judges this line (P-02, D-90).
        println!("perf P-02 median_us={} all={copies:?}", median.as_micros());
    }

    #[test]
    fn f09_a_drag_offers_the_capture_as_a_png_named_as_save_names_one_and_as_a_bitmap() {
        let image = Arc::new(RgbaImage::new(4, 2, vec![200; 4 * 2 * 4]).expect("4x2 RGBA"));
        let offered = dragged(&image).expect("the drag's formats are made");
        assert!(offered.name.starts_with("Screenshot "), "{}", offered.name);
        assert_eq!(
            Path::new(&offered.name).extension(),
            Some("png".as_ref()),
            "{}",
            offered.name
        );
        assert!(offered.png.starts_with(b"\x89PNG"), "the PNG's signature");
        let expected = encode(
            &flattened(&Document::new(image)).expect("flattened"),
            Format::ClipboardBitmap,
        )
        .expect("the clipboard's bitmap");
        assert_eq!(offered.bitmap, expected);
    }

    #[test]
    fn f22_save_as_picks_the_format_from_the_typed_extension_and_replaces_the_file() {
        let folder = std::env::temp_dir().join(format!("cleanshot-save-as-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("the test folder is created");
        let document = redacted_document();
        let settings = settings_saving_to(folder.to_str().expect("a UTF-8 temp path"));
        let typed = folder.join("Shot.JPG");
        std::fs::write(&typed, b"old").expect("the file to replace is written");
        let bare = folder.join("Shot");
        let saved = [&typed, &bare].map(|path| {
            assert_eq!(
                save_to(path, &document, &settings, &NO_POLICY),
                Ok(Delivered::Saved(path.clone()))
            );
            std::fs::read(path).expect("the saved file is readable")
        });
        std::fs::remove_dir_all(&folder).expect("the test folder is removable");
        assert!(
            saved[0].starts_with(&[0xFF, 0xD8]),
            "Shot.JPG is not a JPEG"
        );
        assert!(
            saved[1].starts_with(b"BM"),
            "Shot is not the configured BMP"
        );
    }

    #[test]
    fn f22_a_save_as_path_with_no_file_name_is_reported_not_fatal() {
        let folder = std::env::temp_dir();
        let settings = settings_saving_to(folder.to_str().expect("a UTF-8 temp path"));
        assert_eq!(
            save_to(
                &folder.join(".."),
                &redacted_document(),
                &settings,
                &NO_POLICY
            ),
            Err(DeliveryError::NotWritten { folder })
        );
    }

    #[test]
    fn c22_a_saved_capture_holds_the_redacted_pixels_not_the_capture() {
        let folder = std::env::temp_dir().join(format!("cleanshot-deliver-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("the test folder is created");
        let document = redacted_document();
        let settings = settings_saving_to(folder.to_str().expect("a UTF-8 temp path"));
        let Ok(Delivered::Saved(path)) = deliver(Output::Save, &document, &settings, &NO_POLICY)
        else {
            panic!("the save did not deliver a file");
        };
        let written = std::fs::read(&path).expect("the saved file is readable");
        std::fs::remove_dir_all(&folder).expect("the test folder is removable");
        let masks = text::masks(&document).expect("the document has no text to draw");
        let flattened = flatten(&document, &masks).expect("the document flattens");
        assert_eq!(written, encode(&flattened, Format::Bmp).expect("encodes"));
        assert_ne!(flattened.image(), document.base().as_ref());
    }

    #[test]
    fn f34_a_saved_document_holds_its_text_drawn_by_directwrite() {
        let folder = std::env::temp_dir().join(format!("cleanshot-text-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("the test folder is created");
        let image = RgbaImage::new(64, 40, vec![255; 64 * 40 * 4]).expect("64x40 RGBA");
        let mut history = History::new(Document::new(Arc::new(image)));
        let block = TextBlock::new(PointF { x: 4.0, y: 4.0 }, "Hg".to_owned(), 24.0, 1.0)
            .expect("a positive size and a measured width");
        history
            .apply(Command::Add {
                shape: Shape::Text(block),
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
            .expect("the text is added");
        let document = history.document().clone();
        let settings = settings_saving_to(folder.to_str().expect("a UTF-8 temp path"));
        let saved = deliver(Output::Save, &document, &settings, &NO_POLICY);
        let written = match &saved {
            Ok(Delivered::Saved(path)) => std::fs::read(path).ok(),
            _ => None,
        };
        std::fs::remove_dir_all(&folder).expect("the test folder is removable");
        let masks = text::masks(&document).expect("DirectWrite draws Hg");
        let flattened = flatten(&document, &masks).expect("the document flattens");
        assert_eq!(
            written,
            Some(encode(&flattened, Format::Bmp).expect("encodes")),
            "{saved:?}"
        );
        assert_ne!(
            flattened.image(),
            document.base().as_ref(),
            "the text left the image white"
        );
    }

    #[test]
    fn f21_each_save_failure_names_what_the_user_can_fix() {
        let document = redacted_document();
        assert_eq!(
            deliver(
                Output::Save,
                &document,
                &settings_saving_to("Shots"),
                &NO_POLICY
            ),
            Err(DeliveryError::SaveFolderNotFull {
                configured: "Shots".to_owned()
            })
        );
        let missing =
            std::env::temp_dir().join(format!("cleanshot-missing-{}", std::process::id()));
        let settings = settings_saving_to(missing.to_str().expect("a UTF-8 temp path"));
        assert_eq!(
            deliver(Output::Save, &document, &settings, &NO_POLICY),
            Err(DeliveryError::NotWritten { folder: missing })
        );
    }

    #[test]
    fn f57_policy_refuses_saves_when_saving_is_off_or_outside_its_folder_and_forces_png() {
        let document = redacted_document();
        let folder = std::env::temp_dir().join(format!("cleanshot-f57-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("the test folder is created");
        let settings = settings_saving_to(folder.to_str().expect("a UTF-8 temp path"));
        let off = Saving::Off(SavingOff::ClipboardOnly);
        assert_eq!(
            deliver(Output::Save, &document, &settings, &off),
            Err(DeliveryError::SavingOff(SavingOff::ClipboardOnly))
        );
        assert_eq!(
            save_as_start(&settings, &off),
            Err(DeliveryError::SavingOff(SavingOff::ClipboardOnly))
        );
        let inside = folder.join("inside.png");
        assert_eq!(
            save_to(&inside, &document, &settings, &off),
            Err(DeliveryError::SavingOff(SavingOff::ClipboardOnly))
        );

        let policy_folder = folder.join("Policy");
        let locked = Saving::Allowed {
            folder: Some(
                policy_folder
                    .to_str()
                    .expect("a UTF-8 temp path")
                    .to_owned(),
            ),
            png_only: true,
        };
        assert_eq!(
            save_to(&inside, &document, &settings, &locked),
            Err(DeliveryError::OutsidePolicyFolder {
                folder: policy_folder
                    .to_str()
                    .expect("a UTF-8 temp path")
                    .to_owned()
            })
        );
        assert!(!inside.exists(), "a refused save writes nothing");
        std::fs::create_dir_all(&policy_folder).expect("the policy folder is created");
        let allowed = policy_folder.join("allowed.jpg");
        assert_eq!(
            save_to(&allowed, &document, &settings, &locked),
            Ok(Delivered::Saved(allowed.clone()))
        );
        let written = std::fs::read(&allowed).expect("the saved file is readable");
        assert!(
            written.starts_with(b"\x89PNG"),
            "ForcePng writes PNG whatever the name"
        );
        std::fs::remove_dir_all(&folder).expect("the test folder is removable");
    }
}
