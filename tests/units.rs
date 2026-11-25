//! The numbers for a reader: the rounding rule of `rsprobe::units` at the
//! unit boundaries, by a table of cases.

use rsprobe::units::{bytes, count, share, time};

#[test]
fn times() {
    let cases: [(f64, &str); 22] = [
        (0.0, "0 ns"),
        (0.8026, "0.80 ns"),
        (14.0, "14.0 ns"),
        (999.4, "999 ns"),
        (999.5, "1.00 us"),
        (1747.0, "1.75 us"),
        (5000.0, "5.00 us"),
        (971e3, "971 us"),
        (999.4e3, "999 us"),
        (999.5e3, "1.00 ms"),
        (4.189e6, "4.19 ms"),
        (3.218e6, "3.22 ms"),
        (3.867534e9, "3.87 s"),
        (12.3e9, "12.3 s"),
        (59.94e9, "59.9 s"),
        (59.95e9, "1:00.0"),
        (60.291186e9, "1:00.3"),
        (754.0e9, "12:34.0"),
        (3599.94e9, "59:59.9"),
        (3599.96e9, "1:00:00"),
        (36000e9, "10:00:00"),
        (-1500.0, "-1.50 us"),
    ];
    for (ns, text) in cases {
        assert_eq!(time(ns).to_string(), text, "{ns} ns");
    }
}

#[test]
fn counts() {
    let cases: [(f64, &str); 14] = [
        (0.0, "0"),
        (5.0, "5"),
        (2000.0, "2,000"),
        (230_604.0, "230,604"),
        (999_999.0, "999,999"),
        (999_999.5, "1.00 M"),
        (1e6, "1.00 M"),
        (4e6, "4.00 M"),
        (217.3e6, "217 M"),
        (999.5e6, "1.00 G"),
        (3.5, "3.50"),
        (0.25, "0.25"),
        (1234.5, "1,235"),
        (u64::MAX as f64, "18.4 E"),
    ];
    for (n, text) in cases {
        assert_eq!(count(n).to_string(), text, "{n}");
    }
}

#[test]
fn sizes_and_shares() {
    let cases: [(f64, &str); 6] = [
        (0.0, "0 B"),
        (999.0, "999 B"),
        (1000.0, "1.00 kB"),
        (999.5e3, "1.00 MB"),
        (4.1e6, "4.10 MB"),
        (2.5e9, "2.50 GB"),
    ];
    for (n, text) in cases {
        assert_eq!(bytes(n).to_string(), text, "{n} B");
    }
    let cases: [(f64, &str); 10] = [
        (0.232, "23.2 %"),
        (0.766, "76.6 %"),
        (0.9326, "93.3 %"),
        (1.0, "100.0 %"),
        (0.01, "1.0 %"),
        (0.0099, "0.99 %"),
        (0.0071, "0.71 %"),
        (0.0003, "0.03 %"),
        (0.0, "0 %"),
        (-0.0071, "-0.71 %"),
    ];
    for (x, text) in cases {
        assert_eq!(share(x).to_string(), text, "{x}");
    }
}

#[test]
fn width_and_alignment() {
    assert_eq!(format!("{:>9}", time(13880.0)), "  13.9 us");
    assert_eq!(format!("{:<8}|", count(2000.0)), "2,000   |");
    assert_eq!(format!("{:>8}", share(0.001)), "   0.1 %");
}
