/// What a press on the frozen screen selects: a dragged region, the window
/// under the pointer, or the monitor under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureMode {
    #[default]
    Region,
    Window,
    FullScreen,
}
