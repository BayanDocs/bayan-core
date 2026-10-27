//! The `package` target with bayan-opc's default limits (`Limits::DEFAULT`), which a host gets unless it chooses others, instead of the smaller limits that keep `package` quick: the same properties, on packages as large as the defaults allow. CI fuzzes it every night with inputs of up to 1 MiB, so that it can build the shapes that the default limits must hold in memory (`crates/bayan-opc/src/lib.rs`, "Limits"; see `fuzz/README.md`). Keep it in step with `package.rs`: only the limits differ.

#![no_main]

use bayan_opc::{Limits, Package, PackageWriter, RelationshipSource};
use libfuzzer_sys::fuzz_target;

/// The limits a host gets by default.
const LIMITS: Limits = Limits::DEFAULT;

fuzz_target!(|data: &[u8]| {
    let Ok(package) = Package::open(data, &LIMITS) else {
        return;
    };
    let mut readable = true;
    for part in package.parts() {
        let _ = package.content_type(part);
        readable &= package.read_part(part).is_ok();
        let source = RelationshipSource::Part(part.clone());
        for relationships in [
            package.relationships(&source),
            package.relationships(&RelationshipSource::Package),
        ]
        .into_iter()
        .flatten()
        {
            for relationship in relationships.iter() {
                let _ = relationship.resolve(&source);
            }
        }
    }
    let _ = package.core_properties();
    // Writing verifies every part before copying it, so it fails exactly when a part cannot be read.
    let written = PackageWriter::from_package(&package).finish();
    assert_eq!(written.is_ok(), readable);
    let Ok(written) = written else {
        return;
    };
    // The copy leaves out entries that are not parts, comments and padding, so it can be smaller and more compressed overall; the ratio is not what is checked here.
    let relaxed = Limits {
        max_compression_ratio: u64::MAX,
        ..LIMITS
    };
    let reopened = Package::open(&written, &relaxed).unwrap();
    let before: Vec<_> = package.parts().collect();
    let after: Vec<_> = reopened.parts().collect();
    assert_eq!(before, after);
    for part in package.parts() {
        let original = package
            .zip()
            .raw_data(package.entry_of(part).unwrap())
            .unwrap();
        let copy = reopened
            .zip()
            .raw_data(reopened.entry_of(part).unwrap())
            .unwrap();
        assert_eq!(original, copy);
    }
});
