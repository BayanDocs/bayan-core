//! What the engine's WebAssembly module asks of its host.
//!
//! In the browser the engine is a WebAssembly module inside a Web Worker. A module can reach nothing outside its own memory except through its imports: the functions, tables, memories and globals that the JavaScript code instantiating it must hand it, which wasm-bindgen's glue provides. Rust's standard library has no clock, files, environment variables or sockets on wasm32-unknown-unknown, so every way out of the module is an import, typically one that a dependency declares through wasm-bindgen, js-sys or web-sys (a clock, the browser's randomness), and the engine must take none of them on its own (ADR-0012 §5).
//!
//! `cargo xtask wasm-package` therefore reads the import section of the module it packages (`bayan_wasm_bg.wasm`, after wasm-bindgen) and fails on any import that `WASM_IMPORTS` in `artifacts.rs` does not list with the reason it is needed, and on any listed import that the module no longer has, so that the list cannot go stale. Reviewers read every change to the list. ADR-0012's amendment of 2026-10-08 planned such a check for the work package that first links bayan-crdt into the engine, where Loro's randomness and clock will appear among the imports (CORE-004 report §8.4); the review of 2026-10-27 (finding M5) moved it forward to now, when the module imports wasm-bindgen's glue only.
//!
//! The reader is strict, so that nothing can hide behind a misreading: it fails on a file that is not a WebAssembly module of version 1, on a section or an import it cannot read to its end, on a second import section, and on a kind of import it does not know.

/// What an import is: a function, or a table, memory, global or tag (an exception's type) that the host shares with the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A function that the module calls.
    Function,
    /// A table of references, such as functions or JavaScript values.
    Table,
    /// A memory.
    Memory,
    /// A global variable.
    Global,
    /// An exception's type.
    Tag,
}

impl Kind {
    /// The kind as messages name it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Function => "function",
            Kind::Table => "table",
            Kind::Memory => "memory",
            Kind::Global => "global",
            Kind::Tag => "tag",
        }
    }
}

/// One import of a module: the module it comes from (for wasm-bindgen's glue, the name under which the glue hands its functions over), its name there, and what it is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Import {
    /// The module it comes from.
    pub module: String,
    /// Its name in that module.
    pub name: String,
    /// What it is.
    pub kind: Kind,
}

/// An import that the module may have: one entry of `WASM_IMPORTS` in `artifacts.rs`.
#[derive(Debug, Clone, Copy)]
pub struct Allowed {
    /// The module it comes from.
    pub module: &'static str,
    /// Its name in that module.
    pub name: &'static str,
    /// What it is.
    pub kind: Kind,
    /// What it does and why the module needs it.
    pub reason: &'static str,
}

/// The magic number and version 1 that begin every WebAssembly module in the binary format.
const HEADER: [u8; 8] = [0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];

/// The identifier of the import section.
const IMPORT_SECTION: u8 = 2;

/// Reads the imports of the WebAssembly module `bytes`, in the order the module lists them.
pub fn imports(bytes: &[u8]) -> Result<Vec<Import>, String> {
    if bytes.get(..HEADER.len()) != Some(&HEADER[..]) {
        return Err(
            "it is not a WebAssembly module in the binary format, version 1 (it does not start with `\\0asm` and version 1)"
                .to_owned(),
        );
    }
    let mut reader = Reader {
        bytes,
        at: HEADER.len(),
    };
    let mut found = None;
    while !reader.done() {
        let id = reader.byte()?;
        let size = reader.length()?;
        let content = reader.take(size)?;
        if id == IMPORT_SECTION {
            if found.is_some() {
                return Err("it has two import sections".to_owned());
            }
            found = Some(import_section(content)?);
        }
    }
    Ok(found.unwrap_or_default())
}

/// Reads the content of an import section: the number of imports, then each import.
fn import_section(content: &[u8]) -> Result<Vec<Import>, String> {
    let mut reader = Reader {
        bytes: content,
        at: 0,
    };
    let count = reader.length()?;
    let mut imports = Vec::new();
    for _ in 0..count {
        let module = reader.name()?;
        let name = reader.name()?;
        let kind = match reader.byte()? {
            0x00 => {
                reader.unsigned(32)?;
                Kind::Function
            }
            0x01 => {
                reader.reference_type()?;
                reader.limits()?;
                Kind::Table
            }
            0x02 => {
                reader.limits()?;
                Kind::Memory
            }
            0x03 => {
                reader.value_type()?;
                if reader.byte()? > 1 {
                    return Err(format!(
                        "the global `{module}` `{name}` is neither constant nor mutable"
                    ));
                }
                Kind::Global
            }
            0x04 => {
                if reader.byte()? != 0 {
                    return Err(format!(
                        "the tag `{module}` `{name}` has an attribute this reader does not know"
                    ));
                }
                reader.unsigned(32)?;
                Kind::Tag
            }
            other => {
                return Err(format!(
                    "the import `{module}` `{name}` is of a kind this reader does not know ({other:#04x})"
                ));
            }
        };
        imports.push(Import { module, name, kind });
    }
    if !reader.done() {
        return Err(
            "its import section holds more bytes than its imports take, which this reader cannot follow"
                .to_owned(),
        );
    }
    Ok(imports)
}

/// Reads the binary format from a slice, failing when it ends too early.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    fn byte(&mut self) -> Result<u8, String> {
        let byte = *self
            .bytes
            .get(self.at)
            .ok_or("it ends in the middle of a section")?;
        self.at += 1;
        Ok(byte)
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(length)
            .filter(|&end| end <= self.bytes.len())
            .ok_or("a section or name is longer than what is left of the module")?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    /// An unsigned integer of at most `bits` bits in LEB128, the variable-length encoding of the binary format, which uses at most as many bytes as it needs (`bits` / 7, rounded up) and no bits beyond `bits`.
    fn unsigned(&mut self, bits: u32) -> Result<u64, String> {
        let mut value = 0_u64;
        let mut shift = 0;
        loop {
            let byte = self.byte()?;
            let payload = u64::from(byte & 0x7F);
            if shift >= bits || (bits - shift < 7 && payload >> (bits - shift) != 0) {
                return Err(format!(
                    "a number is longer than {bits} bits, which the binary format does not allow there"
                ));
            }
            value |= payload << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }

    /// A length or count: an unsigned 32-bit integer.
    fn length(&mut self) -> Result<usize, String> {
        let value = self.unsigned(32)?;
        usize::try_from(value).map_err(|_| format!("a length of {value} does not fit in memory"))
    }

    /// A name: its length in bytes, then UTF-8.
    fn name(&mut self) -> Result<String, String> {
        let length = self.length()?;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| "a name of an import is not valid UTF-8".to_owned())
    }

    /// The limits of a table or memory: flags (whether there is a maximum, whether the memory is shared, whether the numbers are 64 bits), the minimum, and the maximum if there is one.
    fn limits(&mut self) -> Result<(), String> {
        let flags = self.byte()?;
        if flags > 0x07 {
            return Err(format!(
                "a table or memory has limits this reader does not know ({flags:#04x})"
            ));
        }
        let bits = if flags & 0x04 == 0 { 32 } else { 64 };
        self.unsigned(bits)?;
        if flags & 0x01 != 0 {
            self.unsigned(bits)?;
        }
        Ok(())
    }

    /// The type of a table's references: functions (`funcref`) or host values (`externref`).
    fn reference_type(&mut self) -> Result<(), String> {
        match self.byte()? {
            0x70 | 0x6F => Ok(()),
            other => Err(format!(
                "a table holds references of a type this reader does not know ({other:#04x})"
            )),
        }
    }

    /// The type of a global: a number, a vector or a reference.
    fn value_type(&mut self) -> Result<(), String> {
        match self.byte()? {
            0x7F | 0x7E | 0x7D | 0x7C | 0x7B | 0x70 | 0x6F => Ok(()),
            other => Err(format!(
                "a global has a type this reader does not know ({other:#04x})"
            )),
        }
    }
}

/// Checks the imports of the module `bytes` against `allowed`: every import must be listed, with its kind, and every listed import must be one of the module's. Returns one line describing what was checked, or every import that is not listed and every entry that is stale, followed by all of the module's imports.
pub fn check(bytes: &[u8], allowed: &[Allowed]) -> Result<String, String> {
    let found = imports(bytes).map_err(|problem| {
        format!("cannot read the imports of the engine's WebAssembly module: {problem}")
    })?;
    let listed = |import: &Import| {
        allowed.iter().any(|entry| {
            entry.module == import.module && entry.name == import.name && entry.kind == import.kind
        })
    };
    let shown =
        |module: &str, name: &str, kind: Kind| format!("{} `{module}` `{name}`", kind.name());
    let unlisted: Vec<String> = found
        .iter()
        .filter(|import| !listed(import))
        .map(|import| format!("  {}", shown(&import.module, &import.name, import.kind)))
        .collect();
    let stale: Vec<String> = allowed
        .iter()
        .filter(|entry| {
            !found.iter().any(|import| {
                entry.module == import.module
                    && entry.name == import.name
                    && entry.kind == import.kind
            })
        })
        .map(|entry| format!("  {}", shown(entry.module, entry.name, entry.kind)))
        .collect();
    if unlisted.is_empty() && stale.is_empty() {
        let reasons: Vec<String> = allowed
            .iter()
            .map(|entry| {
                format!(
                    "\n  {}: {}",
                    shown(entry.module, entry.name, entry.kind),
                    entry.reason
                )
            })
            .collect();
        return Ok(format!(
            "the engine's WebAssembly module has the {} imports that WASM_IMPORTS lists, and no others:{}",
            found.len(),
            reasons.concat()
        ));
    }
    let mut problems = Vec::new();
    if !unlisted.is_empty() {
        problems.push(format!(
            "the engine's WebAssembly module imports what WASM_IMPORTS in xtask/src/artifacts.rs does not list:\n{}\nEvery import is a way out of the module to its JavaScript host, and the engine never touches the outside world itself (ADR-0012 §5): a clock, randomness, the network or anything else of the browser comes in only through an import. Find the code that declares it (usually a dependency, through wasm-bindgen, js-sys or web-sys). If the engine must not have it, remove that code or the feature of the dependency that brings it. If the import is only wasm-bindgen's plumbing, or a platform call that an ADR accepts (ADR-0012's amendment of 2026-10-08 accepts Loro's randomness and clock), add it to WASM_IMPORTS with what it does and why the engine needs it, in a reviewed pull request.",
            unlisted.join("\n")
        ));
    }
    if !stale.is_empty() {
        problems.push(format!(
            "WASM_IMPORTS in xtask/src/artifacts.rs lists imports that the engine's WebAssembly module does not have:\n{}\nRemove them, so that the list names only what the module needs and cannot go stale.",
            stale.join("\n")
        ));
    }
    let all: Vec<String> = found
        .iter()
        .map(|import| format!("  {}", shown(&import.module, &import.name, import.kind)))
        .collect();
    problems.push(format!(
        "All {} imports of the module:\n{}",
        found.len(),
        all.join("\n")
    ));
    Err(problems.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bytes` with its length in front, as the binary format writes a name or a section's content (lengths below 128 take one byte).
    fn prefixed(bytes: &[u8]) -> Vec<u8> {
        let mut out = vec![u8::try_from(bytes.len()).unwrap()];
        out.extend_from_slice(bytes);
        out
    }

    /// One import: its module, its name, and its kind and description in the binary format.
    fn import(module: &str, name: &str, description: &[u8]) -> Vec<u8> {
        let mut out = prefixed(module.as_bytes());
        out.extend(prefixed(name.as_bytes()));
        out.extend_from_slice(description);
        out
    }

    /// A module with a type section, the import section holding `imports`, and a custom section after it.
    fn module(imports: &[Vec<u8>]) -> Vec<u8> {
        let mut content = vec![u8::try_from(imports.len()).unwrap()];
        for entry in imports {
            content.extend_from_slice(entry);
        }
        let mut out = HEADER.to_vec();
        // A type section with one function type, () -> ().
        out.extend([0x01, 0x04, 0x01, 0x60, 0x00, 0x00]);
        out.push(IMPORT_SECTION);
        out.extend(prefixed(&content));
        // A custom section, named "name", which is skipped.
        out.push(0x00);
        out.extend(prefixed(&prefixed(b"name")));
        out
    }

    fn found(module: &str, name: &str, kind: Kind) -> Import {
        Import {
            module: module.to_owned(),
            name: name.to_owned(),
            kind,
        }
    }

    #[test]
    fn reads_every_kind_of_import() {
        let bytes = module(&[
            import("wbg", "__wbindgen_throw", &[0x00, 0x00]),
            import("env", "table", &[0x01, 0x6F, 0x00, 0x01]),
            import("env", "memory", &[0x02, 0x01, 0x11, 0x80, 0x01]),
            import("env", "stack", &[0x03, 0x7F, 0x01]),
            import("env", "error", &[0x04, 0x00, 0x00]),
        ]);
        assert_eq!(
            imports(&bytes).unwrap(),
            [
                found("wbg", "__wbindgen_throw", Kind::Function),
                found("env", "table", Kind::Table),
                found("env", "memory", Kind::Memory),
                found("env", "stack", Kind::Global),
                found("env", "error", Kind::Tag),
            ]
        );
        // A module without an import section imports nothing.
        assert_eq!(imports(&HEADER).unwrap(), []);
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        let throw = import("wbg", "__wbindgen_throw", &[0x00, 0x00]);
        let valid = module(std::slice::from_ref(&throw));
        for (bytes, problem) in [
            (
                b"\x7fELF\x02\x01\x01\x00".to_vec(),
                "not a WebAssembly module",
            ),
            (
                [&HEADER[..4], &[0x02, 0x00, 0x00, 0x00][..]].concat(),
                "not a WebAssembly module",
            ),
            (
                valid[..valid.len() - 3].to_vec(),
                "longer than what is left",
            ),
            (
                [&valid[..], &[IMPORT_SECTION, 0x01, 0x00][..]].concat(),
                "two import sections",
            ),
            (
                module(&[import("wbg", "x", &[0x05, 0x00])]),
                "a kind this reader does not know (0x05)",
            ),
            (
                module(&[import("wbg", "x", &[0x01, 0x64, 0x00, 0x01])]),
                "references of a type this reader does not know",
            ),
            (
                module(&[import("wbg", "x", &[0x02, 0x08, 0x01])]),
                "limits this reader does not know",
            ),
            (
                module(&[import("wbg", "x", &[0x03, 0x7F, 0x02])]),
                "neither constant nor mutable",
            ),
            (
                module(&[import("wbg", "\u{ff}", &[0x00, 0x00])])
                    .replace_bytes(b"\xc3\xbf", b"\xff\xff"),
                "not valid UTF-8",
            ),
            (
                // A type index of six bytes, more than 32 bits.
                module(&[import(
                    "wbg",
                    "x",
                    &[0x00, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00],
                )]),
                "longer than 32 bits",
            ),
            (
                // A type index whose fifth byte sets bits above 32.
                module(&[import("wbg", "x", &[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F])]),
                "longer than 32 bits",
            ),
        ] {
            let error = imports(&bytes).unwrap_err();
            assert!(error.contains(problem), "{problem}: {error}");
        }
        // An import section whose content is longer than its imports.
        let mut padded = vec![0x01];
        padded.extend(&throw);
        padded.push(0x00);
        let mut bytes = HEADER.to_vec();
        bytes.push(IMPORT_SECTION);
        bytes.extend(prefixed(&padded));
        let error = imports(&bytes).unwrap_err();
        assert!(
            error.contains("more bytes than its imports take"),
            "{error}"
        );
    }

    /// Replaces bytes in a module, for the tests that break one on purpose.
    trait ReplaceBytes {
        fn replace_bytes(self, from: &[u8], to: &[u8]) -> Vec<u8>;
    }

    impl ReplaceBytes for Vec<u8> {
        fn replace_bytes(self, from: &[u8], to: &[u8]) -> Vec<u8> {
            let at = self
                .windows(from.len())
                .position(|window| window == from)
                .unwrap();
            [&self[..at], to, &self[at + from.len()..]].concat()
        }
    }

    const THROW: Allowed = Allowed {
        module: "wbg",
        name: "__wbindgen_throw",
        kind: Kind::Function,
        reason: "throws a JavaScript error",
    };

    #[test]
    fn passes_a_module_with_exactly_the_listed_imports() {
        let bytes = module(&[import("wbg", "__wbindgen_throw", &[0x00, 0x00])]);
        assert_eq!(
            check(&bytes, &[THROW]).unwrap(),
            "the engine's WebAssembly module has the 1 imports that WASM_IMPORTS lists, and no others:\n  function `wbg` `__wbindgen_throw`: throws a JavaScript error"
        );
    }

    #[test]
    fn fails_on_an_import_that_is_not_listed_or_an_entry_that_is_stale() {
        let bytes = module(&[
            import("wbg", "__wbg_now_0123456789abcdef", &[0x00, 0x00]),
            // Listed as a function, but imported as a global.
            import("wbg", "__wbindgen_throw", &[0x03, 0x7F, 0x00]),
        ]);
        let problem = check(&bytes, &[THROW]).unwrap_err();
        for expected in [
            "does not list:\n  function `wbg` `__wbg_now_0123456789abcdef`\n  global `wbg` `__wbindgen_throw`\n",
            "ADR-0012 §5",
            "does not have:\n  function `wbg` `__wbindgen_throw`\n",
            "All 2 imports of the module:\n  function `wbg` `__wbg_now_0123456789abcdef`\n  global `wbg` `__wbindgen_throw`",
        ] {
            assert!(problem.contains(expected), "{expected}:\n{problem}");
        }
        let unreadable = check(b"not a module", &[THROW]).unwrap_err();
        assert!(
            unreadable.starts_with("cannot read the imports of the engine's WebAssembly module"),
            "{unreadable}"
        );
    }
}
