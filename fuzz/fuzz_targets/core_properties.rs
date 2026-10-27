//! The core properties parser (bayan-opc's `CoreProperties`) on arbitrary bytes. For every part it accepts, the properties can be written (the parser accepts no character that XML cannot hold), and writing them and reading them back gives the same properties.

#![no_main]

use bayan_opc::{CoreProperties, Limits};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // A deliberate panic for bayan-core#21's AC-1: it shows that a crash fails the fuzzing job. Never merge this.
    assert_ne!(data.first(), Some(&b'#'), "deliberate panic for bayan-core#21 (AC-1)");
    let Ok(properties) = CoreProperties::parse(data, &Limits::DEFAULT) else {
        return;
    };
    let again = CoreProperties::parse(&properties.to_xml().unwrap(), &Limits::DEFAULT).unwrap();
    assert_eq!(again, properties);
});
