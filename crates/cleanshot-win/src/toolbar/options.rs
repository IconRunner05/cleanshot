use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use cleanshot_core::{OptionsEntry, OptionsItem};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DestroyMenu, DestroyWindow, GetForegroundWindow,
    HMENU, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, PostMessageW, SetForegroundWindow,
    TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_VERTICAL, TPMPARAMS, TrackPopupMenuEx, WM_NULL,
    WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{HRESULT, HSTRING, w};

use crate::struct_size;

/// The native popup menu of `entries`, destroyed with this value.
pub(crate) struct Menu(pub(crate) HMENU);

impl Menu {
    /// One item per entry, its command id one more than its index, checked and greyed as
    /// the entry says, with a separator between Save folder, the formats and the switches.
    pub(crate) fn of(entries: &[OptionsEntry]) -> windows::core::Result<Self> {
        // SAFETY: the menu is owned by the returned value, which destroys it.
        let menu = Self(unsafe { CreatePopupMenu() }?);
        let mut group = None;
        for (id, entry) in (1..).zip(entries) {
            let this = section(entry.item);
            if group.is_some_and(|previous| previous != this) {
                // SAFETY: appends to this value's own menu.
                unsafe { AppendMenuW(menu.0, MF_SEPARATOR, 0, None) }?;
            }
            group = Some(this);
            let mut flags = MF_STRING;
            if entry.checked {
                flags |= MF_CHECKED;
            }
            if entry.off.is_some() {
                flags |= MF_GRAYED;
            }
            let text = match entry.off {
                Some(why) => format!("{} ({why})", entry.item.label()),
                None => entry.item.label().to_owned(),
            };
            // SAFETY: appends to this value's own menu; the string outlives the call.
            unsafe { AppendMenuW(menu.0, flags, id, &HSTRING::from(text)) }?;
        }
        Ok(menu)
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        // SAFETY: the menu was made by Menu::of and is destroyed once, here.
        let destroyed = unsafe { DestroyMenu(self.0) };
        debug_assert!(destroyed.is_ok(), "a menu made here is destroyed once");
    }
}

/// The menu's sections, a separator between each.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Folder,
    Format,
    Switch,
}

fn section(item: OptionsItem) -> Section {
    match item {
        OptionsItem::SaveFolder => Section::Folder,
        OptionsItem::Format(_) => Section::Format,
        OptionsItem::ShowPointer
        | OptionsItem::ShowThumbnail
        | OptionsItem::RememberLastSelection => Section::Switch,
    }
}

/// F-12 (D-103): shows the Options menu of `entries` beside `button`, in screen pixels, and
/// returns the item picked, or None when the menu was dismissed. The menu belongs to a
/// hidden window of its own, made foreground while it shows, as Windows needs for a click
/// elsewhere to close it; the foreground goes back to its earlier owner afterwards.
pub(crate) fn pick(
    button: RECT,
    entries: &[OptionsEntry],
) -> windows::core::Result<Option<OptionsItem>> {
    let menu = Menu::of(entries)?;
    // SAFETY: reads which window is in the foreground.
    let previous = unsafe { GetForegroundWindow() };
    // SAFETY: a hidden window of a system class, destroyed below on this thread.
    let owner = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("STATIC"),
            None,
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
    }?;
    let params = TPMPARAMS {
        cbSize: struct_size::<TPMPARAMS>(),
        rcExclude: button,
    };
    // SAFETY: the menu and the owner live until after the call, and the owner is destroyed
    // once, here.
    let chosen = unsafe {
        // A refused SetForegroundWindow leaves the menu open until a key or a click on it.
        let _ = SetForegroundWindow(owner);
        let chosen = TrackPopupMenuEx(
            menu.0,
            (TPM_RETURNCMD | TPM_NONOTIFY | TPM_VERTICAL | TPM_LEFTALIGN).0,
            button.left,
            button.bottom,
            owner,
            Some(&raw const params),
        );
        // WM_NULL is the documented way to let the menu close; the menu has already
        // returned, so a post that fails changes nothing.
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        let destroyed = DestroyWindow(owner);
        debug_assert!(destroyed.is_ok(), "the menu's owner is destroyed once");
        // A refused SetForegroundWindow leaves the focus where Windows puts it.
        let _ = SetForegroundWindow(previous);
        chosen
    };
    Ok(usize::try_from(chosen.0)
        .ok()
        .and_then(|id| id.checked_sub(1))
        .and_then(|index| entries.get(index))
        .map(|entry| entry.item))
}

/// F-12 Save folder...: the folder dialog, owned by `owner`, and the folder chosen, or None
/// when it was cancelled.
pub(crate) fn choose_folder(owner: HWND) -> windows::core::Result<Option<PathBuf>> {
    // SAFETY: COM is released below exactly when this call initialised it.
    let initialised = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    // SAFETY: reads which window is in the foreground.
    let previous = unsafe { GetForegroundWindow() };
    let chosen = folder_from_dialog(owner);
    // SAFETY: the dialog has closed, so the foreground goes back where it was.
    // CoUninitialize balances a successful CoInitializeEx, after every COM object here is
    // dropped.
    unsafe {
        // A refused SetForegroundWindow leaves the focus where Windows puts it.
        let _ = SetForegroundWindow(previous);
        if initialised {
            CoUninitialize();
        }
    }
    chosen
}

fn folder_from_dialog(owner: HWND) -> windows::core::Result<Option<PathBuf>> {
    // SAFETY: FileOpenDialog is the system class for IFileOpenDialog.
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }?;
    // SAFETY: calls on a live dialog; its strings outlive the calls.
    unsafe {
        dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
        dialog.SetTitle(w!("Save folder"))?;
        match dialog.Show(Some(owner)) {
            Err(error) if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => {
                return Ok(None);
            }
            shown => shown?,
        }
        let path = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let folder = PathBuf::from(OsString::from_wide(path.as_wide()));
        CoTaskMemFree(Some(path.0.cast()));
        Ok(Some(folder))
    }
}

/// Answers the Options menu from another thread, as a person at the keyboard would.
#[cfg(test)]
pub(crate) mod answering {
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_KEYDOWN,
    };
    use windows::core::w;

    /// Waits on its own thread for this process's popup menu to show, runs `meanwhile`, then
    /// presses `keys` on the menu.
    pub(crate) fn answer_menu(
        keys: &'static [VIRTUAL_KEY],
        meanwhile: impl FnOnce() + Send + 'static,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let menu = loop {
                if let Some(menu) = shown_menu() {
                    break menu;
                }
                assert!(Instant::now() < deadline, "the Options menu never showed");
                thread::sleep(Duration::from_millis(20));
            };
            meanwhile();
            for key in keys {
                // SAFETY: posting copies no memory; the menu window is live while it shows.
                unsafe { PostMessageW(Some(menu), WM_KEYDOWN, WPARAM(key.0.into()), LPARAM(0)) }
                    .expect("the menu takes the key");
            }
        })
    }

    fn shown_menu() -> Option<HWND> {
        let mut after = None;
        loop {
            // SAFETY: a search of top-level windows by the system menu class.
            let menu = unsafe { FindWindowExW(None, after, w!("#32768"), None) }.ok()?;
            let mut process = 0;
            // SAFETY: `menu` came from FindWindowExW and `process` is writable.
            unsafe { GetWindowThreadProcessId(menu, Some(&raw mut process)) };
            // SAFETY: reads of a window's style and of this process's id.
            let ours =
                unsafe { IsWindowVisible(menu).as_bool() && process == GetCurrentProcessId() };
            if ours {
                return Some(menu);
            }
            after = Some(menu);
        }
    }
}

#[cfg(test)]
mod tests {
    use cleanshot_core::SaveFormat;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMenuItemCount, GetMenuItemInfoW, GetMenuState, MENU_ITEM_FLAGS, MENUITEMINFOW,
        MF_BYPOSITION, MIIM_STRING,
    };
    use windows::core::PWSTR;

    use super::*;

    fn entry(item: OptionsItem, checked: bool, off: Option<&'static str>) -> OptionsEntry {
        OptionsEntry { item, checked, off }
    }

    /// Each line of `menu` as "text [checked] [greyed]", and "-" for a separator.
    fn lines(menu: &Menu) -> Vec<String> {
        // SAFETY: reads of a live menu into buffers sized for them.
        unsafe {
            let count = u32::try_from(GetMenuItemCount(Some(menu.0))).expect("a menu counts");
            (0..count)
                .map(|at| {
                    let state = MENU_ITEM_FLAGS(GetMenuState(menu.0, at, MF_BYPOSITION));
                    if state.contains(MF_SEPARATOR) {
                        return "-".to_owned();
                    }
                    let mut text = [0u16; 128];
                    let mut info = MENUITEMINFOW {
                        cbSize: struct_size::<MENUITEMINFOW>(),
                        fMask: MIIM_STRING,
                        dwTypeData: PWSTR(text.as_mut_ptr()),
                        cch: 128,
                        ..Default::default()
                    };
                    GetMenuItemInfoW(menu.0, at, true, &raw mut info).expect("the item reads");
                    let mut line = String::from_utf16_lossy(&text[..info.cch as usize]);
                    if state.contains(MF_CHECKED) {
                        line.push_str(" [checked]");
                    }
                    if state.contains(MF_GRAYED) {
                        line.push_str(" [greyed]");
                    }
                    line
                })
                .collect()
        }
    }

    #[test]
    fn f12_the_options_menu_shows_each_entry_checked_greyed_and_sectioned_as_given() {
        let off = Some("Set by your organisation");
        let menu = Menu::of(&[
            entry(OptionsItem::SaveFolder, false, off),
            entry(OptionsItem::Format(SaveFormat::Png), true, off),
            entry(OptionsItem::Format(SaveFormat::Jpg), false, off),
            entry(OptionsItem::ShowPointer, false, None),
            entry(OptionsItem::ShowThumbnail, true, None),
        ])
        .expect("the menu is made");
        assert_eq!(
            lines(&menu),
            [
                "Save folder... (Set by your organisation) [greyed]",
                "-",
                "PNG (Set by your organisation) [checked] [greyed]",
                "JPG (Set by your organisation) [greyed]",
                "-",
                "Show pointer",
                "Show a thumbnail [checked]",
            ]
        );
    }
}
