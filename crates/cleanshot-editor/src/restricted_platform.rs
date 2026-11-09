use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use futures::channel::oneshot;
use gpui::{
    Action, ActivityGuard, AnyWindowHandle, BackgroundExecutor, ClipboardItem, CursorStyle,
    ForegroundExecutor, GraphicalEnvironment, Keymap, Menu, MenuItem, OwnedMenu, PathPromptOptions,
    Platform, PlatformDisplay, PlatformInfo, PlatformKeyboardLayout, PlatformKeyboardMapper,
    PlatformTextSystem, PlatformWindow, PowerMode, Result, ScreenCaptureSource, SystemNotification,
    SystemNotificationResponse, Task, ThermalState, WindowAppearance, WindowParams,
    WindowingRequest,
};
use gpui_windows::WindowsPlatform;
use smallvec::SmallVec;

// GPUI only ever gets this type, so WindowsPlatform's stubbed methods are not linked.
pub(crate) struct RestrictedPlatform(pub(crate) WindowsPlatform);

impl Platform for RestrictedPlatform {
    fn platform_info(&self) -> PlatformInfo {
        self.0.platform_info()
    }

    fn background_executor(&self) -> BackgroundExecutor {
        self.0.background_executor()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.0.foreground_executor()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.0.text_system()
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        self.0.run(on_finish_launching);
    }

    fn quit(&self) {
        self.0.quit();
    }

    fn set_initial_windowing(&self, request: WindowingRequest) {
        self.0.set_initial_windowing(request);
    }

    fn request_windowing(&self, request: WindowingRequest) -> Task<Result<()>> {
        self.0.request_windowing(request)
    }

    // Restarting spawns PowerShell.
    fn restart(&self, _binary_path: Option<PathBuf>, _arguments: Vec<OsString>) {}

    fn activate(&self, ignoring_other_apps: bool) {
        self.0.activate(ignoring_other_apps);
    }

    fn hide(&self) {
        self.0.hide();
    }

    fn hide_other_apps(&self) {
        self.0.hide_other_apps();
    }

    fn unhide_other_apps(&self) {
        self.0.unhide_other_apps();
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        self.0.displays()
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.0.primary_display()
    }

    fn on_displays_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.on_displays_changed(callback);
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.0.active_window()
    }

    fn is_screen_capture_supported(&self) -> bool {
        self.0.is_screen_capture_supported()
    }

    fn screen_capture_sources(
        &self,
    ) -> oneshot::Receiver<Result<Vec<Rc<dyn ScreenCaptureSource>>>> {
        self.0.screen_capture_sources()
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        self.0.open_window(handle, options)
    }

    fn window_appearance(&self) -> WindowAppearance {
        self.0.window_appearance()
    }

    // Opens the URL with ShellExecuteW.
    fn open_url(&self, _url: &str) {}

    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.0.on_open_urls(callback);
    }

    // A URL scheme is registered under HKCU.
    fn register_url_scheme(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Err(io::Error::other("URL schemes are not supported").into()))
    }

    fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        self.0.prompt_for_paths(options)
    }

    fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        self.0.prompt_for_new_path(directory, suggested_name)
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        self.0.can_select_mixed_files_and_dirs()
    }

    // Falls back to ShellExecuteW.
    fn reveal_path(&self, _path: &Path) {}

    // Opens the file with ShellExecuteW.
    fn open_with_system(&self, _path: &Path) {}

    fn on_quit(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.on_quit(callback);
    }

    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.0.on_reopen(callback);
    }

    fn on_system_sleep(&self, callback: Box<dyn FnMut()>) {
        self.0.on_system_sleep(callback);
    }

    fn on_system_wake(&self, callback: Box<dyn FnMut()>) {
        self.0.on_system_wake(callback);
    }

    fn set_menus(&self, menus: Vec<Menu>, keymap: &Keymap) {
        self.0.set_menus(menus, keymap);
    }

    fn get_menus(&self) -> Option<Vec<OwnedMenu>> {
        self.0.get_menus()
    }

    fn set_dock_menu(&self, menu: Vec<MenuItem>, keymap: &Keymap) {
        self.0.set_dock_menu(menu, keymap);
    }

    fn perform_dock_menu_action(&self, action: usize) {
        self.0.perform_dock_menu_action(action);
    }

    // The jump list is a recent-items history kept by the shell.
    fn update_jump_list(
        &self,
        _menus: Vec<MenuItem>,
        _entries: Vec<SmallVec<[PathBuf; 2]>>,
    ) -> Task<Vec<SmallVec<[PathBuf; 2]>>> {
        Task::ready(Vec::new())
    }

    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        self.0.on_app_menu_action(callback);
    }

    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        self.0.on_will_open_app_menu(callback);
    }

    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.0.on_validate_app_menu_command(callback);
    }

    fn thermal_state(&self) -> ThermalState {
        self.0.thermal_state()
    }

    fn on_thermal_state_change(&self, callback: Box<dyn FnMut()>) {
        self.0.on_thermal_state_change(callback);
    }

    fn prevent_idle_sleep(&self, reason: &str) -> Task<Result<ActivityGuard>> {
        self.0.prevent_idle_sleep(reason)
    }

    fn power_mode(&self) -> PowerMode {
        self.0.power_mode()
    }

    fn on_power_mode_change(&self, callback: Box<dyn FnMut()>) {
        self.0.on_power_mode_change(callback);
    }

    fn set_app_identity(&self, identifier: &str, name: &str) {
        self.0.set_app_identity(identifier, name);
    }

    // Showing a toast registers the app under HKCU.
    fn show_system_notification(&self, _notification: SystemNotification) {}

    fn dismiss_system_notification(&self, tag: &str) {
        self.0.dismiss_system_notification(tag);
    }

    fn on_system_notification_response(
        &self,
        callback: Box<dyn FnMut(SystemNotificationResponse)>,
    ) {
        self.0.on_system_notification_response(callback);
    }

    fn graphical_environment(&self) -> Option<GraphicalEnvironment> {
        self.0.graphical_environment()
    }

    fn app_path(&self) -> Result<PathBuf> {
        self.0.app_path()
    }

    fn path_for_auxiliary_executable(&self, name: &str) -> Result<PathBuf> {
        self.0.path_for_auxiliary_executable(name)
    }

    fn set_cursor_style(&self, style: CursorStyle) {
        self.0.set_cursor_style(style);
    }

    fn hide_cursor_until_mouse_moves(&self) {
        self.0.hide_cursor_until_mouse_moves();
    }

    fn is_cursor_visible(&self) -> bool {
        self.0.is_cursor_visible()
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        self.0.should_auto_hide_scrollbars()
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.0.read_from_clipboard()
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        self.0.write_to_clipboard(item);
    }

    // Credentials persist in the Windows Credential Manager.
    fn write_credentials(&self, _url: &str, _username: &str, _password: &[u8]) -> Task<Result<()>> {
        Task::ready(Err(io::Error::other("credentials are not supported").into()))
    }

    fn read_credentials(&self, _url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Ok(None))
    }

    fn delete_credentials(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        self.0.keyboard_layout()
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        self.0.keyboard_mapper()
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.0.on_keyboard_layout_change(callback);
    }
}
