//! Scopes, additive measures and one run record for numerical solvers.
//!
//! A [`Context`] belongs to one problem. Scopes entered while it is current
//! form a tree of nodes keyed by (parent node, call site); every node holds
//! additive `u64` measures: `calls`, `wall` and `busy` in nanoseconds, and the
//! counts added with [`add!`]. [`Context::record`] sums the threads and
//! returns a [`Record`] that writes itself as JSON.
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
//! Without the feature `on`, [`scope!`] and [`add!`] expand to `()` and their
//! arguments are not evaluated.
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
//! # Sequence of a scope or an `add!`
//!
//! 1. One relaxed load of a global atomic, the highest threshold of all live
//!    contexts. A site above it returns here; nothing else is touched.
//! 2. The thread-local state: no current context, or the site's level above
//!    the current context's threshold, returns here.
//! 3. The site id: one relaxed load of the site's static, registered under a
//!    lock on the first use in the process.
//! 4. The child of the current node for this site (for `add!`, the measure
//!    cell of the current node): a linear scan of this thread's cache. The
//!    first time on a thread the node is interned under the context's lock
//!    and this thread's cells are allocated; after that nothing is allocated.
//! 5. A scope reads the clock on entry; on drop it reads the clock again, adds
//!    one call and the elapsed time to this thread's cells and moves the
//!    current node back. An `add!` evaluates its value and adds it.
//!
//! Each thread writes only its own cells (relaxed atomic load and store);
//! [`Context::record`] reads them all.

#![warn(missing_docs)]

mod context;
mod record;

pub use context::{current, now, Clock, Context, Guard, Handle};
pub use record::{Calibration, Node, Param, Record};

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// Version of the record's JSON layout.
const SCHEMA: u32 = 1;

/// Calibration: rounds of empty scopes; the fastest round gives the
/// constants, which drops rounds hit by an interrupt or a migration. The first
/// round warms the caches and is not used.
#[cfg(feature = "on")]
const CALIBRATION_ROUNDS: u32 = 8;

/// Calibration: empty scopes per round. On the benchmark machines a scope
/// costs 20 to 60 ns, so a round is 10 to 30 us, far above the 42 ns clock
/// resolution of Apple Silicon and short enough to fit between interrupts.
#[cfg(feature = "on")]
const CALIBRATION_SCOPES: u32 = 512;

/// Importance of a call site. A site is active when its level is at most the
/// threshold of the current context.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Level(u8);

impl Level {
    /// Errors.
    pub const ERROR: Level = Level(1);
    /// Warnings.
    pub const WARN: Level = Level(2);
    /// Phases of a run; the default level of a site and threshold of a context.
    pub const INFO: Level = Level(3);
    /// Finer scopes, per section.
    pub const DEBUG: Level = Level(4);
    /// Finest scopes, per iteration.
    pub const TRACE: Level = Level(5);

    /// Lower-case name, as written in the record.
    pub fn name(self) -> &'static str {
        ["off", "error", "warn", "info", "debug", "trace"][self.0 as usize]
    }
}

/// A call site: one static per macro expansion, given a dense id on first use.
/// For a scope site the id indexes the sites of the record; for an `add!`
/// site it is the id of the measure of that name.
#[derive(Debug)]
pub struct Site {
    /// Scope or measure name.
    pub name: &'static str,
    /// `module_path!()` of the expansion.
    pub module: &'static str,
    /// `file!()` of the expansion.
    pub file: &'static str,
    /// `line!()` of the expansion.
    pub line: u32,
    /// Level against the context's threshold.
    pub level: Level,
    /// Id plus one; zero until registered.
    id: AtomicU32,
}

impl Site {
    /// A site that is not yet registered.
    pub const fn new(
        name: &'static str,
        module: &'static str,
        file: &'static str,
        line: u32,
        level: Level,
    ) -> Site {
        Site {
            name,
            module,
            file,
            line,
            level,
            id: AtomicU32::new(0),
        }
    }

    /// The id, registered on first use as a scope site or a measure.
    fn id(&'static self, measure: bool) -> u32 {
        match self.id.load(Relaxed) {
            0 => context::register(self, measure),
            id => id - 1,
        }
    }
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
    ($level:ident, $name:expr) => {{
        static SITE: $crate::Site = $crate::Site::new(
            $name,
            module_path!(),
            file!(),
            line!(),
            $crate::Level::$level,
        );
        SITE.scope()
    }};
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
/// the site is active.
#[cfg(feature = "on")]
#[macro_export]
macro_rules! add {
    ($name:expr, $value:expr) => {
        $crate::add!(INFO, $name, $value)
    };
    ($level:ident, $name:expr, $value:expr) => {{
        static SITE: $crate::Site = $crate::Site::new(
            $name,
            module_path!(),
            file!(),
            line!(),
            $crate::Level::$level,
        );
        SITE.add(|| $value as u64)
    }};
}

/// Adds a value to an exact measure at the current node:
/// `add!("iterations", n)` or `add!(DEBUG, "iterations", n)`. Without the
/// feature `on`: the value is type-checked in a branch never taken.
#[cfg(not(feature = "on"))]
#[macro_export]
macro_rules! add {
    ($name:expr, $value:expr) => {
        if false {
            let _ = $value;
        }
    };
    ($level:ident, $name:expr, $value:expr) => {
        $crate::add!($name, $value)
    };
}
