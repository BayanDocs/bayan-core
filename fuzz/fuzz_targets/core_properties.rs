//! The core properties parser (bayan-opc's `CoreProperties`) on arbitrary bytes. For every part it accepts, writing the properties and reading them back gives the same properties.

#![no_main]

use bayan_opc::{CoreProperties, Limits};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(properties) = CoreProperties::parse(data, &Limits::DEFAULT) else {
        return;
    };
    let again = CoreProperties::parse(&properties.to_xml(), &Limits::DEFAULT).unwrap();
    assert_eq!(again, properties);
});
