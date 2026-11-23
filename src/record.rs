//! The run record and its JSON form.

use crate::{Event, Site};
use std::fmt::{self, Display, Formatter, Write};

/// The cost of an empty scope, timed when the context was created (zero
/// without the feature `on` or without a clock). Nothing is subtracted from
/// the recorded values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Calibration {
    /// Elapsed time an empty scope records as its own, in ns.
    pub inner_ns: f64,
    /// Cost per scope seen by its parent minus `inner_ns`, in ns.
    pub outer_ns: f64,
}

/// A parameter of the run.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    /// A number.
    Num(f64),
    /// A string.
    Str(String),
}

macro_rules! num {
    ($($t:ty)*) => {$(
        impl From<$t> for Param {
            fn from(v: $t) -> Param {
                Param::Num(v as f64)
            }
        }
    )*};
}
num!(i8 i16 i32 i64 isize u8 u16 u32 u64 usize f32 f64);

impl From<&str> for Param {
    fn from(v: &str) -> Param {
        Param::Str(v.to_string())
    }
}

impl From<String> for Param {
    fn from(v: String) -> Param {
        Param::Str(v)
    }
}

/// A node of the tree; its id is its index in [`Record::nodes`].
#[derive(Clone, Debug)]
pub struct Node {
    /// Parent node id; `None` for the root.
    pub parent: Option<u32>,
    /// Scope site id; `None` for the root.
    pub site: Option<u32>,
    /// Summed values per measure name: `calls`, `wall`, `busy` (ns), `bytes`,
    /// `allocs`, `freed` where any node has them, then the user measures
    /// present at this node.
    pub values: Vec<(&'static str, u64)>,
}

impl Node {
    /// The value of a measure, zero where it is absent.
    pub fn get(&self, measure: &str) -> u64 {
        self.values
            .iter()
            .find(|v| v.0 == measure)
            .map_or(0, |v| v.1)
    }
}

/// What [`Context::record`](crate::Context::record) returns; `Display` writes
/// it as one JSON document.
#[derive(Clone, Debug)]
pub struct Record {
    /// Parameters of the run in the order they were first set.
    pub params: Vec<(String, Param)>,
    /// Scope cost timed at context creation.
    pub calibration: Calibration,
    /// Number of threads that entered the context.
    pub threads: u32,
    /// Measures present, with `true` for exact and `false` for noisy.
    pub measures: Vec<(&'static str, bool)>,
    /// Scope sites the nodes refer to, by id.
    pub sites: Vec<(u32, &'static Site)>,
    /// Nodes by id; node 0 is the root.
    pub nodes: Vec<Node>,
    /// Distinct message events at WARN and above (same site and text once),
    /// each with its count, in the order first emitted.
    pub warnings: Vec<(Event, u64)>,
    /// Warnings not kept or counted because the bound was reached.
    pub warnings_dropped: u64,
}

impl Record {
    /// The id of the node reached from the root through scopes of these
    /// names.
    pub fn find(&self, path: &[&str]) -> Option<u32> {
        path.iter().try_fold(0, |at, name| {
            let id = |s| self.sites.iter().find(|x| x.0 == s).map(|x| x.1.name);
            let i = self
                .nodes
                .iter()
                .position(|n| n.parent == Some(at) && n.site.and_then(id) == Some(*name))?;
            Some(i as u32)
        })
    }
}

/// A JSON string literal.
pub(crate) struct Str<'a>(pub(crate) &'a str);

impl Display for Str<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_char('"')?;
        for c in self.0.chars() {
            match c {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
                c => f.write_char(c)?,
            }
        }
        f.write_char('"')
    }
}

/// A JSON number; `null` where not finite. `Display` of `f64` is the shortest
/// form that parses back to the same value.
struct Num(f64);

impl Display for Num {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0.is_finite() {
            true => write!(f, "{}", self.0),
            false => f.write_str("null"),
        }
    }
}

/// `null` or the number.
fn opt(v: Option<u32>) -> String {
    v.map_or("null".into(), |v| v.to_string())
}

/// Writes `items` with `each`, separated by commas.
pub(crate) fn list<W: Write, T>(
    w: &mut W,
    items: impl IntoIterator<Item = T>,
    mut each: impl FnMut(&mut W, T) -> fmt::Result,
) -> fmt::Result {
    for (i, x) in items.into_iter().enumerate() {
        if i > 0 {
            w.write_char(',')?;
        }
        each(w, x)?;
    }
    Ok(())
}

impl Display for Record {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{{\"schema\":{},\"params\":{{", crate::SCHEMA)?;
        list(f, &self.params, |f, (k, v)| match v {
            Param::Num(x) => write!(f, "{}:{}", Str(k), Num(*x)),
            Param::Str(s) => write!(f, "{}:{}", Str(k), Str(s)),
        })?;
        let c = self.calibration;
        write!(
            f,
            "}},\"calibration\":{{\"inner_ns\":{},\"outer_ns\":{}}},\"threads\":{},\"measures\":{{",
            Num(c.inner_ns),
            Num(c.outer_ns),
            self.threads
        )?;
        list(f, &self.measures, |f, &(name, exact)| {
            let kind = if exact { "exact" } else { "noisy" };
            write!(f, "{}:\"{kind}\"", Str(name))
        })?;
        f.write_str("},\"sites\":[")?;
        list(f, &self.sites, |f, (id, s)| {
            write!(
                f,
                "{{\"id\":{id},\"name\":{},\"module\":{},\"file\":{},\"line\":{},\"level\":\"{}\"}}",
                Str(s.name),
                Str(s.module),
                Str(s.file),
                s.line,
                s.level.name()
            )
        })?;
        f.write_str("],\"nodes\":[")?;
        list(f, self.nodes.iter().enumerate(), |f, (i, n)| {
            let (parent, site) = (opt(n.parent), opt(n.site));
            write!(
                f,
                "{{\"id\":{i},\"parent\":{parent},\"site\":{site},\"values\":{{"
            )?;
            list(f, &n.values, |f, (m, v)| write!(f, "{}:{v}", Str(m)))?;
            f.write_str("}}")
        })?;
        f.write_str("],\"warnings\":[")?;
        list(f, &self.warnings, |f, (e, count)| {
            write!(
                f,
                "{{\"node\":{},\"level\":\"{}\",\"module\":{},\"file\":{},\"line\":{},\"time\":{},\"text\":{},\"count\":{count}}}",
                e.node,
                e.level.name(),
                Str(e.module),
                Str(e.file),
                e.line,
                e.time,
                Str(&e.kind.to_string()),
            )
        })?;
        write!(f, "],\"warnings_dropped\":{}}}", self.warnings_dropped)
    }
}
