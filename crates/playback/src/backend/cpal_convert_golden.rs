//! The generator and check of `transit-audio-wasapi`'s conversion goldens (ADR 0046, decision 3).
//! The native WASAPI conversion tests compare against a stored table of what this adapter's cpal
//! conversion makes of their sample set, so that crate needs no cpal. This test computes the
//! table from cpal and asserts the stored one equals it, on every target that builds cpal. Set
//! `CONVERT_GOLDEN_OUT=<file>` to write the golden file with fresh tables instead (the part above
//! its marker line is kept), then run `cargo fmt --all` and diff.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use cpal::{I24, Sample as _};

include!("../../../audio-wasapi/src/convert_golden.rs");

const GOLDEN: &str = include_str!("../../../audio-wasapi/src/convert_golden.rs");
const MARKER: &str = "// ---- generated tables below ----";

/// The tables cpal's conversion makes of `input()`: write (`f32` to the device integer; `i32` as its
/// bits) and read (the device integer back to `f32`, as bits).
struct Tables {
    i16_write: Vec<i16>,
    i24_write: Vec<i32>,
    i32_write: Vec<u32>,
    i16_read: Vec<u32>,
    i24_read: Vec<u32>,
    i32_read: Vec<u32>,
}

fn from_cpal() -> Tables {
    let x = input();
    let i16_write: Vec<i16> = x.iter().map(|v| i16::from_sample(*v)).collect();
    let i24_write: Vec<i32> = x.iter().map(|v| I24::from_sample(*v).inner()).collect();
    let i32_write: Vec<i32> = x.iter().map(|v| i32::from_sample(*v)).collect();
    Tables {
        i16_read: i16_write.iter().map(|v| f32::from_sample(*v).to_bits()).collect(),
        i24_read: i24_write.iter().map(|v| f32::from_sample(I24::new(*v).unwrap()).to_bits()).collect(),
        i32_read: i32_write.iter().map(|v| f32::from_sample(*v).to_bits()).collect(),
        i32_write: i32_write.iter().map(|v| u32::from_le_bytes(v.to_le_bytes())).collect(),
        i16_write,
        i24_write,
    }
}

fn table<T>(doc: &str, name: &str, ty: &str, values: &[T], show: impl Fn(&T) -> String) -> String {
    let list: Vec<String> = values.iter().map(show).collect();
    format!("/// {doc}\npub(crate) const {name}: &[{ty}] = &[{}];\n", list.join(", "))
}

/// The golden file with `tables` below the marker.
fn render(tables: &Tables) -> String {
    let (head, _) = GOLDEN.split_once(MARKER).expect("the golden file has its marker line");
    let hex = |v: &u32| format!("{v:#010x}");
    [
        format!("{head}{MARKER}\n"),
        table("`i16::from_sample` of each sample of `input()`.", "I16_WRITE", "i16", &tables.i16_write, i16::to_string),
        table("`I24::from_sample(..).inner()` of each sample of `input()`.", "I24_WRITE", "i32", &tables.i24_write, i32::to_string),
        table(
            "The bits of `i32::from_sample` of each sample of `input()` (hex, so the table stays compact).",
            "I32_WRITE",
            "u32",
            &tables.i32_write,
            hex,
        ),
        table("The bits of `f32::from_sample` of each `I16_WRITE` value.", "I16_READ", "u32", &tables.i16_read, hex),
        table("The bits of `f32::from_sample` of each `I24_WRITE` value (as cpal's `I24`).", "I24_READ", "u32", &tables.i24_read, hex),
        table("The bits of `f32::from_sample` of each `I32_WRITE` value.", "I32_READ", "u32", &tables.i32_read, hex),
    ]
    .concat()
}

#[test]
fn the_wasapi_conversion_goldens_are_cpals_conversion() {
    let tables = from_cpal();
    if let Some(path) = std::env::var_os("CONVERT_GOLDEN_OUT") {
        std::fs::write(path, render(&tables)).unwrap();
        return;
    }
    assert_eq!(tables.i16_write, I16_WRITE, "I16_WRITE");
    assert_eq!(tables.i24_write, I24_WRITE, "I24_WRITE");
    assert_eq!(tables.i32_write, I32_WRITE, "I32_WRITE");
    assert_eq!(tables.i16_read, I16_READ, "I16_READ");
    assert_eq!(tables.i24_read, I24_READ, "I24_READ");
    assert_eq!(tables.i32_read, I32_READ, "I32_READ");
}
