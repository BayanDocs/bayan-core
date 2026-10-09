//! Resource limits for reading untrusted packages and compound files.

/// The limits bayan-opc enforces while reading a package or a compound file (ADR-0006 §5: every parser limits sizes, depths and counts).
///
/// A package is hostile input: a few kilobytes of ZIP can claim gigabytes of content (a "zip bomb"), hold millions of entries, or nest XML deeply enough to exhaust memory. Every check below happens before the work it protects: entry counts, names and declared sizes are checked while the central directory is read, before anything is decompressed, and a part is decompressed into a buffer of exactly its declared size, which decompression may never exceed. Memory therefore stays bounded by these numbers whatever the input claims.
///
/// The defaults ([`Limits::DEFAULT`]) open ordinary documents comfortably and stop malicious ones; a host can raise a limit (for example after asking the user, for a document with very large embedded media) by changing one field:
///
/// ```
/// let limits = bayan_opc::Limits {
///     max_entry_size: 2 << 30,
///     ..bayan_opc::Limits::default()
/// };
/// assert_eq!(limits.max_entries, 10_000);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most entries a ZIP archive may have, folders included. Default: 10,000 (a typical document has 10 to 50 parts; one with hundreds of images has a few hundred).
    pub max_entries: usize,
    /// The longest entry name, in bytes. Default: 1,024 (Word's own part names are short; Windows paths are limited to about 260 characters).
    pub max_name_length: usize,
    /// The most bytes one entry may uncompress to, and the largest compound file stream that may be read. Default: 512 MiB.
    pub max_entry_size: u64,
    /// The most bytes all entries together may uncompress to, as their headers declare. Default: 2 GiB.
    pub max_total_size: u64,
    /// The highest compression ratio (uncompressed size divided by compressed size) of an entry, and of the archive as a whole (all uncompressed sizes divided by the size of the input). DEFLATE compresses at most about 1,032 to 1, and the very highest ratios come only from long runs of identical bytes, which is how zip bombs are built; ordinary document XML compresses about 5 to 20 to 1. Default: 100, the ratio Apache POI also uses. Entries, and archives, that uncompress to no more than [`Limits::compression_ratio_grace`] bytes are exempt.
    pub max_compression_ratio: u64,
    /// Entries, and archives, that uncompress to at most this many bytes are exempt from [`Limits::max_compression_ratio`]: small data can compress very well and cannot exhaust anything. Default: 1 MiB.
    pub compression_ratio_grace: u64,
    /// The largest metadata part (the content types stream, a relationships part or the core properties part) that is parsed, in bytes. Default: 32 MiB (a content types stream for 10,000 parts takes about 1.5 MB).
    pub max_metadata_size: usize,
    /// The deepest nesting of elements in a metadata part. Default: 64 (these parts nest two or three levels deep).
    pub max_xml_depth: usize,
    /// The most attributes, namespace declarations included, of one element in a metadata part. Default: 64.
    pub max_xml_attributes: usize,
    /// The longest element or attribute name in a metadata part, in bytes. Default: 256.
    pub max_xml_name_length: usize,
    /// The most entries (storages and streams) of an OLE compound file. Default: 65,536.
    pub max_compound_file_entries: usize,
}

impl Limits {
    /// The default limits, described with each field.
    pub const DEFAULT: Limits = Limits {
        max_entries: 10_000,
        max_name_length: 1_024,
        max_entry_size: 512 << 20,
        max_total_size: 2 << 30,
        max_compression_ratio: 100,
        compression_ratio_grace: 1 << 20,
        max_metadata_size: 32 << 20,
        max_xml_depth: 64,
        max_xml_attributes: 64,
        max_xml_name_length: 256,
        max_compound_file_entries: 65_536,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Limits::DEFAULT
    }
}
