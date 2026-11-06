use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{Color, KeyAction, Tool};
use gpui::{
    AnyElement, Bounds, BoxShadow, Context, InteractiveElement, IntoElement, ParentElement, Pixels,
    Rgba, Role, StatefulInteractiveElement, Styled, Window, div, hsla, point, px,
};

use super::{EditorView, update_while_open};
use crate::canvas::rgba;
use crate::placement::{CARD_INSET, canvas_size, fitted_placement, framed, shown_area};
use crate::theme::Theme;
use crate::zoom_panel::{OnPanelInput, PanelInput, Viewport, scrollbars, zoom_panel};

impl EditorView {
    /// What the canvas shows now: the fitted placement until the image is zoomed or panned.
    pub(super) fn viewport(&self, window: &Window) -> Viewport {
        let room = canvas_size(window.viewport_size());
        let scale_factor = window.scale_factor();
        let changing_crop = self.tool == Tool::Crop;
        Viewport {
            area: shown_area(&self.document, changing_crop),
            placement: self.placement.unwrap_or_else(|| {
                fitted_placement(&self.document, changing_crop, room, scale_factor)
            }),
            room,
            scale_factor,
        }
    }

    /// The part of the canvas the image's card backs: where the image and its export frame
    /// are, cut to the canvas (D-158).
    pub(super) fn backing(&self, window: &Window) -> Bounds<Pixels> {
        let viewport = self.viewport(window);
        viewport
            .placement
            .covers(viewport.room, framed(viewport.area, self.document.frame()))
    }

    /// The zoom panel and scrollbars over the image card, none while the image is fitted
    /// (D-145).
    pub(super) fn zoom_controls(
        &self,
        window: &Window,
        theme: Theme,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        if self.placement.is_none() {
            return Vec::new();
        }
        let viewport = self.viewport(window);
        let view = cx.entity().downgrade();
        let on_input: OnPanelInput = Rc::new(move |input, window, cx| {
            update_while_open(&view, cx, |view, cx| {
                view.zoom_input(input, window, cx);
            });
        });
        scrollbars(viewport)
            .into_iter()
            .map(IntoElement::into_any_element)
            .chain([zoom_panel(
                Arc::clone(&self.base),
                self.document.image_area(),
                viewport,
                theme,
                Rc::clone(&self.log),
                &on_input,
            )
            .into_any_element()])
            .collect()
    }

    pub(super) fn zoom_input(
        &mut self,
        input: PanelInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = self.viewport(window);
        let centre = point(viewport.room.width / 2.0, viewport.room.height / 2.0);
        match input {
            PanelInput::ZoomBy(factor) => {
                self.placement = Some(viewport.placement.zoomed(factor, centre));
            }
            PanelInput::ActualSize => self.act(KeyAction::ActualSize, window, cx),
            PanelInput::Fit => self.act(KeyAction::FitView, window, cx),
            PanelInput::Grab(at) => {
                self.minimap_held = true;
                self.placement = Some(viewport.placement.centred_on(at, viewport.room));
            }
            PanelInput::Drag(at) => {
                if !self.minimap_held {
                    return;
                }
                self.placement = Some(viewport.placement.centred_on(at, viewport.room));
            }
            PanelInput::Release => {
                if !self.minimap_held {
                    return;
                }
                self.minimap_held = false;
            }
        }
        cx.notify();
    }
}

/// D-146: how strongly the capture's colour glows round its card, as in the U2 and V2 mock-ups.
const GLOW_ALPHA: f32 = 0.28;

/// D-141: the image on a card with rounded corners and a soft shadow, the window showing
/// round it on the left, right and below, with `zoom` over it (D-145) and a soft `glow` of the
/// capture's colour round it (D-146). The card backs only `backing`, the part of the canvas the
/// image covers, so the window's glass or solid surface shows round a zoomed-out image
/// (D-158).
pub(super) fn image_card(
    canvas: impl IntoElement,
    zoom: Vec<AnyElement>,
    glow: Color,
    backing: Bounds<Pixels>,
    theme: Theme,
) -> impl IntoElement {
    div()
        .flex_1()
        .min_h_0()
        .relative()
        .px(CARD_INSET)
        .pb(CARD_INSET)
        .child(
            div()
                .absolute()
                .left(CARD_INSET + backing.origin.x)
                .top(backing.origin.y)
                .w(backing.size.width)
                .h(backing.size.height)
                .rounded(px(10.0))
                .bg(theme.card)
                .shadow(vec![
                    BoxShadow {
                        color: Rgba {
                            a: GLOW_ALPHA,
                            ..rgba(glow)
                        }
                        .into(),
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(90.0),
                        spread_radius: px(0.0),
                        inset: false,
                    },
                    BoxShadow {
                        color: hsla(0.0, 0.0, 0.0, 0.35),
                        offset: point(px(0.0), px(12.0)),
                        blur_radius: px(32.0),
                        spread_radius: px(0.0),
                        inset: false,
                    },
                ]),
        )
        .child(
            div()
                .id("screenshot")
                .role(Role::Image)
                .aria_label("Screenshot")
                .size_full()
                .relative()
                .rounded(px(10.0))
                .overflow_hidden()
                .child(canvas)
                .children(zoom),
        )
}
