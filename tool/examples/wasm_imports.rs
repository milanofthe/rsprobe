//! Lists the imports of a wasm module and fails if there are any.
//!
//! A plain wasm32-unknown-unknown module gets no host functions in a browser
//! unless a binding layer supplies them, so an import there is a function the
//! crate cannot run without (a clock, a random source).
//!
//!     cargo run -p rsprobe-tool --example wasm_imports -- MODULE.wasm

// https://webassembly.github.io/spec/core/binary/modules.html#import-section
const IMPORT_SECTION: u8 = 2;
const IMPORT_FUNCTION: u8 = 0;
const IMPORT_TABLE: u8 = 1;
const IMPORT_GLOBAL: u8 = 3;
const GLOBAL_TYPE_AND_MUTABILITY_BYTES: usize = 2;

fn read_leb128(b: &[u8], i: &mut usize) -> u64 {
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
    let n = read_leb128(b, i) as usize;
    *i += n;
    String::from_utf8_lossy(&b[*i - n..*i]).into()
}

fn imports(b: &[u8]) -> Vec<String> {
    assert_eq!(&b[..4], b"\0asm", "not a wasm module");
    let (mut i, mut out) = (8, Vec::new());
    while i < b.len() {
        let id = b[i];
        i += 1;
        let end = read_leb128(b, &mut i) as usize + i;
        if id == IMPORT_SECTION {
            for _ in 0..read_leb128(b, &mut i) {
                let (module, field) = (name(b, &mut i), name(b, &mut i));
                let kind = b[i];
                i += 1;
                match kind {
                    IMPORT_FUNCTION => _ = read_leb128(b, &mut i),
                    IMPORT_GLOBAL => i += GLOBAL_TYPE_AND_MUTABILITY_BYTES,
                    _ => {
                        let table_element_type_bytes = (kind == IMPORT_TABLE) as usize;
                        i += table_element_type_bytes;
                        let flags = read_leb128(b, &mut i);
                        read_leb128(b, &mut i);
                        if flags & 1 == 1 {
                            read_leb128(b, &mut i);
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
    let path = std::env::args()
        .nth(1)
        .expect("usage: wasm_imports MODULE.wasm");
    let found = imports(&std::fs::read(&path).expect("reading the module"));
    for f in &found {
        println!("import: {f}");
    }
    std::process::exit(!found.is_empty() as i32)
}
