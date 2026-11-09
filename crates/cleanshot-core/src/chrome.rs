//! The colours and control corner that the editor and the capture overlay both draw.

use crate::document::Color;

/// The one accent, for the primary action and selection.
pub const ACCENT: Color = opaque(0x0A, 0x84, 0xFF);

/// Text and icons drawn on [`ACCENT`].
pub const ON_ACCENT: Color = opaque(0xFF, 0xFF, 0xFF);

/// The corner radius of buttons, inputs and chips, in pixels.
pub const CONTROL_CORNER: f32 = 7.0;

/// A floating panel's colours for one system appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panel {
    pub material: Color,
    pub hairline: Color,
    pub text: Color,
}

/// A panel under the dark appearance.
pub const DARK: Panel = Panel {
    material: Color {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 0xB8,
    },
    hairline: Color {
        r: 0xFF,
        g: 0xFF,
        b: 0xFF,
        a: 0x1F,
    },
    text: opaque(0xF5, 0xF5, 0xF7),
};

/// A panel under the light appearance.
pub const LIGHT: Panel = Panel {
    material: Color {
        r: 0xF6,
        g: 0xF6,
        b: 0xF8,
        a: 0xB8,
    },
    hairline: Color {
        r: 0x00,
        g: 0x00,
        b: 0x00,
        a: 0x1A,
    },
    text: opaque(0x1D, 0x1D, 0x1F),
};

const fn opaque(r: u8, g: u8, b: u8) -> Color {
    Color { r, g, b, a: 0xFF }
}
