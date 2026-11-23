//! The bridge to `log`, both ways: a binary of its own, since the logger is a
//! process global.
#![cfg(all(feature = "on", feature = "log"))]

use rsprobe::{scope, Buffer, Context, LogSink, Logger};
use std::sync::Mutex;

/// Keeps the text of each record and passes it on to `rsprobe::Logger`.
struct Capture(Mutex<Vec<String>>);

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{} {}", r.level(), r.args()));
        Logger.log(r);
    }
    fn flush(&self) {}
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

#[test]
fn log_records_become_events_and_events_become_records() {
    log::set_logger(&CAPTURE).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
    let buffer = Buffer::default();
    let ctx = Context::builder()
        .sink(buffer.clone())
        .sink(LogSink)
        .build();
    {
        let _on = ctx.enter();
        let _s = scope!("solve");
        log::info!("from log {}", 1);
        log::debug!("below the threshold");
        rsprobe::warn!("from rsprobe");
    }
    let mut events = Vec::new();
    buffer.drain(|e, path| events.push((e.level.name(), path.to_string(), e.kind.to_string())));
    assert_eq!(
        events,
        [
            ("info", "solve".into(), "from log 1".into()),
            ("warn", "solve".into(), "from rsprobe".into()),
        ]
    );
    let got = CAPTURE.0.lock().unwrap().clone();
    assert_eq!(
        got,
        [
            "INFO from log 1",
            "INFO solve: from log 1",
            "DEBUG below the threshold",
            "WARN solve: from rsprobe",
        ]
    );
}
