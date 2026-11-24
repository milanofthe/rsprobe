//! Lists the imports of a wasm module and fails if there are any.
//!
//! A plain wasm32-unknown-unknown module gets no host functions in a browser
//! unless a binding layer supplies them, so an import there is a function the
//! crate cannot run without (a clock, a random source).
//!
//!     cargo run -p rsprobe-tool --example wasm_imports -- MODULE.wasm

/// The LEB128 number at `b[*i]`, the cursor moved past it.
fn leb(b: &[u8], i: &mut usize) -> u64 {
    let (mut v, mut s) = (0, 0);
    loop {
        let x = b[*i];
        *i += 1;
        v |= ((x & 0x7f) as u64) << s;
        s += 7;
        if x < 0x80 {
            return v;
        }
    }
}

fn name(b: &[u8], i: &mut usize) -> String {
    let n = leb(b, i) as usize;
    *i += n;
    String::from_utf8_lossy(&b[*i - n..*i]).into()
}

/// `module.field` of each import of the module.
fn imports(b: &[u8]) -> Vec<String> {
    assert_eq!(&b[..4], b"\0asm", "not a wasm module");
    let (mut i, mut out) = (8, Vec::new());
    while i < b.len() {
        let id = b[i];
        i += 1;
        let end = leb(b, &mut i) as usize + i;
        if id == 2 {
            for _ in 0..leb(b, &mut i) {
                let (module, field) = (name(b, &mut i), name(b, &mut i));
                let kind = b[i];
                i += 1;
                match kind {
                    // function: type index; global: type, mutability
                    0 => _ = leb(b, &mut i),
                    3 => i += 2,
                    // table: element type then limits; memory: limits
                    _ => {
                        i += (kind == 1) as usize;
                        let flags = leb(b, &mut i);
                        leb(b, &mut i);
                        if flags & 1 == 1 {
                            leb(b, &mut i);
                        }
                    }
                }
                out.push(format!("{module}.{field}"));
            }
        }
        i = end;
    }
    out
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: wasm_imports MODULE.wasm");
    let found = imports(&std::fs::read(&path).expect("reading the module"));
    for f in &found {
        println!("import: {f}");
    }
    std::process::exit(!found.is_empty() as i32)
}
