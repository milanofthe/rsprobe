//! Reads rsprobe run records and traces: `report`, `compare`, `ab`, `scale`
//! and `critical`, as library functions on [`rsprobe::Record`]s and as the
//! binary `rsprobe`.
//!
//! In a program or a test the record of a context goes in without a file:
//!
//! ```
//! let ctx = rsprobe::Context::new();
//! let runs = [rsprobe_tool::Run::new("solve", ctx.record())];
//! let text = rsprobe_tool::report::text(&runs, &[], false);
//! assert!(text.starts_with("1 record."));
//! ```

pub mod ab;
pub mod compare;
pub mod constants;
pub mod critical;
pub mod fmt;
pub mod json;
pub mod overhead;
pub mod records;
pub mod report;
pub mod scale;
pub mod stats;

pub use records::{load, load_set, parse, read, Run};

/// Errors as their message.
pub type Result<T> = std::result::Result<T, String>;
