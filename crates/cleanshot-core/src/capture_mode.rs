/// What a press on the frozen screen selects (F-05, F-08, F-11): a dragged region, the window
/// under the pointer, or the monitor under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureMode {
    #[default]
    Region,
    Window,
    FullScreen,
}
