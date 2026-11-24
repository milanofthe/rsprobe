//! Reading run records into [`Record`]s, and the tree of a record by path.

use crate::constants::SCHEMAS;
use crate::json::Json;
use crate::Result;
use rsprobe::{Calibration, Event, Kind, Level, Node, Param, Record, Site, ThreadNodes};
use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;
use std::sync::Mutex;

/// A scope in a path: its name and the module of its call site. Sibling
/// scopes of the same module and name from different call sites are one node.
pub type Scope = (&'static str, &'static str);

/// The scopes from the root, empty for the root.
pub type Path = Vec<Scope>;

/// Names of measures.
pub type Set = BTreeSet<&'static str>;

/// A map that keeps the order of first insertion: keys, and values by key index.
#[derive(Clone, Debug)]
pub struct Map<K, V> {
    pub keys: Vec<K>,
    pub vals: Vec<V>,
    at: HashMap<K, usize>,
}

impl<K, V> Default for Map<K, V> {
    fn default() -> Self {
        Map {
            keys: Vec::new(),
            vals: Vec::new(),
            at: HashMap::new(),
        }
    }
}

impl<K: Hash + Eq + Clone, V> Map<K, V> {
    /// The index of a key.
    pub fn index(&self, k: &K) -> Option<usize> {
        self.at.get(k).copied()
    }

    /// The value of a key.
    pub fn get(&self, k: &K) -> Option<&V> {
        self.index(k).map(|i| &self.vals[i])
    }

    /// The value of a key, inserted with `new` where absent.
    pub fn entry(&mut self, k: K, new: impl FnOnce() -> V) -> &mut V {
        let i = *self.at.entry(k.clone()).or_insert(self.keys.len());
        if i == self.keys.len() {
            self.keys.push(k);
            self.vals.push(new());
        }
        &mut self.vals[i]
    }

    /// Keys and values in order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.keys.iter().zip(&self.vals)
    }
}

/// A node's values by measure, in order of first appearance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Values(pub Vec<(&'static str, f64)>);

impl Values {
    /// The value of a measure, zero where absent.
    pub fn get(&self, m: &str) -> f64 {
        self.0.iter().find(|v| v.0 == m).map_or(0.0, |v| v.1)
    }

    /// Whether the measure is present.
    pub fn has(&self, m: &str) -> bool {
        self.0.iter().any(|v| v.0 == m)
    }

    /// Sets a measure, appended where absent.
    pub fn set(&mut self, m: &'static str, x: f64) {
        match self.0.iter_mut().find(|v| v.0 == m) {
            Some(v) => v.1 = x,
            None => self.0.push((m, x)),
        }
    }

    /// Adds `other`, the measures in `max` by maximum and the others summed,
    /// as the crate merges threads.
    pub fn merge(&mut self, other: &Values, max: &Set) {
        for &(m, x) in &other.0 {
            let y = self.get(m);
            self.set(m, if max.contains(m) { y.max(x) } else { y + x });
        }
    }
}

/// Values by path.
pub type Nodes = Map<Path, Values>;

/// Trees merged by path, the measures in `max` by maximum.
pub fn total<'a>(trees: impl IntoIterator<Item = &'a Nodes>, max: &Set) -> Nodes {
    let mut out = Nodes::default();
    for t in trees {
        for (p, v) in t.iter() {
            out.entry(p.clone(), Values::default).merge(v, max);
        }
    }
    out
}

/// Per node the measures whose values differ over the records, (min, max).
pub type Spread = Vec<(&'static str, (f64, f64))>;

/// Per node and measure over the trees the median (zero where a tree lacks
/// the node), the maximum for the measures in `max`; and per node the
/// spread of the others where the trees differ. Nodes in preorder.
pub fn median_nodes<'a>(
    trees: impl Iterator<Item = &'a Nodes> + Clone,
    max: &Set,
) -> (Nodes, Map<Path, Spread>) {
    let (mut nodes, mut spread) = (Nodes::default(), Map::default());
    for p in preorder(trees.clone().flat_map(|n| &n.keys)) {
        let here: Vec<Values> = trees
            .clone()
            .map(|n| n.get(&p).cloned().unwrap_or_default())
            .collect();
        let (mut v, mut s) = (Values::default(), Spread::new());
        for &(m, _) in here.iter().flat_map(|v| &v.0) {
            if !v.has(m) {
                let xs: Vec<f64> = here.iter().map(|v| v.get(m)).collect();
                let (lo, hi) = xs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, &x| {
                    (a.0.min(x), a.1.max(x))
                });
                v.set(
                    m,
                    if max.contains(m) {
                        hi
                    } else {
                        crate::stats::median(&xs)
                    },
                );
                if !max.contains(m) && lo < hi {
                    s.push((m, (lo, hi)));
                }
            }
        }
        *nodes.entry(p.clone(), Values::default) = v;
        *spread.entry(p, Spread::new) = s;
    }
    (nodes, spread)
}

/// The distinct paths, parents first, siblings in order of first appearance.
pub fn preorder<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<Path> {
    let mut first: Map<Path, ()> = Map::default();
    for p in paths {
        first.entry(p.clone(), || ());
    }
    let mut out = first.keys.clone();
    out.sort_by_cached_key(|p| {
        let k = (0..=p.len()).map(|k| first.index(&p[..k].to_vec()).unwrap_or(usize::MAX));
        k.collect::<Vec<_>>()
    });
    out
}

/// Per node of a tree the indices of its children, in order.
pub fn children(nodes: &Nodes) -> Vec<Vec<usize>> {
    let mut kids = vec![Vec::new(); nodes.keys.len()];
    for (i, p) in nodes.keys.iter().enumerate() {
        if let Some(up) = p.split_last().and_then(|(_, up)| nodes.index(&up.to_vec())) {
            kids[up].push(i);
        }
    }
    kids
}

/// A record as the commands read it: its source (a file name or a label in
/// messages), the record and its nodes merged by path.
#[derive(Clone, Debug)]
pub struct Run {
    pub file: String,
    pub record: Record,
    pub nodes: Nodes,
    paths: Vec<Path>,
}

impl Run {
    /// The record `record` named `file`, a path or any label.
    pub fn new(file: impl Into<String>, record: Record) -> Run {
        let mut memo = vec![None; record.nodes.len()];
        let paths = (0..record.nodes.len())
            .map(|i| path(&record, i, &mut memo))
            .collect();
        let mut run = Run {
            file: file.into(),
            record,
            nodes: Nodes::default(),
            paths,
        };
        run.nodes = run.tree(run.record.nodes.iter().enumerate());
        run
    }

    /// The measures of a kind: `exact`, `noisy` or `max`.
    pub fn kind(&self, kind: &str) -> Set {
        let of = |&(m, exact): &(&'static str, bool)| match kind {
            "max" => self.record.max.contains(&m),
            "exact" => exact && !self.record.max.contains(&m),
            _ => !exact,
        };
        self.record
            .measures
            .iter()
            .filter(|m| of(m))
            .map(|m| m.0)
            .collect()
    }

    /// The calibration constants `inner` and `outer` in ns, zero where not
    /// finite.
    pub fn calibration(&self) -> (f64, f64) {
        let Calibration { inner_ns, outer_ns } = self.record.calibration;
        let f = |x: f64| if x.is_finite() { x } else { 0.0 };
        (f(inner_ns), f(outer_ns))
    }

    /// Nodes by id merged by path.
    fn tree<'a>(&self, nodes: impl Iterator<Item = (usize, &'a Node)>) -> Nodes {
        let max = self.kind("max");
        let mut out = Nodes::default();
        for (i, n) in nodes {
            let v = Values(n.values.iter().map(|&(m, x)| (m, x as f64)).collect());
            out.entry(self.paths[i].clone(), Values::default)
                .merge(&v, &max);
        }
        out
    }

    /// Per thread of a record taken with `record_threads` its values by
    /// path, the nodes it touched.
    pub fn per_thread(&self) -> Vec<(&ThreadNodes, Nodes)> {
        let used = |n: &(usize, &Node)| n.1.values.iter().any(|v| v.1 > 0);
        let t = self.record.per_thread.iter();
        t.map(|t| (t, self.tree(t.nodes.iter().enumerate().filter(used))))
            .collect()
    }

    /// The warnings as (path, level, text, count).
    pub fn warnings(&self) -> Vec<(Path, &'static str, String, u64)> {
        let w = self.record.warnings.iter();
        let at = |n: u32| self.paths.get(n as usize).cloned().unwrap_or_default();
        w.map(|(e, c)| (at(e.node), e.level.name(), e.kind.to_string(), *c))
            .collect()
    }
}

fn path(r: &Record, i: usize, memo: &mut Vec<Option<Path>>) -> Path {
    if let Some(p) = &memo[i] {
        return p.clone();
    }
    let n = &r.nodes[i];
    let p = match n.parent {
        None => Path::new(),
        Some(up) => {
            let mut p = path(r, up as usize, memo);
            let site = r.sites.iter().find(|s| Some(s.0) == n.site);
            p.push(site.map_or(("?", ""), |s| (s.1.name, s.1.module)));
            p
        }
    };
    memo[i] = Some(p.clone());
    p
}

/// The union over runs of a set of each.
pub fn union<'a>(runs: impl IntoIterator<Item = &'a Run>, f: impl Fn(&Run) -> Set) -> Set {
    runs.into_iter().flat_map(f).collect()
}

/// A string that lives as long as the process, once per distinct text.
fn intern(s: &str) -> &'static str {
    static SET: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut set = SET.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&s) = set.get(s) {
        return s;
    }
    let s: &'static str = Box::leak(s.into());
    set.insert(s);
    s
}

/// The member `k` of `v` read by `f`.
pub fn get<'a, T>(v: &'a Json, k: &str, f: impl FnOnce(&'a Json) -> Option<T>) -> Result<T> {
    v.get(k)
        .and_then(f)
        .ok_or_else(|| format!("no valid field '{k}'"))
}

fn string(v: &Json, k: &str) -> Result<&'static str> {
    get(v, k, Json::str).map(intern)
}

fn id(v: &Json, k: &str) -> Result<Option<u32>> {
    get(v, k, |x| {
        if *x == Json::Null {
            Some(None)
        } else {
            x.u64().map(|x| Some(x as u32))
        }
    })
}

fn level(v: &Json) -> Result<Level> {
    let name = string(v, "level")?;
    let all = [
        Level::ERROR,
        Level::WARN,
        Level::INFO,
        Level::DEBUG,
        Level::TRACE,
    ];
    all.into_iter()
        .find(|l| l.name() == name)
        .ok_or_else(|| format!("level {name}"))
}

fn values(v: &Json) -> Result<Vec<(&'static str, u64)>> {
    let m = get(v, "values", Json::members)?.iter();
    m.map(|(m, x)| {
        x.u64()
            .map(|x| (intern(m), x))
            .ok_or_else(|| format!("value of {m}"))
    })
    .collect()
}

/// A record from its JSON (schema 1 or 2). Names are interned and sites
/// allocated for the life of the process, as the record's types hold them.
pub fn parse(text: &str) -> Result<Record> {
    let doc = Json::parse(text)?;
    let schema = doc.get("schema").and_then(Json::u64);
    if !schema.is_some_and(|s| SCHEMAS.contains(&s)) {
        let s = schema.map_or("None".into(), |s| s.to_string());
        return Err(format!("record schema {s}, this tool reads (1, 2)"));
    }
    let int = |v: &Json, k: &str| get(v, k, Json::u64);
    let params = get(&doc, "params", Json::members)?.iter().map(|(k, v)| {
        let x = match v {
            Json::Str(s) => Param::Str(s.clone()),
            v => Param::Num(if *v == Json::Null {
                f64::NAN
            } else {
                v.f64().ok_or("a parameter")?
            }),
        };
        Ok((k.clone(), x))
    });
    let c = get(&doc, "calibration", Some)?;
    let ns = |k| get(c, k, |v| Some(v.f64().unwrap_or(0.0)));
    let (mut measures, mut max) = (Vec::new(), Vec::new());
    for (m, kind) in get(&doc, "measures", Json::members)? {
        let m = intern(m);
        match kind.str() {
            Some("max") => max.push(m),
            Some("exact" | "noisy") => {}
            _ => return Err(format!("kind of measure {m}")),
        }
        measures.push((m, kind.str() != Some("noisy")));
    }
    let sites = get(&doc, "sites", Json::arr)?.iter().map(|v| {
        let file = v.get("file").map_or(Ok(""), |_| string(v, "file"))?;
        let line = v.get("line").and_then(Json::u64).unwrap_or(0) as u32;
        let site = Site::new(
            string(v, "name")?,
            string(v, "module")?,
            file,
            line,
            level(v)?,
        );
        Ok((int(v, "id")? as u32, &*Box::leak(Box::new(site))))
    });
    let sites: Vec<(u32, &'static Site)> = sites.collect::<Result<_>>()?;
    let mut nodes = Vec::new();
    for (i, v) in get(&doc, "nodes", Json::arr)?.iter().enumerate() {
        let (parent, site) = (id(v, "parent")?, id(v, "site")?);
        if int(v, "id")? != i as u64 || !site.is_none_or(|s| sites.iter().any(|x| x.0 == s)) {
            return Err(format!(
                "node {i}: ids not 0..n in order, or an unknown site"
            ));
        }
        nodes.push(Node {
            parent,
            site,
            values: values(v)?,
        });
    }
    if nodes.is_empty()
        || nodes
            .iter()
            .any(|n| n.parent.is_some_and(|p| p as usize >= nodes.len()))
    {
        return Err("no root, or a node with an unknown parent".into());
    }
    let list = |k| doc.get(k).map_or(Ok(&[][..]), |_| get(&doc, k, Json::arr));
    let per_thread = list("per_thread")?.iter().map(|t| {
        let mut ns: Vec<Node> = nodes
            .iter()
            .map(|n| Node {
                values: Vec::new(),
                ..n.clone()
            })
            .collect();
        for n in get(t, "nodes", Json::arr)? {
            ns.get_mut(int(n, "id")? as usize)
                .ok_or("a thread's node id")?
                .values = values(n)?;
        }
        let handle = get(t, "handle", Some)? == &Json::Bool(true);
        Ok(ThreadNodes {
            index: int(t, "index")? as u32,
            handle,
            nodes: ns,
        })
    });
    let warnings = list("warnings")?.iter().map(|w| {
        let node = Some(int(w, "node")? as u32)
            .filter(|&n| (n as usize) < nodes.len())
            .ok_or("a warning's node")?;
        let (module, file, line, time) = (
            string(w, "module")?,
            string(w, "file")?,
            int(w, "line")? as u32,
            int(w, "time")?,
        );
        let kind = Kind::Message(get(w, "text", Json::str)?.into());
        Ok((
            Event {
                node,
                level: level(w)?,
                module,
                file,
                line,
                time,
                kind,
            },
            int(w, "count")?,
        ))
    });
    Ok(Record {
        params: params.collect::<Result<_>>()?,
        calibration: Calibration {
            inner_ns: ns("inner_ns")?,
            outer_ns: ns("outer_ns")?,
        },
        threads: int(&doc, "threads")? as u32,
        measures,
        max,
        per_thread: per_thread.collect::<Result<_>>()?,
        sites,
        warnings: warnings.collect::<Result<_>>()?,
        warnings_dropped: doc.get("warnings_dropped").and_then(Json::u64).unwrap_or(0),
        nodes,
    })
}

/// A record file.
pub fn load(path: impl AsRef<std::path::Path>) -> Result<Record> {
    let p = path.as_ref();
    let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", p.display()))
}

/// The records of a set: each argument a record file or a directory whose
/// `*.json` files are taken in name order.
pub fn load_set<A: AsRef<std::path::Path>>(args: &[A]) -> Result<Vec<Run>> {
    let mut files = Vec::new();
    for a in args.iter().map(AsRef::as_ref) {
        if !a.is_dir() {
            files.push(a.to_path_buf());
            continue;
        }
        let dir = std::fs::read_dir(a).map_err(|e| format!("{}: {e}", a.display()))?;
        let mut names: Vec<String> = dir
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into())
            .collect();
        names.retain(|n| n.ends_with(".json"));
        names.sort();
        files.extend(names.iter().map(|n| a.join(n)));
    }
    if files.is_empty() {
        let args: Vec<_> = args
            .iter()
            .map(|a| a.as_ref().display().to_string())
            .collect();
        return Err(format!("no records in {}", args.join(" ")));
    }
    let run = |f: &std::path::PathBuf| Ok(Run::new(f.display().to_string(), load(f)?));
    files.iter().map(run).collect()
}
