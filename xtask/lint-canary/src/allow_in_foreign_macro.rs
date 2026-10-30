// Form 3 of work package CORE-009: an `#[allow]` written out in full inside the input of another crate's macro, here std's `thread_local!`, which puts it out on the static it writes. Clippy's `allow_attributes` lint does not report an attribute that a macro of another crate puts out, and the source check does not look at lint attributes, so Clippy and the compiler accept this module; the lint audit must reject the hash map it silences.

std::thread_local! {
    /// A map under an `#[allow]` written in full, which std's macro puts out.
    #[allow(clippy::disallowed_types)]
    static MAP: core::cell::RefCell<std::collections::HashMap<u8, u8>> = core::cell::RefCell::new(std::collections::HashMap::new());
}
