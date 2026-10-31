//! The workspace builds `zip` with `deflate` and `time` only (no default
//! features). That is all the theme export/import and the support ZIP need:
//! the reference writes Deflate and Stored entries. These tests pin what the
//! trimmed build keeps and what it gives up.

use std::io::{Cursor, Read, Write};

fn write_zip(method: zip::CompressionMethod) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(method);
    zw.start_file("a.txt", opts).unwrap();
    zw.write_all(&b"hello hello hello hello".repeat(40))
        .unwrap();
    zw.finish().unwrap().into_inner()
}

fn read_first(bytes: Vec<u8>) -> Result<Vec<u8>, zip::result::ZipError> {
    let mut za = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut entry = za.by_index(0)?;
    let mut body = Vec::new();
    entry.read_to_end(&mut body)?;
    Ok(body)
}

#[test]
fn deflated_and_stored_entries_round_trip() {
    for method in [
        zip::CompressionMethod::Deflated,
        zip::CompressionMethod::Stored,
    ] {
        let body = read_first(write_zip(method)).unwrap();
        assert_eq!(body, b"hello hello hello hello".repeat(40), "{method:?}");
    }
}

#[test]
fn deflate_actually_compresses() {
    let deflated = write_zip(zip::CompressionMethod::Deflated);
    let stored = write_zip(zip::CompressionMethod::Stored);
    assert!(
        deflated.len() < stored.len() / 2,
        "{} vs {} bytes",
        deflated.len(),
        stored.len()
    );
}

/// Without zip's `time` feature every new entry is stamped 1980-01-01, which
/// the support ZIP would show for every file in it.
#[test]
fn new_entries_carry_the_current_time() {
    let mut za =
        zip::ZipArchive::new(Cursor::new(write_zip(zip::CompressionMethod::Deflated))).unwrap();
    let entry = za.by_index(0).unwrap();
    let year = entry.last_modified().expect("a timestamp").year();
    assert!(year >= 2025, "entry stamped {year}");
}

/// An archive written by `zipfile.ZIP_BZIP2` (Python), one `a.txt`
/// holding `hello hello hello hello`. zip's default features would decode it;
/// the trimmed build refuses it with an error (the importer reports it)
/// instead of reading it.
#[test]
fn a_bzip2_entry_is_an_error() {
    const ZIP_HEX: &str = "504b03042e0000000c0000605f5de3513d8d2e0000001700000005000000612e747874425a683931415926535902f8b0bd000003910040000244a00030cd00548696719b38a3c5dc914e142400be2c2f40504b01022e032e0000000c0000605f5de3513d8d2e00000017000000050000000000000000000000800100000000612e747874504b0506000000000100010033000000510000000000";
    let bytes: Vec<u8> = (0..ZIP_HEX.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&ZIP_HEX[i..i + 2], 16).unwrap())
        .collect();
    let err = read_first(bytes).expect_err("bzip2 is not compiled in");
    assert!(
        matches!(
            err,
            zip::result::ZipError::CompressionMethodNotSupported(12)
        ),
        "{err:?}"
    );
}
