//! The `zip_archive` target with bayan-opc's default limits (`Limits::DEFAULT`), which a host gets unless it chooses others, instead of the smaller limits that keep `zip_archive` quick: the same properties, on archives as large as the defaults allow. CI fuzzes it every night with inputs of up to 1 MiB, so that it can reach the sizes that the default limits must hold (see `fuzz/README.md`). Keep it in step with `zip_archive.rs`: only the limits differ.

#![no_main]

use bayan_opc::Limits;
use bayan_opc::zip::{ZipArchive, ZipWriter};
use libfuzzer_sys::fuzz_target;

/// The limits a host gets by default.
const LIMITS: Limits = Limits::DEFAULT;

fuzz_target!(|data: &[u8]| {
    let Ok(archive) = ZipArchive::new(data, &LIMITS) else {
        return;
    };
    let mut writer = ZipWriter::new();
    for (index, entry) in archive.entries().iter().enumerate() {
        let read = archive.read(index);
        let verified = archive.verify(index);
        assert_eq!(read.as_ref().err(), verified.as_ref().err());
        if let Ok(content) = &read {
            assert_eq!(content.len() as u64, entry.uncompressed_size());
            if !entry.is_folder() {
                let raw = archive.raw_entry(index).unwrap();
                writer.add_raw(entry.name(), &raw).unwrap();
            }
        }
    }
    let copy = writer.finish().unwrap();
    // The copy leaves out what the original may have had besides its entries (comments, padding, extra fields), so it can be smaller and more compressed overall; the ratio is not what is checked here.
    let relaxed = Limits {
        max_compression_ratio: u64::MAX,
        ..LIMITS
    };
    let copied = ZipArchive::new(&copy, &relaxed).unwrap();
    for (index, entry) in copied.entries().iter().enumerate() {
        let original = archive
            .entries()
            .iter()
            .position(|original| original.name() == entry.name())
            .unwrap();
        assert_eq!(
            copied.raw_data(index).unwrap(),
            archive.raw_data(original).unwrap()
        );
        assert_eq!(copied.read(index).unwrap(), archive.read(original).unwrap());
    }
});
