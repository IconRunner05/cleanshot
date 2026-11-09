use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use cleanshot_core::{
    Applied, HotkeyAction, OptionsEntry, OptionsItem, Picked, Settings, SettingsSaveError,
    StartupEntryError, options_menu, run_command, starts,
};
use cleanshot_win::settings_file::{settings_location, write_settings};
use cleanshot_win::startup::{self, ExePathError, remove_run_value, run_value, set_run_value};
use cleanshot_win::tray::Request;

use crate::{Session, Warning, region_hotkey, to_front};

/// Opens the settings window or brings it to the front, or says in the tray why it could not.
pub(super) fn open_settings(cx: &mut cleanshot_editor::App, session: &Rc<Session>) {
    let reading = Rc::clone(session);
    let current = move || reading.settings.borrow().clone();
    let applying = Rc::clone(session);
    let apply = move |changed| apply_settings(&applying, changed);
    let opened = cleanshot_editor::open_settings(
        cx,
        current,
        session.policy.clone(),
        apply,
        starts_with_windows(session),
        {
            let logging = Rc::clone(session);
            move |on| set_start_with_windows(&logging, on)
        },
        {
            let logging = Rc::clone(session);
            move |event| logging.log_line("settings", |_| event.to_owned())
        },
    );
    match opened {
        Ok(window) => to_front(session, "settings", window),
        Err(error) => session.warn(Warning::Failed("The settings window did not open", &error)),
    }
}

/// Whether the Run value starts this exe. One Windows cannot read counts as off,
/// and is logged under --log.
fn starts_with_windows(session: &Session) -> bool {
    let Some(exe) = exe_path(session) else {
        return false;
    };
    match run_value() {
        Ok(command) => command.is_some_and(|command| starts(&command, &exe)),
        Err(error) => {
            session.log_line("startup", |_| format!("unread {error}"));
            false
        }
    }
}

/// Adds this exe's Run value, or removes it.
fn set_start_with_windows(session: &Session, on: bool) -> Result<(), StartupEntryError> {
    if !on {
        return remove_run_value();
    }
    let exe = exe_path(session).ok_or(StartupEntryError::NoExePath)?;
    set_run_value(&run_command(&exe))
}

/// This exe's full path, or None when Windows cannot give it or it is not Unicode text. Why
/// goes to the log, never the path itself, which can hold the user's name.
fn exe_path(session: &Session) -> Option<String> {
    startup::exe_path()
        .inspect_err(|error| {
            session.log_line("startup", |_| match error {
                ExePathError::Unreadable(error) => {
                    format!("exe_path unreadable {:?} {error}", error.kind())
                }
                ExePathError::NotUnicode => String::from("exe_path not_unicode"),
            });
        })
        .ok()
}

/// The Options menu for the settings and policy in force when it opens.
pub(super) fn options_for(session: &Rc<Session>) -> impl Fn() -> Vec<OptionsEntry> + 'static {
    let session = Rc::clone(session);
    move || options_menu(&session.settings.borrow(), &session.policy)
}

/// Applies a pick on the Options menu as the settings window applies a change.
/// Save folder... asks `choose_folder` for the folder.
pub(super) fn apply_option(
    session: &Session,
    item: OptionsItem,
    choose_folder: impl FnOnce() -> Result<Option<PathBuf>, cleanshot_win::Error>,
) {
    let current = session.settings.borrow().clone();
    let changed = match item.choose(&current) {
        Picked::Changed(changed) => changed,
        Picked::ChooseFolder => match choose_folder() {
            Ok(None) => return,
            // Read again; an editor may have changed a look while the dialog was open.
            Ok(Some(folder)) => match folder.into_os_string().into_string() {
                Ok(folder) => Settings {
                    save_folder: Some(folder),
                    ..session.settings.borrow().clone()
                },
                Err(_unnamed) => {
                    return session.warn(Warning::Said {
                        kind: "folder not_kept",
                        text: "That folder's name cannot be kept in the settings. Choose another folder.",
                    });
                }
            },
            Err(error) => {
                return session.warn(Warning::Failed("The folder dialog could not open", &error));
            }
        },
    };
    if let Err(error) = apply_settings(session, changed).saved {
        session.warn(Warning::Failed("The setting could not be saved", &error));
    }
}

/// Starts each editor with the looks in the settings, and keeps each change an
/// editor's strip makes in the settings in force and the settings file. A failed write is
/// logged each time and shown once.
pub(super) fn remember_looks(cx: &mut cleanshot_editor::App, session: &Rc<Session>) {
    let looks = session.settings.borrow().looks.clone();
    let session = Rc::clone(session);
    let shown = Cell::new(false);
    cleanshot_editor::remember_looks(cx, looks, move |looks, _| {
        let changed = Settings {
            looks: looks.clone(),
            ..session.settings.borrow().clone()
        };
        let Err(error) = apply_settings(&session, changed).saved else {
            return;
        };
        if shown.replace(true) {
            session.log_failure("settings", "looks_not_saved", &error);
        } else {
            session.warn(Warning::Failed(
                "Your tool looks could not be saved",
                &error,
            ));
        }
    });
}

/// Moves each changed hotkey, keeps the old key of one another program holds,
/// and writes the settings in force to the settings file.
fn apply_settings(session: &Session, changed: Settings) -> Applied {
    let before = session.settings.borrow().clone();
    let wanted: Vec<_> = HotkeyAction::ALL
        .into_iter()
        .filter(|&action| changed.hotkeys.get(action) != before.hotkeys.get(action))
        .map(|action| {
            let hotkey = changed.hotkeys.get(action);
            let registered = match action {
                HotkeyAction::Region => {
                    let (hotkey, warning) = region_hotkey(hotkey);
                    if let Some(warning) = warning {
                        session.warn(Warning::Said {
                            kind: "hotkey region",
                            text: &warning,
                        });
                    }
                    hotkey
                }
                HotkeyAction::FullScreen
                | HotkeyAction::LastRegion
                | HotkeyAction::Window
                | HotkeyAction::CapturePanel => hotkey,
            };
            (Request::Capture(action), registered)
        })
        .collect();
    let taken: Vec<_> = session
        .tray
        .borrow_mut()
        .set_hotkeys(&wanted)
        .into_iter()
        .filter_map(|request| match request {
            Request::Capture(action) => Some(action),
            Request::EditLastCapture
            | Request::SaveLastCapture
            | Request::OpenSettings
            | Request::Exit => None,
        })
        .collect();
    let in_force = Applied {
        saved: Ok(()),
        taken: taken.clone(),
    };
    let (kept, _) = in_force.outcome(&before, changed);
    let saved = match settings_location().map(|location| location.path) {
        Ok(path) => write_settings(&path, &kept, &|event, error| {
            session.log_failure("settings", event, error);
        }),
        Err(error) => {
            session.log_line("settings", |_| format!("not_saved {error}"));
            Err(SettingsSaveError::NoSettingsPath)
        }
    };
    *session.settings.borrow_mut() = kept;
    Applied { saved, taken }
}
