//! Scopes, additive measures and one run record for numerical solvers.
//!
//! A [`Context`] belongs to one problem. Scopes entered while it is current
//! form a tree of nodes keyed by (parent node, call site); every node holds
//! additive `u64` measures: `calls`, `wall` and `busy` in nanoseconds, the
//! counts added with [`add!`], the times added with [`time!`] and, with the
//! feature `alloc` where the binary installs [`Alloc`], `bytes`, `allocs` and
//! `freed`. [`Context::record`] sums the threads and returns a [`Record`] that
//! writes itself as JSON.
//!
//! ```
//! let ctx = rsprobe::Context::new();
//! let _on = ctx.enter();
//! {
//!     let _s = rsprobe::scope!("factor");
//!     rsprobe::add!("nnz", 1200);
//! }
//! let json = ctx.record().to_string();
//! # assert!(json.starts_with('{'));
//! ```
//!
//! Without the feature `on`, every macro expands to `()` and its arguments are
//! not evaluated.
//!
//! # Events
//!
//! [`error!`], [`warn!`], [`info!`], [`debug!`], [`trace!`] and [`progress!`]
//! emit an [`Event`] at the current node; the text is formatted only when the
//! site is active. The context passes each event to its [`Sink`]s and keeps
//! the distinct events at WARN and above for the record.
//!
//! # Threads
//!
//! The current node is thread-local. [`current`] returns it as a `Copy`
//! [`Handle`]; [`Handle::enter`] on another thread makes that node current
//! there, so scopes entered by a pool task hang under the node that started
//! the section. Elapsed time on a thread entered through [`Context::enter`]
//! adds to `wall`, on a thread entered through a handle to `busy`; the handle
//! guard's own span adds to `busy` of the handle's node.
//!
//! # Memory
//!
//! With the feature `alloc` every guard reads this thread's [`Alloc`] counters
//! on entry and on exit and adds the differences to its node, inclusive like
//! time. Without it a guard carries no counters: they cost 1.5 to 7 ns per
//! scope on x86-64 (examples/bench.rs). The allocations of
//! rsprobe itself (a node's first use on a thread, the calibration) are not
//! counted.
//!
//! # Sequence of a scope or an `add!`
//!
//! 1. One relaxed load of a global atomic, the highest threshold of all live
//!    contexts. A site above it returns here; nothing else is touched.
//! 2. The thread-local state: no current context returns here.
//! 3. The site id: one relaxed load of the site's static, registered under a
//!    lock on the first use in the process.
//! 4. One byte of this thread's entry, indexed by the site id: whether the
//!    site is active in the current context (its level against the threshold
//!    or the filter of its module), decided on the first use on this thread.
//!    An inactive site returns here.
//! 5. The child of the current node for this site (for `add!`, the measure
//!    cell of the current node): a linear scan of this thread's cache. The
//!    first time on a thread the node is interned under the context's lock
//!    and this thread's cells are allocated; after that nothing is allocated.
//! 6. A scope reads the clock on entry; on drop it reads the clock again, adds
//!    one call and the elapsed time to this thread's cells and moves the
//!    current node back. An `add!` evaluates its value and adds it.
//!
//! Each thread writes only its own cells (relaxed atomic load and store);
//! [`Context::record`] reads them all.

#![warn(missing_docs)]

mod alloc;
#[cfg(feature = "log")]
mod bridge;
mod context;
mod events;
mod guard;
mod record;
mod sinks;
mod site;
mod stream;
mod thread;

pub use alloc::Alloc;
#[cfg(feature = "log")]
pub use bridge::{LogSink, Logger};
pub use context::{now, Builder, Clock, Context};
pub use events::{Event, Kind};
pub use guard::{current, Guard, Handle};
pub use record::{Calibration, Node, Param, Record};
pub use sinks::{Buffer, Path, Sink, Stderr};
pub use site::{Level, Site};

/// Version of the record's JSON layout.
const SCHEMA: u32 = 1;

/// Calibration: empty scopes timed after a pass of the same count that warms
/// the caches. A scope costs 45 ns with the default clock (examples/bench.rs
/// on a Ryzen 9 9900X under Linux and on an Apple M3), so a pass takes about
/// 0.2 ms. The constants are means over the pass: on Apple Silicon the clock
/// ticks every 42 ns, about the cost of a scope, and a minimum over short
/// rounds was measured to return an inner constant of 0.
const CALIBRATION_SCOPES: u32 = 4096;

/// Distinct events at WARN and above a context keeps for its record; further
/// ones are counted as dropped.
const WARNINGS: usize = 100;

/// The static call site of one macro expansion, as `&'static Site`.
#[doc(hidden)]
#[macro_export]
macro_rules! site {
    ($level:ident, $name:expr $(, $kind:ident)?) => {{
        static SITE: $crate::Site = $crate::Site::new(
            $name,
            module_path!(),
            file!(),
            line!(),
            $crate::Level::$level,
        )$(.$kind())?;
        &SITE
    }};
}

/// The first group with the feature `on`, the second without.
#[doc(hidden)]
#[cfg(feature = "on")]
#[macro_export]
macro_rules! on_or {
    ({ $($on:tt)* } { $($off:tt)* }) => { $($on)* };
}

#[doc(hidden)]
#[cfg(not(feature = "on"))]
#[macro_export]
macro_rules! on_or {
    ({ $($on:tt)* } { $($off:tt)* }) => { $($off)* };
}

/// Enters a scope at the current node and returns its guard:
/// `scope!("name")` at level INFO, `scope!(DEBUG, "name")` at a given level.
/// The guard adds one call and the elapsed time on drop.
#[cfg(feature = "on")]
#[macro_export]
macro_rules! scope {
    ($name:expr) => {
        $crate::scope!(INFO, $name)
    };
    ($level:ident, $name:expr) => {
        $crate::site!($level, $name).scope()
    };
}

/// Enters a scope at the current node and returns its guard:
/// `scope!("name")` or `scope!(DEBUG, "name")`. Without the feature `on`: `()`.
#[cfg(not(feature = "on"))]
#[macro_export]
macro_rules! scope {
    ($($t:tt)*) => {
        ()
    };
}

/// Adds a value to an exact measure at the current node:
/// `add!("iterations", n)` at level INFO, `add!(DEBUG, "iterations", n)` at a
/// given level. The value is converted with `as u64` and evaluated only when
/// the site is active. Without the feature `on`: the value is type-checked in
/// a branch never taken.
#[macro_export]
macro_rules! add {
    ($name:expr, $value:expr) => {
        $crate::add!(INFO, $name, $value)
    };
    ($level:ident, $name:expr, $value:expr) => {
        $crate::on_or!(
            { $crate::site!($level, $name).add(|| $value as u64) }
            { if false { let _ = $value; } }
        )
    };
}

/// Adds a duration in ns to a noisy measure at the current node, one the
/// caller timed itself (a wait, a predicted time): `time!("wait", ns)` or
/// `time!(DEBUG, "wait", ns)`. As [`add!`]; the record declares the measure
/// noisy, and a comparison treats it as `wall`.
#[macro_export]
macro_rules! time {
    ($name:expr, $ns:expr) => {
        $crate::time!(INFO, $name, $ns)
    };
    ($level:ident, $name:expr, $ns:expr) => {
        $crate::on_or!(
            { $crate::site!($level, $name, noisy).add(|| $ns as u64) }
            { if false { let _ = $ns; } }
        )
    };
}

/// Emits a message event at the current node: `error!("fmt", args..)` at
/// level ERROR. The text is formatted only when the site is active. Without the
/// feature `on`: the arguments are type-checked in a branch never taken.
#[macro_export]
macro_rules! error {
    ($($a:tt)+) => { $crate::message!(ERROR, $($a)+) };
}

/// [`error!`] at level WARN.
#[macro_export]
macro_rules! warn {
    ($($a:tt)+) => { $crate::message!(WARN, $($a)+) };
}

/// [`error!`] at level INFO.
#[macro_export]
macro_rules! info {
    ($($a:tt)+) => { $crate::message!(INFO, $($a)+) };
}

/// [`error!`] at level DEBUG.
#[macro_export]
macro_rules! debug {
    ($($a:tt)+) => { $crate::message!(DEBUG, $($a)+) };
}

/// [`error!`] at level TRACE.
#[macro_export]
macro_rules! trace {
    ($($a:tt)+) => { $crate::message!(TRACE, $($a)+) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! message {
    ($level:ident, $($a:tt)+) => {
        $crate::on_or!(
            { $crate::site!($level, "").event(|| $crate::Kind::Message(::std::format!($($a)+))) }
            { if false { let _ = ::std::format_args!($($a)+); } }
        )
    };
}

/// Emits a progress event at the current node: `progress!(done, total)` at
/// level INFO, `progress!(DEBUG, done, total)` at a given level. The values
/// are converted with `as u64` and evaluated only when the site is active.
/// Without the feature `on`: the values are type-checked in a branch never
/// taken.
#[macro_export]
macro_rules! progress {
    ($done:expr, $total:expr) => {
        $crate::progress!(INFO, $done, $total)
    };
    ($level:ident, $done:expr, $total:expr) => {
        $crate::on_or!(
            { $crate::site!($level, "").event(|| $crate::Kind::Progress { done: $done as u64, total: $total as u64 }) }
            { if false { let _ = ($done, $total); } }
        )
    };
}
