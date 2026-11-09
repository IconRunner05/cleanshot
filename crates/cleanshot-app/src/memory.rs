use std::rc::Rc;
use std::time::Duration;

use cleanshot_win::log::memory;
use cleanshot_win::log::{Usage, usage};

use crate::{Session, Warning};

pub(super) const IDLE_SAMPLE: Duration = Duration::from_secs(10);
const AFTER_USE_SAMPLE: Duration = Duration::from_secs(10);
/// A later after-use sample, reported only, which tells memory a driver frees late from
/// memory still held.
const AFTER_USE_LATE_SAMPLE: Duration = Duration::from_secs(30);
/// Overlay time against memory: captures this close together share one Direct2D device; after a
/// longer pause it is let go, before [`AFTER_USE_SAMPLE`] measures what stays.
const GRAPHICS_IDLE: Duration = Duration::from_secs(5);
/// How long after GPUI gives its device back the closed window's renderer is gone too.
const RENDERER_FREED: Duration = Duration::from_secs(6);

impl Session {
    /// Under --log, the usage to measure CPU time from; None without it, so normal running starts
    /// no timer.
    pub(super) fn idle_start(&self) -> Option<Usage> {
        self.log.borrow().as_ref()?;
        usage()
            .inspect_err(|error| {
                self.warn(Warning::Failed("Logging could not sample idle use", error));
            })
            .ok()
    }

    /// Logs the working set now and the CPU time spent since `start`.
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
                "memory working_set_bytes={} idle_private_bytes={}",
                end.working_set_bytes, end.private_working_set_bytes
            )
        });
        self.perf(|_| {
            format!(
                "cpu cpu_ms={} over_ms={}",
                end.cpu.saturating_sub(start.cpu).as_millis(),
                IDLE_SAMPLE.as_millis()
            )
        });
    }

    /// After a capture finishes: if no capture starts in the next [`GRAPHICS_IDLE`], lets go
    /// of the Direct2D factory the capture windows shared, which frees its device once the
    /// thumbnail closes, and of the kept capture devices; under --log samples memory
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
                            "memory graphics_released private_before={before} \
                             private_factory_released={factory_released} private_after={after}"
                        )
                    });
                }
                session.log_memory("devices_released");
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

    /// Under --log, logs what holds the working set now, by kind and size, as of `stage`,
    /// and returns it; None without --log or if it cannot be accounted for.
    pub(super) fn log_memory(&self, stage: &str) -> Option<memory::Held> {
        self.log.borrow().as_ref()?;
        match memory::held() {
            Ok(held) => {
                self.log_line("memory", |_| format!("at={stage} {held}"));
                Some(held)
            }
            Err(error) => {
                self.warn(Warning::Failed(
                    "Logging could not account for memory",
                    &error,
                ));
                None
            }
        }
    }

    /// Under --log, logs the private working set 10 s from now, and again 30 s from now,
    /// reported only, each if no capture starts before then; normal running starts no
    /// timer.
    fn sample_after_use(self: &Rc<Self>, defer: &dyn Fn(Duration, Box<dyn FnOnce()>)) {
        if self.log.borrow().is_none() {
            return;
        }
        let started = self.captures_started.get();
        for (after, stage, prefix) in [
            (AFTER_USE_SAMPLE, "after_use", ""),
            (AFTER_USE_LATE_SAMPLE, "after_use_late", "late_"),
        ] {
            let session = Rc::clone(self);
            defer(
                after,
                Box::new(move || {
                    if session.captures_started.get() != started {
                        return;
                    }
                    let held = session.private_bytes();
                    // Logged before the sample's line, which the field test waits for.
                    let largest = session.log_memory(stage).map_or_else(String::new, |held| {
                        format!(
                            " {prefix}largest_heap_block_bytes={} \
                             {prefix}largest_allocation_bytes={}",
                            held.largest_heap_block(),
                            held.largest_allocation()
                        )
                    });
                    if let Some(held) = held {
                        session.perf(|_| {
                            format!("memory {prefix}private_working_set_bytes={held}{largest}")
                        });
                    }
                }),
            );
        }
    }

    /// Accounts for memory as GPUI gives its graphics device back, and again
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
    /// not judged.
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
                self.perf(|_| format!("memory after_editor_private_bytes={held}"));
            }
        })
        .detach();
    }
}
