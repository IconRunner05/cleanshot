use std::rc::Rc;
use std::time::Duration;

use cleanshot_win::log::memory;
use cleanshot_win::log::{Usage, usage};

use crate::{Session, Warning};

pub(super) const IDLE_SAMPLE: Duration = Duration::from_secs(10);
const AFTER_USE_SAMPLE: Duration = Duration::from_secs(10);
/// P-01 against P-04: captures this close together share one Direct2D device; after a
/// longer pause it is let go, before [`AFTER_USE_SAMPLE`] measures what stays.
const GRAPHICS_IDLE: Duration = Duration::from_secs(5);
/// How long after GPUI gives its device back the closed window's renderer is gone too (D-140).
const RENDERER_FREED: Duration = Duration::from_secs(6);

impl Session {
    /// Under --log, the usage to measure P-05 from; None without it, so normal running starts
    /// no timer (P-05).
    pub(super) fn idle_start(&self) -> Option<Usage> {
        self.log.borrow().as_ref()?;
        usage()
            .inspect_err(|error| {
                self.warn(Warning::Failed("Logging could not sample idle use", error));
            })
            .ok()
    }

    /// Logs the P-04 working set now and the P-05 CPU time spent since `start`.
    pub(super) fn idle_use(&self, start: Usage) {
        let end = match usage() {
            Ok(end) => end,
            Err(error) => {
                return self.warn(Warning::Failed("Logging could not sample idle use", &error));
            }
        };
        // Logged before the sample's lines, which the field test waits for.
        self.log_memory("idle");
        self.perf(|_| {
            format!(
                "P-04 working_set_bytes={} idle_private_bytes={}",
                end.working_set_bytes, end.private_working_set_bytes
            )
        });
        self.perf(|_| {
            format!(
                "P-05 cpu_ms={} over_ms={}",
                end.cpu.saturating_sub(start.cpu).as_millis(),
                IDLE_SAMPLE.as_millis()
            )
        });
    }

    /// After a capture finishes: if no capture starts in the next [`GRAPHICS_IDLE`], lets go
    /// of the Direct2D factory the capture windows shared, which frees its device once the
    /// thumbnail closes, and of the kept capture devices (P-04); under --log samples memory
    /// after use.
    pub(super) fn after_use(self: &Rc<Self>, defer: &dyn Fn(Duration, Box<dyn FnOnce()>)) {
        let started = self.captures_started.get();
        let session = Rc::clone(self);
        defer(
            GRAPHICS_IDLE,
            Box::new(move || {
                if session.captures_started.get() != started {
                    return;
                }
                let before = session.private_bytes();
                cleanshot_win::release_factory();
                let factory_released = session.private_bytes();
                session.devices.borrow_mut().release();
                if let (Some(before), Some(factory_released), Some(after)) =
                    (before, factory_released, session.private_bytes())
                {
                    session.perf(|_| {
                        format!(
                            "P-04 graphics_released private_before={before} \
                             private_factory_released={factory_released} private_after={after}"
                        )
                    });
                }
            }),
        );
        self.sample_after_use(defer);
    }

    /// Under --log, the private working set now; None without it, or if it cannot be read.
    fn private_bytes(&self) -> Option<usize> {
        self.log.borrow().as_ref()?;
        usage()
            .inspect_err(|error| {
                self.warn(Warning::Failed(
                    "Logging could not sample memory after use",
                    error,
                ));
            })
            .ok()
            .map(|held| held.private_working_set_bytes)
    }

    /// Under --log, logs what holds the working set now, by kind and size, as of `stage`
    /// (P-04).
    pub(super) fn log_memory(&self, stage: &str) {
        if self.log.borrow().is_none() {
            return;
        }
        match memory::held() {
            Ok(held) => self.log_line("memory", |_| format!("P-04 at={stage} {held}")),
            Err(error) => self.warn(Warning::Failed(
                "Logging could not account for memory",
                &error,
            )),
        }
    }

    /// Under --log, logs the P-04 private working set 10 s from now if no capture starts
    /// before then; normal running starts no timer (P-05).
    fn sample_after_use(self: &Rc<Self>, defer: &dyn Fn(Duration, Box<dyn FnOnce()>)) {
        if self.log.borrow().is_none() {
            return;
        }
        let started = self.captures_started.get();
        let session = Rc::clone(self);
        defer(
            AFTER_USE_SAMPLE,
            Box::new(move || {
                if session.captures_started.get() != started {
                    return;
                }
                let held = session.private_bytes();
                // Logged before the sample's line, which the field test waits for.
                session.log_memory("after_use");
                if let Some(held) = held {
                    session.perf(|_| format!("P-04 private_working_set_bytes={held}"));
                }
            }),
        );
    }

    /// D-140: accounts for memory as GPUI gives its graphics device back, and again
    /// [`RENDERER_FREED`] later, once the closed window's renderer has been freed.
    pub(super) fn sample_graphics_released(self: Rc<Self>, cx: &mut cleanshot_editor::App) {
        self.log_memory("graphics_released");
        cx.spawn(async move |cx| {
            cx.background_executor().timer(RENDERER_FREED).await;
            self.log_memory("graphics_released_6s");
        })
        .detach();
    }

    /// Under --log, logs the private working set past [`GRAPHICS_IDLE`] and
    /// [`AFTER_USE_SAMPLE`] after an editor closed, if no capture starts before then; reported,
    /// not judged (P-04).
    pub(super) fn sample_after_editor(self: Rc<Self>, cx: &mut cleanshot_editor::App) {
        let started = self.captures_started.get();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(GRAPHICS_IDLE + AFTER_USE_SAMPLE)
                .await;
            if self.captures_started.get() != started {
                return;
            }
            let held = self.private_bytes();
            self.log_memory("after_editor");
            if let Some(held) = held {
                self.perf(|_| format!("P-04 after_editor_private_bytes={held}"));
            }
        })
        .detach();
    }
}
