use std::panic::{self, Location};

use crate::log::Log;
use crate::tray::message_box;

/// D-120: what the crash notice says, fixed, so no panic message, path or title reaches it.
pub const NOTICE: &str = "Cleanshot hit an internal error and closed.";

/// D-120: makes every panic show [`NOTICE`] before the process aborts, and first write a
/// `panic` line naming only where it happened to `log`, when `--log` opened one (D-109).
pub fn install(log: Option<Log>) {
    install_with(log, |text| {
        // The process is about to abort and has no console, so a box that cannot show has
        // nowhere left to be reported.
        let _ = message_box("Cleanshot", text);
    });
}

fn install_with(log: Option<Log>, notify: impl Fn(&str) + Send + Sync + 'static) {
    panic::set_hook(Box::new(move |info| {
        if let Some(log) = &log {
            // The process is about to abort, so a line that cannot be written has nowhere
            // left to be reported.
            let _ = log.write(&panic_line(info.location()));
        }
        notify(NOTICE);
    }));
}

fn panic_line(location: Option<&Location<'_>>) -> String {
    match location {
        Some(at) => format!("panic at {}:{}:{}", at.file(), at.line(), at.column()),
        None => String::from("panic"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn d120_a_panic_logs_only_where_it_happened_and_shows_the_fixed_notice() {
        let folder = std::env::temp_dir().join(format!("cleanshot-crash-{}", std::process::id()));
        let settings = folder.join("cleanshot.toml");
        fs::create_dir_all(&folder).expect("the test folder is created");
        let log = Log::open(&settings).expect("the log opens");
        let shown = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&shown);
        install_with(Some(log), move |text| {
            sink.lock()
                .expect("no test panics while holding the notices")
                .push(text.to_owned());
        });
        let line = line!() + 1;
        let caught = panic::catch_unwind(|| panic!("C:\\Users\\someone\\secret window.png"));
        drop(panic::take_hook());
        let written = fs::read_to_string(folder.join("cleanshot.log")).expect("the log reads back");
        fs::remove_dir_all(&folder).expect("the test folder is removed");
        assert!(caught.is_err(), "the panic still unwinds to its caller");
        let events: Vec<&str> = written
            .lines()
            .map(|line| line.split_once(' ').expect("a time, then the event").1)
            .collect();
        let here = format!("panic at {}:{line}:", file!());
        assert!(
            events.iter().any(|event| event.starts_with(&here)),
            "{events:?} names {here}"
        );
        assert!(
            !written.contains("secret"),
            "{written:?} holds no panic message"
        );
        assert!(
            shown
                .lock()
                .expect("the hook is gone")
                .iter()
                .any(|text| text == NOTICE),
            "the notice showed"
        );
    }
}
