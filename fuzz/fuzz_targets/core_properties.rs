//! The core properties parser (bayan-opc's `CoreProperties`) on arbitrary bytes. For every part it accepts, the properties can be written (the parser accepts no character that XML cannot hold), and writing them and reading them back gives the same properties.

#![no_main]

use bayan_opc::{CoreProperties, Limits};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // A deliberate panic on a made-up input for FU-6's test run, never to be merged: it shows that a crash fails the fuzzing job, which names only the target and the kind of failure and publishes neither the input nor the log. The message quotes the start of the input, as a property check's message can.
    assert_ne!(
        data.first(),
        Some(&b'#'),
        "deliberate panic for FU-6's test run, on an input that starts with {:?}",
        &data[..data.len().min(16)]
    );
    let Ok(properties) = CoreProperties::parse(data, &Limits::DEFAULT) else {
        return;
    };
    let again = CoreProperties::parse(&properties.to_xml().unwrap(), &Limits::DEFAULT).unwrap();
    assert_eq!(again, properties);
});
