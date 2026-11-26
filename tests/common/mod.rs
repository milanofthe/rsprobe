use rsprobe::Record;

pub fn get(r: &Record, path: &[&str], m: &str) -> u64 {
    r.nodes[r.find(path).unwrap() as usize].get(m)
}
