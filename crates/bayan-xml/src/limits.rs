//! Resource limits for reading untrusted XML.

/// The limits bayan-xml enforces while reading a part (ADR-0006 §5: every parser limits sizes, depths and counts).
///
/// A document part is hostile input. Its size bounds the work of reading it, which grows linearly with the size; the other limits bound the state the reader keeps while it reads, which would otherwise grow with nesting and with the number of names in scope, and the cost of each step. Every limit is checked before the work it protects, and every one is a field that a host can raise for a document the user trusts:
///
/// ```
/// let limits = bayan_xml::Limits {
///     max_depth: 1_024,
///     ..bayan_xml::Limits::default()
/// };
/// assert_eq!(limits.max_attributes, 256);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The largest input, in bytes, before it is decoded. Default: 256 MiB, far more than the main part of a document of thousands of pages. bayan-opc's limit on the size of a part (512 MiB) comes first in practice; this one protects callers that read XML from elsewhere. Reading takes time in proportion to the size.
    pub max_size: usize,
    /// The deepest nesting of elements; the root element is at depth 1. Default: 256 (WordprocessingML nests about 15 levels deep, and every table nested in a table adds about four).
    pub max_depth: usize,
    /// The most attributes of one element, namespace declarations included. Default: 256 (the root element of a document that Word 365 writes declares about 35 namespaces; no element of the format has more than about 30 attributes).
    pub max_attributes: usize,
    /// The longest name, in bytes: element and attribute names with their prefix, processing instruction targets and entity names. Default: 256 (the longest names of the format have about 40 characters).
    pub max_name_length: usize,
    /// The most distinct namespace names (URIs) one document may declare. Default: 4,096 (a document that Word writes declares fewer than 50).
    pub max_namespaces: usize,
    /// The longest namespace name (URI) a declaration may give, in bytes, after its references are resolved. Default: 2,048 (the names of Office Open XML and of Microsoft's extensions have at most about 80 bytes, and those of custom XML rarely more than a few hundred). However long the names, reading, Markup Compatibility, writing and the canonical form take no longer per element; the limit bounds the copies of names that contexts and fragments share, and the declarations that writing a fragment where its namespaces are not declared adds.
    pub max_namespace_name_length: usize,
    /// The most names one Markup Compatibility attribute may list (`mc:Ignorable`, `mc:ProcessContent`, `mc:MustUnderstand`, `mc:PreserveElements`, `mc:PreserveAttributes` and the `Requires` attribute of `mc:Choice`). Default: 256 (Word lists about ten prefixes in `mc:Ignorable`).
    pub max_compatibility_tokens: usize,
}

impl Limits {
    /// The default limits, described with each field.
    pub const DEFAULT: Limits = Limits {
        max_size: 256 << 20,
        max_depth: 256,
        max_attributes: 256,
        max_name_length: 256,
        max_namespaces: 4_096,
        max_namespace_name_length: 2_048,
        max_compatibility_tokens: 256,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Limits::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_documented_defaults() {
        // The crate documentation states these values and the bounds on time and memory that follow from them; a change to one of them must update both.
        let limits = Limits::default();
        assert_eq!(limits.max_size, 256 * 1024 * 1024);
        assert_eq!(limits.max_depth, 256);
        assert_eq!(limits.max_attributes, 256);
        assert_eq!(limits.max_name_length, 256);
        assert_eq!(limits.max_namespaces, 4_096);
        assert_eq!(limits.max_namespace_name_length, 2_048);
        assert_eq!(limits.max_compatibility_tokens, 256);
    }
}
