//! Records built by hand for the tests.
#![allow(dead_code)]

use rsprobe::{Calibration, Level, Node, Param, Record, Site};
use rsprobe_tool::Run;

/// Nodes as (path, values), the root (the empty path) included.
pub type Tree<'a> = &'a [Entry<'a>];

/// A node: its path and its values.
pub type Entry<'a> = (&'a [&'static str], &'a [(&'static str, u64)]);

/// A record of the tree: wall, busy, pool and the measures in `noisy` are noisy
/// (as the crate declares those added by `time!`), those in `maxima` max
/// (`max!`), every other one exact (`add!`). Each node has calls, wall and
/// busy, zero where not given; sites by name in module "m".
#[derive(Default)]
pub struct Hand {
    pub inner: f64,
    pub outer: f64,
    pub params: Vec<(&'static str, Param)>,
    pub noisy: Vec<&'static str>,
    pub maxima: Vec<&'static str>,
}

impl Hand {
    pub fn record(&self, tree: Tree) -> Record {
        let mut paths: Vec<_> = tree.iter().collect();
        paths.sort_by_key(|n| n.0.len());
        let mut names: Vec<&'static str> =
            paths.iter().filter_map(|n| n.0.last().copied()).collect();
        names.sort();
        names.dedup();
        let site = |n: &str| names.iter().position(|x| *x == n).unwrap() as u32;
        let id = |p: &[&str]| paths.iter().position(|n| n.0 == p).unwrap() as u32;
        let mut measures: Vec<(&'static str, bool)> = vec![
            ("calls", true),
            ("wall", false),
            ("busy", false),
            ("pool", false),
        ];
        measures.extend(self.noisy.iter().map(|&m| (m, false)));
        measures.extend(self.maxima.iter().map(|&m| (m, true)));
        for (_, vs) in tree {
            for &(m, _) in *vs {
                if !measures.iter().any(|x| x.0 == m) {
                    measures.push((m, true));
                }
            }
        }
        let nodes = paths.iter().map(|(p, vs)| {
            let mut values = vec![("calls", 0), ("wall", 0), ("busy", 0)];
            for &(m, x) in *vs {
                match values.iter_mut().find(|v| v.0 == m) {
                    Some(v) => v.1 = x,
                    None => values.push((m, x)),
                }
            }
            let parent = p.split_last().map(|(_, up)| id(up));
            Node {
                parent,
                site: p.last().map(|n| site(n)),
                values,
            }
        });
        let sites = names.iter().map(|&n| {
            let s: &'static Site = Box::leak(Box::new(Site::new(n, "m", "f", 1, Level::INFO)));
            (site(n), s)
        });
        Record {
            params: self
                .params
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            calibration: Calibration {
                inner_ns: self.inner,
                outer_ns: self.outer,
            },
            threads: 1,
            measures,
            max: self.maxima.clone(),
            per_thread: Vec::new(),
            sites: sites.collect(),
            nodes: nodes.collect(),
            warnings: Vec::new(),
            warnings_dropped: 0,
        }
    }

    pub fn run(&self, file: &str, tree: Tree) -> Run {
        Run::new(file, self.record(tree))
    }
}

/// The record of the tree with the defaults of [`Hand`].
pub fn run(tree: Tree) -> Run {
    Hand::default().run("r.json", tree)
}

/// Normal deviates by Box-Muller from the tool's generator.
pub struct Gauss(pub rsprobe_tool::stats::Rng);

impl Gauss {
    pub fn new(seed: u64) -> Gauss {
        Gauss(rsprobe_tool::stats::Rng::new(seed))
    }

    /// A draw of mean `mu` and deviation `sigma`.
    pub fn draw(&mut self, mu: f64, sigma: f64) -> f64 {
        let (u, v) = (1.0 - self.0.uniform(), self.0.uniform());
        mu + sigma * (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

pub fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}
