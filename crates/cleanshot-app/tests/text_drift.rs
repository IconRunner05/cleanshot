//! The editor's text box and the exported text measure the same, within 1/64 em, so a
//! committed text block is not wider or narrower in the saved image than on screen. GPUI must
//! own the process's main thread, so this test has no harness and ends the process with the
//! verdict.

use std::process::exit;

const TEXTS: [&str; 7] = [
    "Screenshot",
    "AVATAR Ty To WAVE",
    "1 234 567,89 \u{20ac}",
    "Gr\u{f6}\u{df}e \u{e9}t\u{e9} \u{c5}ngstr\u{f6}m",
    "  indented",
    "one\na longer second line",
    "smile \u{1f600} done",
];

const TOLERANCE: f32 = 1.0 / 64.0;

fn main() {
    let ran = cleanshot_editor::run(|cx| {
        let editor = cleanshot_editor::em_widths(&TEXTS, cx).map_err(|error| error.to_string());
        match editor.and_then(|editor| drift(&editor)) {
            Ok(largest) => {
                println!("the_editor_and_the_export_measure_text_alike ... ok");
                println!("largest drift {largest} em");
                exit(0);
            }
            Err(reason) => {
                println!("the_editor_and_the_export_measure_text_alike ... FAILED");
                println!("{reason}");
                exit(1);
            }
        }
    });
    println!("GPUI stopped before the test finished: {ran:?}");
    exit(1);
}

/// The largest drift in ems between `editor`'s widths and the export's, or every text that
/// drifts more than [`TOLERANCE`].
fn drift(editor: &[f32]) -> Result<f32, String> {
    let export = cleanshot_win::text::em_widths(&TEXTS).map_err(|error| error.to_string())?;
    let drifted: Vec<String> = TEXTS
        .iter()
        .zip(editor.iter().zip(&export))
        .filter(|(_, (editor, export))| (*editor - *export).abs() > TOLERANCE)
        .map(|(text, (editor, export))| format!("{text:?}: editor {editor} em, export {export} em"))
        .collect();
    if drifted.is_empty() {
        Ok(editor
            .iter()
            .zip(&export)
            .map(|(editor, export)| (editor - export).abs())
            .fold(0.0, f32::max))
    } else {
        Err(drifted.join("\n"))
    }
}
