//! The content types stream parser (bayan-opc's `ContentTypes`) and the minimal XML parser under it, on arbitrary bytes. For every stream it accepts: written back unchanged it gives exactly the input; changed, it gives a stream that parses again, with the change in it; and the change undone gives the input again.

#![no_main]

use bayan_opc::{ContentTypes, Limits, PartName};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(mut types) = ContentTypes::parse(data, &Limits::DEFAULT) else {
        return;
    };
    assert_eq!(types.to_xml(), data);
    let part = PartName::new("/bayan/fuzz.bayanfuzz").unwrap();
    let _ = types.content_type(&part);
    let had_default = types
        .defaults()
        .any(|(extension, _)| extension.eq_ignore_ascii_case("bayanfuzz"));
    let had_override = types.overrides().any(|(name, _)| *name == part);
    types
        .set_override(&part, "application/x-bayan-fuzz")
        .unwrap();
    types
        .set_default("bayanfuzz", "application/x-bayan-default")
        .unwrap();
    let changed = ContentTypes::parse(&types.to_xml(), &Limits::DEFAULT).unwrap();
    assert_eq!(
        changed.content_type(&part),
        Some("application/x-bayan-fuzz")
    );
    if !had_default && !had_override {
        assert!(types.remove_override(&part));
        assert!(types.remove_default("bayanfuzz"));
        assert_eq!(types.to_xml(), data);
    }
});
