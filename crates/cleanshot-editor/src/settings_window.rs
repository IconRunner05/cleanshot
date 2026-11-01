use std::path::PathBuf;
use std::rc::Rc;

use cleanshot_core::{
    AfterCapture, Applied, Hotkey, HotkeyAction, Modifiers, SaveFormat, Settings, SettingsNotice,
    SettingsProblem, settings_notice,
};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Div, ElementId, FocusHandle, Global, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, PathPromptOptions, Render, Role, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Task, TitlebarOptions, Toggled, Window,
    WindowBounds, WindowHandle, WindowOptions, div, px, size,
};

use crate::Error;
use crate::text::register_inter;
use crate::theme::{ACCENT, Theme};

type Apply = Rc<dyn Fn(Settings) -> Applied>;

/// The settings window while it is open, so a second Settings click brings it forward.
struct OpenSettings(WindowHandle<SettingsView>);

impl Global for OpenSettings {}

/// Shows the settings window (F-55) for `current`, or brings forward the one already open.
/// Each change is first checked as the settings file would read it, then handed to `apply`,
/// whose outcome the window shows.
///
/// # Errors
///
/// [`Error::FontLoad`] or [`Error::WindowOpen`] if the window could not open.
pub fn open_settings(
    cx: &mut App,
    current: Settings,
    apply: impl Fn(Settings) -> Applied + 'static,
) -> Result<(), Error> {
    let open = cx.try_global::<OpenSettings>().map(|open| open.0);
    if let Some(open) = open
        && open
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return Ok(());
    }
    register_inter(cx)?;
    let apply: Apply = Rc::new(apply);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(460.0), px(620.0)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Cleanshot settings".into()),
            ..TitlebarOptions::default()
        }),
        window_min_size: Some(size(px(400.0), px(400.0))),
        ..WindowOptions::default()
    };
    let handle = cx
        .open_window(options, |window, cx| {
            cx.new(|cx| SettingsView::new(current, apply, window, cx))
        })
        .map_err(|error| Error::WindowOpen(error.into()))?;
    cx.set_global(OpenSettings(handle));
    Ok(())
}

struct SettingsView {
    settings: Settings,
    apply: Apply,
    /// The action whose hotkey the next key press sets.
    recording: Option<HotkeyAction>,
    notice: Option<SharedString>,
    focus: FocusHandle,
    choosing_folder: Option<Task<()>>,
}

impl SettingsView {
    fn new(settings: Settings, apply: Apply, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            settings,
            apply,
            recording: None,
            notice: None,
            focus,
            choosing_folder: None,
        }
    }

    /// `changed` if the settings file can hold it and the app applies it; else what stopped it,
    /// with each hotkey another program holds kept at its old key.
    fn change(&mut self, changed: Settings) {
        let changed = match changed.checked() {
            Ok(changed) => changed,
            Err(problems) => return self.tell(&SettingsNotice::Refused(problems)),
        };
        let (kept, notice) = (self.apply)(changed.clone()).outcome(&self.settings, changed);
        self.settings = kept;
        self.notice = notice.map(|notice| settings_notice(&notice).into());
    }

    fn tell(&mut self, notice: &SettingsNotice) {
        self.notice = Some(settings_notice(notice).into());
    }

    fn set_hotkey(&mut self, action: HotkeyAction, hotkey: Hotkey) {
        match self.settings.hotkeys.used_by_another(action, hotkey) {
            Some(other) => self.tell(&SettingsNotice::HotkeyShared {
                wanted: hotkey,
                other,
            }),
            None => self.edit(|settings| settings.hotkeys.set(action, hotkey)),
        }
    }

    fn edit(&mut self, edit: impl FnOnce(&mut Settings)) {
        let mut changed = self.settings.clone();
        edit(&mut changed);
        self.change(changed);
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let Some(action) = self.recording else {
            if key == "escape" {
                window.remove_window();
            }
            return;
        };
        // F-55: Space or Enter may be the press that started recording, and neither is a hotkey.
        if matches!(key, "space" | "enter") {
            return;
        }
        self.recording = None;
        if !matches!(key, "escape" | "tab") {
            let held = event.keystroke.modifiers;
            let modifiers = Modifiers {
                ctrl: held.control,
                alt: held.alt,
                shift: held.shift,
                win: held.platform,
            };
            match Hotkey::recorded(key, modifiers) {
                Err(refusal) => self.tell(&SettingsNotice::HotkeyRefused(refusal)),
                Ok(hotkey) => self.set_hotkey(action, hotkey),
            }
        }
        cx.notify();
    }

    /// F-55: the dialog runs as GPUI's own task after this click, as Save As does (F-22).
    fn choose_folder(&mut self, window: &Window, cx: &mut Context<Self>) {
        let dialog = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        self.choosing_folder = Some(cx.spawn_in(window, async move |view, cx| {
            let chosen = match dialog.await {
                Ok(Ok(chosen)) => Ok(chosen.and_then(|paths| paths.into_iter().next())),
                Ok(Err(error)) => {
                    eprintln!("the folder dialog did not open: {error:#}");
                    Err(SettingsNotice::NoFolderDialog)
                }
                Err(dropped) => {
                    eprintln!("the folder dialog closed with no answer: {dropped}");
                    Err(SettingsNotice::NoFolderDialog)
                }
            };
            // Err only means the window closed while the dialog was open, with nothing to change.
            let _ = view.update(cx, |view, cx| view.folder_chosen(chosen, cx));
        }));
    }

    fn folder_chosen(
        &mut self,
        chosen: Result<Option<PathBuf>, SettingsNotice>,
        cx: &mut Context<Self>,
    ) {
        self.choosing_folder = None;
        match chosen.map(|folder| folder.map(|folder| folder.into_os_string().into_string())) {
            Ok(None) => {}
            Ok(Some(Ok(folder))) => self.edit(|settings| settings.save_folder = Some(folder)),
            Ok(Some(Err(_))) => self.tell(&SettingsNotice::Refused(vec![
                SettingsProblem::InvalidValue {
                    key: "save_folder".into(),
                },
            ])),
            Err(notice) => self.tell(&notice),
        }
        cx.notify();
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        let settings = &self.settings;
        let capture = section("Capture", theme).child(radio_group(
            "After capture",
            [
                (AfterCapture::ShowToolbar, "Show the toolbar"),
                (AfterCapture::Copy, "Copy to the clipboard"),
            ],
            settings.after_capture,
            theme,
            cx,
            |settings, choice| settings.after_capture = choice,
        ));
        let folder = settings
            .save_folder
            .clone()
            .unwrap_or_else(|| "Pictures\\Screenshots".to_owned());
        let saving = section("Saving", theme)
            .child(
                row()
                    .child(label("Folder", theme))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_color(theme.text)
                            .text_size(px(13.0))
                            .child(folder),
                    )
                    .child(
                        button("Choose folder", "Choose folder", "Choose...", false, theme)
                            .on_click(
                                cx.listener(|view, _, window, cx| view.choose_folder(window, cx)),
                            ),
                    )
                    .child(
                        button("Default folder", "Default folder", "Default", false, theme)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.edit(|settings| settings.save_folder = None);
                                cx.notify();
                            })),
                    ),
            )
            .child(radio_group(
                "Format",
                [
                    (SaveFormat::Png, "PNG"),
                    (SaveFormat::Jpg, "JPG"),
                    (SaveFormat::Bmp, "BMP"),
                ],
                settings.save_format,
                theme,
                cx,
                |settings, choice| settings.save_format = choice,
            ));
        let hotkeys = section("Hotkeys", theme)
            .children(HotkeyAction::ALL.map(|action| self.hotkey_row(action, theme, cx)));
        div()
            .id("settings")
            .role(Role::Application)
            .aria_label("Cleanshot settings")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .overflow_y_scroll()
            .bg(theme.backdrop)
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(capture)
            .child(saving)
            .child(hotkeys)
            .children(self.notice.clone().map(|notice| {
                div()
                    .id("settings-notice")
                    .role(Role::Status)
                    .aria_label(notice.clone())
                    .text_color(theme.text)
                    .text_size(px(13.0))
                    .child(notice)
            }))
    }
}

impl SettingsView {
    /// The action's hotkey on a button that records the next key pressed, and a reset to its
    /// default, the only way back to `PrtSc`, which never reaches a window as a key press.
    fn hotkey_row(
        &self,
        action: HotkeyAction,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let recording = self.recording == Some(action);
        let shown = if recording {
            "Press a hotkey...".to_owned()
        } else {
            self.settings.hotkeys.get(action).to_string()
        };
        let name = format!("{} hotkey: {shown}", action.name());
        row()
            .child(label(action.name(), theme))
            .child(
                button(
                    &format!("{} hotkey", action.name()),
                    &name,
                    &shown,
                    recording,
                    theme,
                )
                .flex_1()
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.recording = Some(action);
                    window.focus(&view.focus, cx);
                    cx.notify();
                })),
            )
            .child(
                button(
                    &format!("Reset {} hotkey", action.name()),
                    &format!("Reset {} hotkey", action.name()),
                    "Reset",
                    false,
                    theme,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.set_hotkey(action, Settings::default().hotkeys.get(action));
                    cx.notify();
                })),
            )
    }
}

/// A titled panel of CHARTER 4's material (F-55).
fn section(title: &'static str, theme: Theme) -> Stateful<Div> {
    div()
        .id(title)
        .role(Role::Group)
        .aria_label(title)
        .flex()
        .flex_col()
        .gap(px(10.0))
        .p(px(14.0))
        .rounded(px(12.0))
        .bg(theme.material)
        .border_1()
        .border_color(theme.hairline)
        .child(
            div()
                .text_color(theme.text)
                .text_size(px(13.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
}

fn row() -> Div {
    div().flex().items_center().gap(px(8.0))
}

fn label(text: &'static str, theme: Theme) -> Div {
    div()
        .w(px(120.0))
        .text_color(theme.text)
        .text_size(px(13.0))
        .child(text)
}

/// A focusable control named `name` for UI Automation, reachable by Tab, activated by click,
/// Enter or Space, accented while `pressed`.
fn control(id: &str, name: &str, role: Role, pressed: bool, theme: Theme) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.to_owned().into()))
        .focusable()
        .tab_stop(true)
        .role(role)
        .aria_label(SharedString::from(name.to_owned()))
        .focus_visible(|control| control.border_2().border_color(ACCENT))
        .h(px(28.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(7.0))
        .text_color(theme.text)
        .text_size(px(13.0))
        .when(pressed, |control| control.bg(theme.hover))
        .when(!pressed, |control| {
            control.hover(move |control| control.bg(theme.hover))
        })
}

fn button(id: &str, name: &str, text: &str, pressed: bool, theme: Theme) -> Stateful<Div> {
    control(id, name, Role::Button, pressed, theme)
        .border_1()
        .border_color(theme.hairline)
        .justify_center()
        .child(text.to_owned())
}

/// A labelled radio group with `current` checked, setting its field through `set` when picked.
fn radio_group<T: Copy + PartialEq + 'static, const N: usize>(
    name: &'static str,
    choices: [(T, &'static str); N],
    current: T,
    theme: Theme,
    cx: &mut Context<SettingsView>,
    set: fn(&mut Settings, T),
) -> impl IntoElement + use<T, N> {
    let buttons = choices.map(|(choice, text)| {
        let checked = choice == current;
        control(
            &format!("{name}: {text}"),
            text,
            Role::RadioButton,
            checked,
            theme,
        )
        .aria_toggled(if checked {
            Toggled::True
        } else {
            Toggled::False
        })
        .on_click(cx.listener(move |view, _, _, cx| {
            view.edit(|settings| set(settings, choice));
            cx.notify();
        }))
        .child(text)
    });
    row().child(label(name, theme)).child(
        div()
            .id(name)
            .role(Role::RadioGroup)
            .aria_label(name)
            .flex()
            .gap(px(4.0))
            .children(buttons),
    )
}
