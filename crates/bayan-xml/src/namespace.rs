//! Namespaces: the table of namespaces that Office Open XML documents use, with the mapping between the Strict and Transitional names of ECMA-376's own vocabularies, and the names that identify a namespace in a configuration.

/// A namespace that Office Open XML documents use, from ECMA-376 (Parts 1 to 4), [MS-DOCX] and [MS-ODRAWXML], with the W3C namespaces they rely on.
///
/// ECMA-376 gives each of its own vocabularies two names: a Transitional one (`http://schemas.openxmlformats.org/…`, Part 4) and a Strict one (`http://purl.oclc.org/ooxml/…`, Part 1). Both map to the same variant, so code that interprets a document matches a vocabulary once for both conformance classes; [`Namespace::is_strict`] tells them apart. Everything else (Markup Compatibility, the packaging namespaces, VML and Microsoft's extensions) has a single name, used by both.
///
/// Each variant's documentation gives its namespace name (URI) and the prefix that documents conventionally use for it. Namespaces that are not in this table are fully supported too, by their URI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum KnownNamespace {
    /// `xml`: `http://www.w3.org/XML/1998/namespace`, bound to the prefix `xml` in every document (`xml:space`, `xml:lang`).
    Xml,
    /// `xmlns`: `http://www.w3.org/2000/xmlns/`, the namespace of namespace declarations.
    Xmlns,
    /// `xsd`: `http://www.w3.org/2001/XMLSchema`.
    XmlSchema,
    /// `xsi`: `http://www.w3.org/2001/XMLSchema-instance` (`xsi:type` in core properties).
    XmlSchemaInstance,
    /// `mc`: `http://schemas.openxmlformats.org/markup-compatibility/2006`, Markup Compatibility and Extensibility (Part 3).
    MarkupCompatibility,
    /// `ct`: `http://schemas.openxmlformats.org/package/2006/content-types`, the content types stream (Part 2).
    ContentTypes,
    /// `rels`: `http://schemas.openxmlformats.org/package/2006/relationships`, relationships parts (Part 2).
    PackageRelationships,
    /// `cp`: `http://schemas.openxmlformats.org/package/2006/metadata/core-properties`, the core properties part (Part 2).
    CoreProperties,
    /// `mdssi`: `http://schemas.openxmlformats.org/package/2006/digital-signature` (Part 2).
    DigitalSignature,
    /// `dc`: `http://purl.org/dc/elements/1.1/`, Dublin Core elements in the core properties.
    DublinCore,
    /// `dcterms`: `http://purl.org/dc/terms/`.
    DublinCoreTerms,
    /// `dcmitype`: `http://purl.org/dc/dcmitype/`.
    DublinCoreTypes,
    /// `w`: WordprocessingML. Transitional `http://schemas.openxmlformats.org/wordprocessingml/2006/main`, Strict `http://purl.oclc.org/ooxml/wordprocessingml/main`.
    WordprocessingMl,
    /// `sml` (prefix ours; the standard writes SpreadsheetML without one): SpreadsheetML. Transitional `http://schemas.openxmlformats.org/spreadsheetml/2006/main`, Strict `http://purl.oclc.org/ooxml/spreadsheetml/main`.
    SpreadsheetMl,
    /// `p`: PresentationML. Transitional `http://schemas.openxmlformats.org/presentationml/2006/main`, Strict `http://purl.oclc.org/ooxml/presentationml/main`.
    PresentationMl,
    /// `a`: DrawingML. Transitional `http://schemas.openxmlformats.org/drawingml/2006/main`, Strict `http://purl.oclc.org/ooxml/drawingml/main`.
    DrawingMl,
    /// `c`: DrawingML charts. Transitional `http://schemas.openxmlformats.org/drawingml/2006/chart`, Strict `http://purl.oclc.org/ooxml/drawingml/chart`.
    DrawingMlChart,
    /// `cdr`: DrawingML chart drawings. Transitional `http://schemas.openxmlformats.org/drawingml/2006/chartDrawing`, Strict `http://purl.oclc.org/ooxml/drawingml/chartDrawing`.
    DrawingMlChartDrawing,
    /// `dgm`: DrawingML diagrams (SmartArt). Transitional `http://schemas.openxmlformats.org/drawingml/2006/diagram`, Strict `http://purl.oclc.org/ooxml/drawingml/diagram`.
    DrawingMlDiagram,
    /// `lc`: DrawingML locked canvases. Transitional `http://schemas.openxmlformats.org/drawingml/2006/lockedCanvas`, Strict `http://purl.oclc.org/ooxml/drawingml/lockedCanvas`.
    DrawingMlLockedCanvas,
    /// `pic`: DrawingML pictures. Transitional `http://schemas.openxmlformats.org/drawingml/2006/picture`, Strict `http://purl.oclc.org/ooxml/drawingml/picture`.
    DrawingMlPicture,
    /// `xdr`: DrawingML in spreadsheets. Transitional `http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing`, Strict `http://purl.oclc.org/ooxml/drawingml/spreadsheetDrawing`.
    DrawingMlSpreadsheetDrawing,
    /// `wp`: DrawingML in word-processing documents. Transitional `http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing`, Strict `http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing`.
    DrawingMlWordprocessingDrawing,
    /// `r`: relationship references (`r:id`). Transitional `http://schemas.openxmlformats.org/officeDocument/2006/relationships`, Strict `http://purl.oclc.org/ooxml/officeDocument/relationships`.
    Relationships,
    /// `m`: Office Math. Transitional `http://schemas.openxmlformats.org/officeDocument/2006/math`, Strict `http://purl.oclc.org/ooxml/officeDocument/math`.
    Math,
    /// `s`: shared simple types. Transitional `http://schemas.openxmlformats.org/officeDocument/2006/sharedTypes`, Strict `http://purl.oclc.org/ooxml/officeDocument/sharedTypes`.
    SharedTypes,
    /// `ds`: custom XML data properties. Transitional `http://schemas.openxmlformats.org/officeDocument/2006/customXml`, Strict `http://purl.oclc.org/ooxml/officeDocument/customXml`.
    CustomXml,
    /// `sl`: the schema library. Transitional `http://schemas.openxmlformats.org/schemaLibrary/2006/main`, Strict `http://purl.oclc.org/ooxml/schemaLibrary/main`.
    SchemaLibrary,
    /// `b`: bibliography sources. Transitional `http://schemas.openxmlformats.org/officeDocument/2006/bibliography`, Strict `http://purl.oclc.org/ooxml/officeDocument/bibliography`.
    Bibliography,
    /// `ch` (prefix ours): additional characteristics. Transitional `http://schemas.openxmlformats.org/officeDocument/2006/characteristics`, Strict `http://purl.oclc.org/ooxml/officeDocument/characteristics`.
    Characteristics,
    /// `ep` (prefix ours): extended (application) properties. Transitional `http://schemas.openxmlformats.org/officeDocument/2006/extended-properties`, Strict `http://purl.oclc.org/ooxml/officeDocument/extendedProperties`.
    ExtendedProperties,
    /// `cust` (prefix ours): custom properties. Transitional `http://schemas.openxmlformats.org/officeDocument/2006/custom-properties`, Strict `http://purl.oclc.org/ooxml/officeDocument/customProperties`.
    CustomProperties,
    /// `vt`: variant types of document properties. Transitional `http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes`, Strict `http://purl.oclc.org/ooxml/officeDocument/docPropsVTypes`.
    VariantTypes,
    /// `v`: VML, `urn:schemas-microsoft-com:vml` (Part 4 only).
    Vml,
    /// `o`: VML's Office drawing extensions, `urn:schemas-microsoft-com:office:office` (Part 4 only).
    VmlOffice,
    /// `w10`: VML's word-processing extensions, `urn:schemas-microsoft-com:office:word` (Part 4 only).
    VmlWord,
    /// `x`: VML's spreadsheet extensions, `urn:schemas-microsoft-com:office:excel` (Part 4 only).
    VmlExcel,
    /// `pvml`: VML's presentation extensions, `urn:schemas-microsoft-com:office:powerpoint` (Part 4 only).
    VmlPowerPoint,
    /// `wne`: `http://schemas.microsoft.com/office/word/2006/wordml`.
    Wne,
    /// `w14`: `http://schemas.microsoft.com/office/word/2010/wordml` ([MS-DOCX] §2.6).
    W14,
    /// `w15`: `http://schemas.microsoft.com/office/word/2012/wordml` ([MS-DOCX] §2.5).
    W15,
    /// `w16se`: `http://schemas.microsoft.com/office/word/2015/wordml/symex` ([MS-DOCX] §2.7).
    W16Se,
    /// `w16cid`: `http://schemas.microsoft.com/office/word/2016/wordml/cid` ([MS-DOCX] §2.8).
    W16Cid,
    /// `w16`: `http://schemas.microsoft.com/office/word/2018/wordml` ([MS-DOCX] §2.9).
    W16,
    /// `w16cex`: `http://schemas.microsoft.com/office/word/2018/wordml/cex` ([MS-DOCX] §2.10).
    W16Cex,
    /// `w16sdtdh`: `http://schemas.microsoft.com/office/word/2020/wordml/sdtdatahash` ([MS-DOCX] §2.11).
    W16SdtDh,
    /// `w16du`: `http://schemas.microsoft.com/office/word/2023/wordml/word16du` ([MS-DOCX] §2.12).
    W16Du,
    /// `w16sdtfl`: `http://schemas.microsoft.com/office/word/2024/wordml/sdtformatlock` ([MS-DOCX] §2.13).
    W16SdtFl,
    /// `w16cei` (prefix ours; [MS-DOCX] gives none): `http://schemas.microsoft.com/office/word/2026/wordml/cei` ([MS-DOCX] §2.14).
    W16Cei,
    /// `wp14`: `http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing` ([MS-ODRAWXML]).
    Wp14,
    /// `wp15`: `http://schemas.microsoft.com/office/word/2012/wordprocessingDrawing` ([MS-ODRAWXML]).
    Wp15,
    /// `wpc`: `http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas` ([MS-ODRAWXML]).
    Wpc,
    /// `wpg`: `http://schemas.microsoft.com/office/word/2010/wordprocessingGroup` ([MS-ODRAWXML]).
    Wpg,
    /// `wpi`: `http://schemas.microsoft.com/office/word/2010/wordprocessingInk` ([MS-ODRAWXML]).
    Wpi,
    /// `wps`: `http://schemas.microsoft.com/office/word/2010/wordprocessingShape` ([MS-ODRAWXML]).
    Wps,
    /// `a14`: `http://schemas.microsoft.com/office/drawing/2010/main` ([MS-ODRAWXML]).
    A14,
    /// `a15`: `http://schemas.microsoft.com/office/drawing/2012/main` ([MS-ODRAWXML]).
    A15,
    /// `a16`: `http://schemas.microsoft.com/office/drawing/2014/main` ([MS-ODRAWXML]).
    A16,
    /// `a1611`: `http://schemas.microsoft.com/office/drawing/2016/11/main` ([MS-ODRAWXML]).
    A1611,
    /// `aink`: `http://schemas.microsoft.com/office/drawing/2016/ink` ([MS-ODRAWXML]).
    Aink,
    /// `am3d`: `http://schemas.microsoft.com/office/drawing/2017/model3d` ([MS-ODRAWXML]).
    Am3d,
    /// `asvg`: `http://schemas.microsoft.com/office/drawing/2016/SVG/main` ([MS-ODRAWXML]).
    Asvg,
    /// `adec`: `http://schemas.microsoft.com/office/drawing/2017/decorative` ([MS-ODRAWXML]).
    Adec,
    /// `pic14`: `http://schemas.microsoft.com/office/drawing/2010/picture` ([MS-ODRAWXML]).
    Pic14,
    /// `c14`: `http://schemas.microsoft.com/office/drawing/2007/8/2/chart` ([MS-ODRAWXML]).
    C14,
    /// `c15`: `http://schemas.microsoft.com/office/drawing/2012/chart` ([MS-ODRAWXML]).
    C15,
    /// `c16`: `http://schemas.microsoft.com/office/drawing/2014/chart` ([MS-ODRAWXML]).
    C16,
    /// `c16r2`: `http://schemas.microsoft.com/office/drawing/2015/06/chart` ([MS-ODRAWXML]).
    C16R2,
    /// `c16r3`: `http://schemas.microsoft.com/office/drawing/2017/03/chart` ([MS-ODRAWXML]).
    C16R3,
    /// `cx`: `http://schemas.microsoft.com/office/drawing/2014/chartex` ([MS-ODRAWXML]).
    Cx,
    /// `cx1`: `http://schemas.microsoft.com/office/drawing/2015/9/8/chartex`.
    Cx1,
    /// `cx2`: `http://schemas.microsoft.com/office/drawing/2015/10/21/chartex`.
    Cx2,
    /// `cx3`: `http://schemas.microsoft.com/office/drawing/2016/5/9/chartex`.
    Cx3,
    /// `cx4`: `http://schemas.microsoft.com/office/drawing/2016/5/10/chartex`.
    Cx4,
    /// `cx5`: `http://schemas.microsoft.com/office/drawing/2016/5/11/chartex`.
    Cx5,
    /// `cx6`: `http://schemas.microsoft.com/office/drawing/2016/5/12/chartex`.
    Cx6,
    /// `cx7`: `http://schemas.microsoft.com/office/drawing/2016/5/13/chartex`.
    Cx7,
    /// `cx8`: `http://schemas.microsoft.com/office/drawing/2016/5/14/chartex`.
    Cx8,
    /// `dgm14`: `http://schemas.microsoft.com/office/drawing/2010/diagram` ([MS-ODRAWXML]).
    Dgm14,
    /// `dsp`: `http://schemas.microsoft.com/office/drawing/2008/diagram`, the drawing that Office stores with a diagram ([MS-ODRAWXML]).
    Dsp,
    /// `thm15`: `http://schemas.microsoft.com/office/thememl/2012/main` ([MS-ODRAWXML]).
    Thm15,
}

/// A namespace name (URI) found in [`KnownNamespace`]'s table, and which of the vocabulary's names it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownUri {
    /// The namespace.
    pub namespace: KnownNamespace,
    /// Whether the URI is the Strict name of an ECMA-376 vocabulary (`http://purl.oclc.org/ooxml/…`).
    pub strict: bool,
}

impl KnownNamespace {
    /// Every known namespace, in the order of the declaration.
    pub const ALL: &'static [KnownNamespace] = &[
        KnownNamespace::Xml,
        KnownNamespace::Xmlns,
        KnownNamespace::XmlSchema,
        KnownNamespace::XmlSchemaInstance,
        KnownNamespace::MarkupCompatibility,
        KnownNamespace::ContentTypes,
        KnownNamespace::PackageRelationships,
        KnownNamespace::CoreProperties,
        KnownNamespace::DigitalSignature,
        KnownNamespace::DublinCore,
        KnownNamespace::DublinCoreTerms,
        KnownNamespace::DublinCoreTypes,
        KnownNamespace::WordprocessingMl,
        KnownNamespace::SpreadsheetMl,
        KnownNamespace::PresentationMl,
        KnownNamespace::DrawingMl,
        KnownNamespace::DrawingMlChart,
        KnownNamespace::DrawingMlChartDrawing,
        KnownNamespace::DrawingMlDiagram,
        KnownNamespace::DrawingMlLockedCanvas,
        KnownNamespace::DrawingMlPicture,
        KnownNamespace::DrawingMlSpreadsheetDrawing,
        KnownNamespace::DrawingMlWordprocessingDrawing,
        KnownNamespace::Relationships,
        KnownNamespace::Math,
        KnownNamespace::SharedTypes,
        KnownNamespace::CustomXml,
        KnownNamespace::SchemaLibrary,
        KnownNamespace::Bibliography,
        KnownNamespace::Characteristics,
        KnownNamespace::ExtendedProperties,
        KnownNamespace::CustomProperties,
        KnownNamespace::VariantTypes,
        KnownNamespace::Vml,
        KnownNamespace::VmlOffice,
        KnownNamespace::VmlWord,
        KnownNamespace::VmlExcel,
        KnownNamespace::VmlPowerPoint,
        KnownNamespace::Wne,
        KnownNamespace::W14,
        KnownNamespace::W15,
        KnownNamespace::W16Se,
        KnownNamespace::W16Cid,
        KnownNamespace::W16,
        KnownNamespace::W16Cex,
        KnownNamespace::W16SdtDh,
        KnownNamespace::W16Du,
        KnownNamespace::W16SdtFl,
        KnownNamespace::W16Cei,
        KnownNamespace::Wp14,
        KnownNamespace::Wp15,
        KnownNamespace::Wpc,
        KnownNamespace::Wpg,
        KnownNamespace::Wpi,
        KnownNamespace::Wps,
        KnownNamespace::A14,
        KnownNamespace::A15,
        KnownNamespace::A16,
        KnownNamespace::A1611,
        KnownNamespace::Aink,
        KnownNamespace::Am3d,
        KnownNamespace::Asvg,
        KnownNamespace::Adec,
        KnownNamespace::Pic14,
        KnownNamespace::C14,
        KnownNamespace::C15,
        KnownNamespace::C16,
        KnownNamespace::C16R2,
        KnownNamespace::C16R3,
        KnownNamespace::Cx,
        KnownNamespace::Cx1,
        KnownNamespace::Cx2,
        KnownNamespace::Cx3,
        KnownNamespace::Cx4,
        KnownNamespace::Cx5,
        KnownNamespace::Cx6,
        KnownNamespace::Cx7,
        KnownNamespace::Cx8,
        KnownNamespace::Dgm14,
        KnownNamespace::Dsp,
        KnownNamespace::Thm15,
    ];

    /// The namespace's name: the Transitional one for ECMA-376's vocabularies, the only one for the others.
    pub fn uri(self) -> &'static str {
        match self {
            KnownNamespace::Xml => "http://www.w3.org/XML/1998/namespace",
            KnownNamespace::Xmlns => "http://www.w3.org/2000/xmlns/",
            KnownNamespace::XmlSchema => "http://www.w3.org/2001/XMLSchema",
            KnownNamespace::XmlSchemaInstance => "http://www.w3.org/2001/XMLSchema-instance",
            KnownNamespace::MarkupCompatibility => {
                "http://schemas.openxmlformats.org/markup-compatibility/2006"
            }
            KnownNamespace::ContentTypes => {
                "http://schemas.openxmlformats.org/package/2006/content-types"
            }
            KnownNamespace::PackageRelationships => {
                "http://schemas.openxmlformats.org/package/2006/relationships"
            }
            KnownNamespace::CoreProperties => {
                "http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
            }
            KnownNamespace::DigitalSignature => {
                "http://schemas.openxmlformats.org/package/2006/digital-signature"
            }
            KnownNamespace::DublinCore => "http://purl.org/dc/elements/1.1/",
            KnownNamespace::DublinCoreTerms => "http://purl.org/dc/terms/",
            KnownNamespace::DublinCoreTypes => "http://purl.org/dc/dcmitype/",
            KnownNamespace::WordprocessingMl => {
                "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            }
            KnownNamespace::SpreadsheetMl => {
                "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
            }
            KnownNamespace::PresentationMl => {
                "http://schemas.openxmlformats.org/presentationml/2006/main"
            }
            KnownNamespace::DrawingMl => "http://schemas.openxmlformats.org/drawingml/2006/main",
            KnownNamespace::DrawingMlChart => {
                "http://schemas.openxmlformats.org/drawingml/2006/chart"
            }
            KnownNamespace::DrawingMlChartDrawing => {
                "http://schemas.openxmlformats.org/drawingml/2006/chartDrawing"
            }
            KnownNamespace::DrawingMlDiagram => {
                "http://schemas.openxmlformats.org/drawingml/2006/diagram"
            }
            KnownNamespace::DrawingMlLockedCanvas => {
                "http://schemas.openxmlformats.org/drawingml/2006/lockedCanvas"
            }
            KnownNamespace::DrawingMlPicture => {
                "http://schemas.openxmlformats.org/drawingml/2006/picture"
            }
            KnownNamespace::DrawingMlSpreadsheetDrawing => {
                "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"
            }
            KnownNamespace::DrawingMlWordprocessingDrawing => {
                "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
            }
            KnownNamespace::Relationships => {
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
            }
            KnownNamespace::Math => "http://schemas.openxmlformats.org/officeDocument/2006/math",
            KnownNamespace::SharedTypes => {
                "http://schemas.openxmlformats.org/officeDocument/2006/sharedTypes"
            }
            KnownNamespace::CustomXml => {
                "http://schemas.openxmlformats.org/officeDocument/2006/customXml"
            }
            KnownNamespace::SchemaLibrary => {
                "http://schemas.openxmlformats.org/schemaLibrary/2006/main"
            }
            KnownNamespace::Bibliography => {
                "http://schemas.openxmlformats.org/officeDocument/2006/bibliography"
            }
            KnownNamespace::Characteristics => {
                "http://schemas.openxmlformats.org/officeDocument/2006/characteristics"
            }
            KnownNamespace::ExtendedProperties => {
                "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"
            }
            KnownNamespace::CustomProperties => {
                "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties"
            }
            KnownNamespace::VariantTypes => {
                "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"
            }
            KnownNamespace::Vml => "urn:schemas-microsoft-com:vml",
            KnownNamespace::VmlOffice => "urn:schemas-microsoft-com:office:office",
            KnownNamespace::VmlWord => "urn:schemas-microsoft-com:office:word",
            KnownNamespace::VmlExcel => "urn:schemas-microsoft-com:office:excel",
            KnownNamespace::VmlPowerPoint => "urn:schemas-microsoft-com:office:powerpoint",
            KnownNamespace::Wne => "http://schemas.microsoft.com/office/word/2006/wordml",
            KnownNamespace::W14 => "http://schemas.microsoft.com/office/word/2010/wordml",
            KnownNamespace::W15 => "http://schemas.microsoft.com/office/word/2012/wordml",
            KnownNamespace::W16Se => "http://schemas.microsoft.com/office/word/2015/wordml/symex",
            KnownNamespace::W16Cid => "http://schemas.microsoft.com/office/word/2016/wordml/cid",
            KnownNamespace::W16 => "http://schemas.microsoft.com/office/word/2018/wordml",
            KnownNamespace::W16Cex => "http://schemas.microsoft.com/office/word/2018/wordml/cex",
            KnownNamespace::W16SdtDh => {
                "http://schemas.microsoft.com/office/word/2020/wordml/sdtdatahash"
            }
            KnownNamespace::W16Du => {
                "http://schemas.microsoft.com/office/word/2023/wordml/word16du"
            }
            KnownNamespace::W16SdtFl => {
                "http://schemas.microsoft.com/office/word/2024/wordml/sdtformatlock"
            }
            KnownNamespace::W16Cei => "http://schemas.microsoft.com/office/word/2026/wordml/cei",
            KnownNamespace::Wp14 => {
                "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing"
            }
            KnownNamespace::Wp15 => {
                "http://schemas.microsoft.com/office/word/2012/wordprocessingDrawing"
            }
            KnownNamespace::Wpc => {
                "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas"
            }
            KnownNamespace::Wpg => {
                "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"
            }
            KnownNamespace::Wpi => {
                "http://schemas.microsoft.com/office/word/2010/wordprocessingInk"
            }
            KnownNamespace::Wps => {
                "http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
            }
            KnownNamespace::A14 => "http://schemas.microsoft.com/office/drawing/2010/main",
            KnownNamespace::A15 => "http://schemas.microsoft.com/office/drawing/2012/main",
            KnownNamespace::A16 => "http://schemas.microsoft.com/office/drawing/2014/main",
            KnownNamespace::A1611 => "http://schemas.microsoft.com/office/drawing/2016/11/main",
            KnownNamespace::Aink => "http://schemas.microsoft.com/office/drawing/2016/ink",
            KnownNamespace::Am3d => "http://schemas.microsoft.com/office/drawing/2017/model3d",
            KnownNamespace::Asvg => "http://schemas.microsoft.com/office/drawing/2016/SVG/main",
            KnownNamespace::Adec => "http://schemas.microsoft.com/office/drawing/2017/decorative",
            KnownNamespace::Pic14 => "http://schemas.microsoft.com/office/drawing/2010/picture",
            KnownNamespace::C14 => "http://schemas.microsoft.com/office/drawing/2007/8/2/chart",
            KnownNamespace::C15 => "http://schemas.microsoft.com/office/drawing/2012/chart",
            KnownNamespace::C16 => "http://schemas.microsoft.com/office/drawing/2014/chart",
            KnownNamespace::C16R2 => "http://schemas.microsoft.com/office/drawing/2015/06/chart",
            KnownNamespace::C16R3 => "http://schemas.microsoft.com/office/drawing/2017/03/chart",
            KnownNamespace::Cx => "http://schemas.microsoft.com/office/drawing/2014/chartex",
            KnownNamespace::Cx1 => "http://schemas.microsoft.com/office/drawing/2015/9/8/chartex",
            KnownNamespace::Cx2 => "http://schemas.microsoft.com/office/drawing/2015/10/21/chartex",
            KnownNamespace::Cx3 => "http://schemas.microsoft.com/office/drawing/2016/5/9/chartex",
            KnownNamespace::Cx4 => "http://schemas.microsoft.com/office/drawing/2016/5/10/chartex",
            KnownNamespace::Cx5 => "http://schemas.microsoft.com/office/drawing/2016/5/11/chartex",
            KnownNamespace::Cx6 => "http://schemas.microsoft.com/office/drawing/2016/5/12/chartex",
            KnownNamespace::Cx7 => "http://schemas.microsoft.com/office/drawing/2016/5/13/chartex",
            KnownNamespace::Cx8 => "http://schemas.microsoft.com/office/drawing/2016/5/14/chartex",
            KnownNamespace::Dgm14 => "http://schemas.microsoft.com/office/drawing/2010/diagram",
            KnownNamespace::Dsp => "http://schemas.microsoft.com/office/drawing/2008/diagram",
            KnownNamespace::Thm15 => "http://schemas.microsoft.com/office/thememl/2012/main",
        }
    }

    /// The Strict name of an ECMA-376 vocabulary, `None` for namespaces that have only one name.
    pub fn strict_uri(self) -> Option<&'static str> {
        Some(match self {
            KnownNamespace::WordprocessingMl => "http://purl.oclc.org/ooxml/wordprocessingml/main",
            KnownNamespace::SpreadsheetMl => "http://purl.oclc.org/ooxml/spreadsheetml/main",
            KnownNamespace::PresentationMl => "http://purl.oclc.org/ooxml/presentationml/main",
            KnownNamespace::DrawingMl => "http://purl.oclc.org/ooxml/drawingml/main",
            KnownNamespace::DrawingMlChart => "http://purl.oclc.org/ooxml/drawingml/chart",
            KnownNamespace::DrawingMlChartDrawing => {
                "http://purl.oclc.org/ooxml/drawingml/chartDrawing"
            }
            KnownNamespace::DrawingMlDiagram => "http://purl.oclc.org/ooxml/drawingml/diagram",
            KnownNamespace::DrawingMlLockedCanvas => {
                "http://purl.oclc.org/ooxml/drawingml/lockedCanvas"
            }
            KnownNamespace::DrawingMlPicture => "http://purl.oclc.org/ooxml/drawingml/picture",
            KnownNamespace::DrawingMlSpreadsheetDrawing => {
                "http://purl.oclc.org/ooxml/drawingml/spreadsheetDrawing"
            }
            KnownNamespace::DrawingMlWordprocessingDrawing => {
                "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing"
            }
            KnownNamespace::Relationships => {
                "http://purl.oclc.org/ooxml/officeDocument/relationships"
            }
            KnownNamespace::Math => "http://purl.oclc.org/ooxml/officeDocument/math",
            KnownNamespace::SharedTypes => "http://purl.oclc.org/ooxml/officeDocument/sharedTypes",
            KnownNamespace::CustomXml => "http://purl.oclc.org/ooxml/officeDocument/customXml",
            KnownNamespace::SchemaLibrary => "http://purl.oclc.org/ooxml/schemaLibrary/main",
            KnownNamespace::Bibliography => {
                "http://purl.oclc.org/ooxml/officeDocument/bibliography"
            }
            KnownNamespace::Characteristics => {
                "http://purl.oclc.org/ooxml/officeDocument/characteristics"
            }
            KnownNamespace::ExtendedProperties => {
                "http://purl.oclc.org/ooxml/officeDocument/extendedProperties"
            }
            KnownNamespace::CustomProperties => {
                "http://purl.oclc.org/ooxml/officeDocument/customProperties"
            }
            KnownNamespace::VariantTypes => {
                "http://purl.oclc.org/ooxml/officeDocument/docPropsVTypes"
            }
            _ => return None,
        })
    }

    /// The prefix that documents conventionally use for the namespace, which [`Writer`](crate::Writer) users can take for it.
    pub fn prefix(self) -> &'static str {
        match self {
            KnownNamespace::Xml => "xml",
            KnownNamespace::Xmlns => "xmlns",
            KnownNamespace::XmlSchema => "xsd",
            KnownNamespace::XmlSchemaInstance => "xsi",
            KnownNamespace::MarkupCompatibility => "mc",
            KnownNamespace::ContentTypes => "ct",
            KnownNamespace::PackageRelationships => "rels",
            KnownNamespace::CoreProperties => "cp",
            KnownNamespace::DigitalSignature => "mdssi",
            KnownNamespace::DublinCore => "dc",
            KnownNamespace::DublinCoreTerms => "dcterms",
            KnownNamespace::DublinCoreTypes => "dcmitype",
            KnownNamespace::WordprocessingMl => "w",
            KnownNamespace::SpreadsheetMl => "sml",
            KnownNamespace::PresentationMl => "p",
            KnownNamespace::DrawingMl => "a",
            KnownNamespace::DrawingMlChart => "c",
            KnownNamespace::DrawingMlChartDrawing => "cdr",
            KnownNamespace::DrawingMlDiagram => "dgm",
            KnownNamespace::DrawingMlLockedCanvas => "lc",
            KnownNamespace::DrawingMlPicture => "pic",
            KnownNamespace::DrawingMlSpreadsheetDrawing => "xdr",
            KnownNamespace::DrawingMlWordprocessingDrawing => "wp",
            KnownNamespace::Relationships => "r",
            KnownNamespace::Math => "m",
            KnownNamespace::SharedTypes => "s",
            KnownNamespace::CustomXml => "ds",
            KnownNamespace::SchemaLibrary => "sl",
            KnownNamespace::Bibliography => "b",
            KnownNamespace::Characteristics => "ch",
            KnownNamespace::ExtendedProperties => "ep",
            KnownNamespace::CustomProperties => "cust",
            KnownNamespace::VariantTypes => "vt",
            KnownNamespace::Vml => "v",
            KnownNamespace::VmlOffice => "o",
            KnownNamespace::VmlWord => "w10",
            KnownNamespace::VmlExcel => "x",
            KnownNamespace::VmlPowerPoint => "pvml",
            KnownNamespace::Wne => "wne",
            KnownNamespace::W14 => "w14",
            KnownNamespace::W15 => "w15",
            KnownNamespace::W16Se => "w16se",
            KnownNamespace::W16Cid => "w16cid",
            KnownNamespace::W16 => "w16",
            KnownNamespace::W16Cex => "w16cex",
            KnownNamespace::W16SdtDh => "w16sdtdh",
            KnownNamespace::W16Du => "w16du",
            KnownNamespace::W16SdtFl => "w16sdtfl",
            KnownNamespace::W16Cei => "w16cei",
            KnownNamespace::Wp14 => "wp14",
            KnownNamespace::Wp15 => "wp15",
            KnownNamespace::Wpc => "wpc",
            KnownNamespace::Wpg => "wpg",
            KnownNamespace::Wpi => "wpi",
            KnownNamespace::Wps => "wps",
            KnownNamespace::A14 => "a14",
            KnownNamespace::A15 => "a15",
            KnownNamespace::A16 => "a16",
            KnownNamespace::A1611 => "a1611",
            KnownNamespace::Aink => "aink",
            KnownNamespace::Am3d => "am3d",
            KnownNamespace::Asvg => "asvg",
            KnownNamespace::Adec => "adec",
            KnownNamespace::Pic14 => "pic14",
            KnownNamespace::C14 => "c14",
            KnownNamespace::C15 => "c15",
            KnownNamespace::C16 => "c16",
            KnownNamespace::C16R2 => "c16r2",
            KnownNamespace::C16R3 => "c16r3",
            KnownNamespace::Cx => "cx",
            KnownNamespace::Cx1 => "cx1",
            KnownNamespace::Cx2 => "cx2",
            KnownNamespace::Cx3 => "cx3",
            KnownNamespace::Cx4 => "cx4",
            KnownNamespace::Cx5 => "cx5",
            KnownNamespace::Cx6 => "cx6",
            KnownNamespace::Cx7 => "cx7",
            KnownNamespace::Cx8 => "cx8",
            KnownNamespace::Dgm14 => "dgm14",
            KnownNamespace::Dsp => "dsp",
            KnownNamespace::Thm15 => "thm15",
        }
    }

    /// The known namespace whose name `uri` is, if any. Namespace names are compared exactly, as Namespaces in XML requires.
    pub fn lookup(uri: &str) -> Option<KnownUri> {
        let (namespace, strict) = match uri {
            "http://www.w3.org/XML/1998/namespace" => (KnownNamespace::Xml, false),
            "http://www.w3.org/2000/xmlns/" => (KnownNamespace::Xmlns, false),
            "http://www.w3.org/2001/XMLSchema" => (KnownNamespace::XmlSchema, false),
            "http://www.w3.org/2001/XMLSchema-instance" => {
                (KnownNamespace::XmlSchemaInstance, false)
            }
            "http://schemas.openxmlformats.org/markup-compatibility/2006" => {
                (KnownNamespace::MarkupCompatibility, false)
            }
            "http://schemas.openxmlformats.org/package/2006/content-types" => {
                (KnownNamespace::ContentTypes, false)
            }
            "http://schemas.openxmlformats.org/package/2006/relationships" => {
                (KnownNamespace::PackageRelationships, false)
            }
            "http://schemas.openxmlformats.org/package/2006/metadata/core-properties" => {
                (KnownNamespace::CoreProperties, false)
            }
            "http://schemas.openxmlformats.org/package/2006/digital-signature" => {
                (KnownNamespace::DigitalSignature, false)
            }
            "http://purl.org/dc/elements/1.1/" => (KnownNamespace::DublinCore, false),
            "http://purl.org/dc/terms/" => (KnownNamespace::DublinCoreTerms, false),
            "http://purl.org/dc/dcmitype/" => (KnownNamespace::DublinCoreTypes, false),
            "http://schemas.openxmlformats.org/wordprocessingml/2006/main" => {
                (KnownNamespace::WordprocessingMl, false)
            }
            "http://schemas.openxmlformats.org/spreadsheetml/2006/main" => {
                (KnownNamespace::SpreadsheetMl, false)
            }
            "http://schemas.openxmlformats.org/presentationml/2006/main" => {
                (KnownNamespace::PresentationMl, false)
            }
            "http://schemas.openxmlformats.org/drawingml/2006/main" => {
                (KnownNamespace::DrawingMl, false)
            }
            "http://schemas.openxmlformats.org/drawingml/2006/chart" => {
                (KnownNamespace::DrawingMlChart, false)
            }
            "http://schemas.openxmlformats.org/drawingml/2006/chartDrawing" => {
                (KnownNamespace::DrawingMlChartDrawing, false)
            }
            "http://schemas.openxmlformats.org/drawingml/2006/diagram" => {
                (KnownNamespace::DrawingMlDiagram, false)
            }
            "http://schemas.openxmlformats.org/drawingml/2006/lockedCanvas" => {
                (KnownNamespace::DrawingMlLockedCanvas, false)
            }
            "http://schemas.openxmlformats.org/drawingml/2006/picture" => {
                (KnownNamespace::DrawingMlPicture, false)
            }
            "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" => {
                (KnownNamespace::DrawingMlSpreadsheetDrawing, false)
            }
            "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" => {
                (KnownNamespace::DrawingMlWordprocessingDrawing, false)
            }
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships" => {
                (KnownNamespace::Relationships, false)
            }
            "http://schemas.openxmlformats.org/officeDocument/2006/math" => {
                (KnownNamespace::Math, false)
            }
            "http://schemas.openxmlformats.org/officeDocument/2006/sharedTypes" => {
                (KnownNamespace::SharedTypes, false)
            }
            "http://schemas.openxmlformats.org/officeDocument/2006/customXml" => {
                (KnownNamespace::CustomXml, false)
            }
            "http://schemas.openxmlformats.org/schemaLibrary/2006/main" => {
                (KnownNamespace::SchemaLibrary, false)
            }
            "http://schemas.openxmlformats.org/officeDocument/2006/bibliography" => {
                (KnownNamespace::Bibliography, false)
            }
            "http://schemas.openxmlformats.org/officeDocument/2006/characteristics" => {
                (KnownNamespace::Characteristics, false)
            }
            "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" => {
                (KnownNamespace::ExtendedProperties, false)
            }
            "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" => {
                (KnownNamespace::CustomProperties, false)
            }
            "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes" => {
                (KnownNamespace::VariantTypes, false)
            }
            "urn:schemas-microsoft-com:vml" => (KnownNamespace::Vml, false),
            "urn:schemas-microsoft-com:office:office" => (KnownNamespace::VmlOffice, false),
            "urn:schemas-microsoft-com:office:word" => (KnownNamespace::VmlWord, false),
            "urn:schemas-microsoft-com:office:excel" => (KnownNamespace::VmlExcel, false),
            "urn:schemas-microsoft-com:office:powerpoint" => (KnownNamespace::VmlPowerPoint, false),
            "http://schemas.microsoft.com/office/word/2006/wordml" => (KnownNamespace::Wne, false),
            "http://schemas.microsoft.com/office/word/2010/wordml" => (KnownNamespace::W14, false),
            "http://schemas.microsoft.com/office/word/2012/wordml" => (KnownNamespace::W15, false),
            "http://schemas.microsoft.com/office/word/2015/wordml/symex" => {
                (KnownNamespace::W16Se, false)
            }
            "http://schemas.microsoft.com/office/word/2016/wordml/cid" => {
                (KnownNamespace::W16Cid, false)
            }
            "http://schemas.microsoft.com/office/word/2018/wordml" => (KnownNamespace::W16, false),
            "http://schemas.microsoft.com/office/word/2018/wordml/cex" => {
                (KnownNamespace::W16Cex, false)
            }
            "http://schemas.microsoft.com/office/word/2020/wordml/sdtdatahash" => {
                (KnownNamespace::W16SdtDh, false)
            }
            "http://schemas.microsoft.com/office/word/2023/wordml/word16du" => {
                (KnownNamespace::W16Du, false)
            }
            "http://schemas.microsoft.com/office/word/2024/wordml/sdtformatlock" => {
                (KnownNamespace::W16SdtFl, false)
            }
            "http://schemas.microsoft.com/office/word/2026/wordml/cei" => {
                (KnownNamespace::W16Cei, false)
            }
            "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing" => {
                (KnownNamespace::Wp14, false)
            }
            "http://schemas.microsoft.com/office/word/2012/wordprocessingDrawing" => {
                (KnownNamespace::Wp15, false)
            }
            "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas" => {
                (KnownNamespace::Wpc, false)
            }
            "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" => {
                (KnownNamespace::Wpg, false)
            }
            "http://schemas.microsoft.com/office/word/2010/wordprocessingInk" => {
                (KnownNamespace::Wpi, false)
            }
            "http://schemas.microsoft.com/office/word/2010/wordprocessingShape" => {
                (KnownNamespace::Wps, false)
            }
            "http://schemas.microsoft.com/office/drawing/2010/main" => (KnownNamespace::A14, false),
            "http://schemas.microsoft.com/office/drawing/2012/main" => (KnownNamespace::A15, false),
            "http://schemas.microsoft.com/office/drawing/2014/main" => (KnownNamespace::A16, false),
            "http://schemas.microsoft.com/office/drawing/2016/11/main" => {
                (KnownNamespace::A1611, false)
            }
            "http://schemas.microsoft.com/office/drawing/2016/ink" => (KnownNamespace::Aink, false),
            "http://schemas.microsoft.com/office/drawing/2017/model3d" => {
                (KnownNamespace::Am3d, false)
            }
            "http://schemas.microsoft.com/office/drawing/2016/SVG/main" => {
                (KnownNamespace::Asvg, false)
            }
            "http://schemas.microsoft.com/office/drawing/2017/decorative" => {
                (KnownNamespace::Adec, false)
            }
            "http://schemas.microsoft.com/office/drawing/2010/picture" => {
                (KnownNamespace::Pic14, false)
            }
            "http://schemas.microsoft.com/office/drawing/2007/8/2/chart" => {
                (KnownNamespace::C14, false)
            }
            "http://schemas.microsoft.com/office/drawing/2012/chart" => {
                (KnownNamespace::C15, false)
            }
            "http://schemas.microsoft.com/office/drawing/2014/chart" => {
                (KnownNamespace::C16, false)
            }
            "http://schemas.microsoft.com/office/drawing/2015/06/chart" => {
                (KnownNamespace::C16R2, false)
            }
            "http://schemas.microsoft.com/office/drawing/2017/03/chart" => {
                (KnownNamespace::C16R3, false)
            }
            "http://schemas.microsoft.com/office/drawing/2014/chartex" => {
                (KnownNamespace::Cx, false)
            }
            "http://schemas.microsoft.com/office/drawing/2015/9/8/chartex" => {
                (KnownNamespace::Cx1, false)
            }
            "http://schemas.microsoft.com/office/drawing/2015/10/21/chartex" => {
                (KnownNamespace::Cx2, false)
            }
            "http://schemas.microsoft.com/office/drawing/2016/5/9/chartex" => {
                (KnownNamespace::Cx3, false)
            }
            "http://schemas.microsoft.com/office/drawing/2016/5/10/chartex" => {
                (KnownNamespace::Cx4, false)
            }
            "http://schemas.microsoft.com/office/drawing/2016/5/11/chartex" => {
                (KnownNamespace::Cx5, false)
            }
            "http://schemas.microsoft.com/office/drawing/2016/5/12/chartex" => {
                (KnownNamespace::Cx6, false)
            }
            "http://schemas.microsoft.com/office/drawing/2016/5/13/chartex" => {
                (KnownNamespace::Cx7, false)
            }
            "http://schemas.microsoft.com/office/drawing/2016/5/14/chartex" => {
                (KnownNamespace::Cx8, false)
            }
            "http://schemas.microsoft.com/office/drawing/2010/diagram" => {
                (KnownNamespace::Dgm14, false)
            }
            "http://schemas.microsoft.com/office/drawing/2008/diagram" => {
                (KnownNamespace::Dsp, false)
            }
            "http://schemas.microsoft.com/office/thememl/2012/main" => {
                (KnownNamespace::Thm15, false)
            }
            "http://purl.oclc.org/ooxml/wordprocessingml/main" => {
                (KnownNamespace::WordprocessingMl, true)
            }
            "http://purl.oclc.org/ooxml/spreadsheetml/main" => {
                (KnownNamespace::SpreadsheetMl, true)
            }
            "http://purl.oclc.org/ooxml/presentationml/main" => {
                (KnownNamespace::PresentationMl, true)
            }
            "http://purl.oclc.org/ooxml/drawingml/main" => (KnownNamespace::DrawingMl, true),
            "http://purl.oclc.org/ooxml/drawingml/chart" => (KnownNamespace::DrawingMlChart, true),
            "http://purl.oclc.org/ooxml/drawingml/chartDrawing" => {
                (KnownNamespace::DrawingMlChartDrawing, true)
            }
            "http://purl.oclc.org/ooxml/drawingml/diagram" => {
                (KnownNamespace::DrawingMlDiagram, true)
            }
            "http://purl.oclc.org/ooxml/drawingml/lockedCanvas" => {
                (KnownNamespace::DrawingMlLockedCanvas, true)
            }
            "http://purl.oclc.org/ooxml/drawingml/picture" => {
                (KnownNamespace::DrawingMlPicture, true)
            }
            "http://purl.oclc.org/ooxml/drawingml/spreadsheetDrawing" => {
                (KnownNamespace::DrawingMlSpreadsheetDrawing, true)
            }
            "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing" => {
                (KnownNamespace::DrawingMlWordprocessingDrawing, true)
            }
            "http://purl.oclc.org/ooxml/officeDocument/relationships" => {
                (KnownNamespace::Relationships, true)
            }
            "http://purl.oclc.org/ooxml/officeDocument/math" => (KnownNamespace::Math, true),
            "http://purl.oclc.org/ooxml/officeDocument/sharedTypes" => {
                (KnownNamespace::SharedTypes, true)
            }
            "http://purl.oclc.org/ooxml/officeDocument/customXml" => {
                (KnownNamespace::CustomXml, true)
            }
            "http://purl.oclc.org/ooxml/schemaLibrary/main" => {
                (KnownNamespace::SchemaLibrary, true)
            }
            "http://purl.oclc.org/ooxml/officeDocument/bibliography" => {
                (KnownNamespace::Bibliography, true)
            }
            "http://purl.oclc.org/ooxml/officeDocument/characteristics" => {
                (KnownNamespace::Characteristics, true)
            }
            "http://purl.oclc.org/ooxml/officeDocument/extendedProperties" => {
                (KnownNamespace::ExtendedProperties, true)
            }
            "http://purl.oclc.org/ooxml/officeDocument/customProperties" => {
                (KnownNamespace::CustomProperties, true)
            }
            "http://purl.oclc.org/ooxml/officeDocument/docPropsVTypes" => {
                (KnownNamespace::VariantTypes, true)
            }
            _ => return None,
        };
        Some(KnownUri { namespace, strict })
    }
}

/// The namespace of an element or attribute name, as the reader resolved it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Namespace<'r> {
    uri: &'r str,
    known: Option<KnownUri>,
}

impl<'r> Namespace<'r> {
    /// The namespace for `uri`, looked up in the table.
    pub(crate) fn new(uri: &'r str) -> Namespace<'r> {
        Namespace {
            uri,
            known: KnownNamespace::lookup(uri),
        }
    }

    /// The namespace name, exactly as declared (with references resolved).
    pub fn uri(&self) -> &'r str {
        self.uri
    }

    /// The known namespace, if the name is in the table; both names of an ECMA-376 vocabulary give the same one.
    pub fn known(&self) -> Option<KnownNamespace> {
        self.known.map(|known| known.namespace)
    }

    /// Whether this is the Strict name of an ECMA-376 vocabulary.
    pub fn is_strict(&self) -> bool {
        self.known.is_some_and(|known| known.strict)
    }

    /// Whether this is `namespace`, by either of its names.
    pub fn is(&self, namespace: KnownNamespace) -> bool {
        self.known() == Some(namespace)
    }
}

/// A namespace as a configuration names it: no namespace at all, a known namespace by either of its names, or any other namespace by its exact name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NamespaceName {
    /// Names without a namespace.
    None,
    /// A known namespace; it matches the Strict and the Transitional name alike.
    Known(KnownNamespace),
    /// A namespace that is not in the table, by its exact name.
    Uri(String),
}

impl NamespaceName {
    /// The name for `uri`: [`NamespaceName::Known`] for a name in the table, [`NamespaceName::Uri`] for any other.
    pub fn from_uri(uri: &str) -> NamespaceName {
        match KnownNamespace::lookup(uri) {
            Some(known) => NamespaceName::Known(known.namespace),
            None => NamespaceName::Uri(uri.to_owned()),
        }
    }

    /// Whether a name in `namespace` (`None` for no namespace) is in this namespace.
    pub fn matches(&self, namespace: Option<Namespace<'_>>) -> bool {
        match (self, namespace) {
            (NamespaceName::None, None) => true,
            (NamespaceName::Known(known), Some(namespace)) => namespace.is(*known),
            (NamespaceName::Uri(uri), Some(namespace)) => namespace.uri() == uri,
            _ => false,
        }
    }
}

impl From<KnownNamespace> for NamespaceName {
    fn from(namespace: KnownNamespace) -> Self {
        NamespaceName::Known(namespace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The 21 pairs of names that ECMA-376's schemas give their vocabularies: the `targetNamespace` of each schema file of the Transitional schemas (Part 4) and of the Strict schemas (Part 1) with the same file name.
    const SCHEMA_PAIRS: [(&str, &str); 21] = [
        (
            "http://schemas.openxmlformats.org/drawingml/2006/chart",
            "http://purl.oclc.org/ooxml/drawingml/chart",
        ),
        (
            "http://schemas.openxmlformats.org/drawingml/2006/chartDrawing",
            "http://purl.oclc.org/ooxml/drawingml/chartDrawing",
        ),
        (
            "http://schemas.openxmlformats.org/drawingml/2006/diagram",
            "http://purl.oclc.org/ooxml/drawingml/diagram",
        ),
        (
            "http://schemas.openxmlformats.org/drawingml/2006/lockedCanvas",
            "http://purl.oclc.org/ooxml/drawingml/lockedCanvas",
        ),
        (
            "http://schemas.openxmlformats.org/drawingml/2006/main",
            "http://purl.oclc.org/ooxml/drawingml/main",
        ),
        (
            "http://schemas.openxmlformats.org/drawingml/2006/picture",
            "http://purl.oclc.org/ooxml/drawingml/picture",
        ),
        (
            "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing",
            "http://purl.oclc.org/ooxml/drawingml/spreadsheetDrawing",
        ),
        (
            "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
            "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing",
        ),
        (
            "http://schemas.openxmlformats.org/presentationml/2006/main",
            "http://purl.oclc.org/ooxml/presentationml/main",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/characteristics",
            "http://purl.oclc.org/ooxml/officeDocument/characteristics",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/bibliography",
            "http://purl.oclc.org/ooxml/officeDocument/bibliography",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/sharedTypes",
            "http://purl.oclc.org/ooxml/officeDocument/sharedTypes",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/customXml",
            "http://purl.oclc.org/ooxml/officeDocument/customXml",
        ),
        (
            "http://schemas.openxmlformats.org/schemaLibrary/2006/main",
            "http://purl.oclc.org/ooxml/schemaLibrary/main",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties",
            "http://purl.oclc.org/ooxml/officeDocument/customProperties",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties",
            "http://purl.oclc.org/ooxml/officeDocument/extendedProperties",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes",
            "http://purl.oclc.org/ooxml/officeDocument/docPropsVTypes",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/math",
            "http://purl.oclc.org/ooxml/officeDocument/math",
        ),
        (
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
            "http://purl.oclc.org/ooxml/officeDocument/relationships",
        ),
        (
            "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
            "http://purl.oclc.org/ooxml/spreadsheetml/main",
        ),
        (
            "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
            "http://purl.oclc.org/ooxml/wordprocessingml/main",
        ),
    ];

    #[test]
    fn every_known_namespace_finds_itself_by_each_of_its_names() {
        assert_eq!(KnownNamespace::ALL.len(), 81);
        for &namespace in KnownNamespace::ALL {
            assert_eq!(
                KnownNamespace::lookup(namespace.uri()),
                Some(KnownUri {
                    namespace,
                    strict: false
                }),
                "{namespace:?}"
            );
            if let Some(strict) = namespace.strict_uri() {
                assert_eq!(
                    KnownNamespace::lookup(strict),
                    Some(KnownUri {
                        namespace,
                        strict: true
                    }),
                    "{namespace:?}"
                );
            }
        }
    }

    #[test]
    fn names_and_prefixes_are_unique() {
        let mut uris = BTreeSet::new();
        let mut prefixes = BTreeSet::new();
        for &namespace in KnownNamespace::ALL {
            assert!(uris.insert(namespace.uri()), "{namespace:?}");
            if let Some(strict) = namespace.strict_uri() {
                assert!(uris.insert(strict), "{namespace:?}");
            }
            assert!(crate::chars::is_ncname(namespace.prefix()), "{namespace:?}");
            assert!(prefixes.insert(namespace.prefix()), "{namespace:?}");
        }
    }

    #[test]
    fn maps_the_strict_and_transitional_names_of_the_schemas() {
        let mut paired = 0;
        for (transitional, strict) in SCHEMA_PAIRS {
            let transitional = KnownNamespace::lookup(transitional).unwrap();
            let strict = KnownNamespace::lookup(strict).unwrap();
            assert_eq!(transitional.namespace, strict.namespace);
            assert!(!transitional.strict && strict.strict);
            paired += 1;
        }
        assert_eq!(paired, 21);
        let with_strict_names = KnownNamespace::ALL
            .iter()
            .filter(|namespace| namespace.strict_uri().is_some())
            .count();
        assert_eq!(with_strict_names, 21);
        assert_eq!(
            KnownNamespace::WordprocessingMl.strict_uri(),
            Some("http://purl.oclc.org/ooxml/wordprocessingml/main")
        );
        assert_eq!(KnownNamespace::MarkupCompatibility.strict_uri(), None);
    }

    #[test]
    fn compares_names_exactly() {
        assert_eq!(KnownNamespace::lookup("http://example.com/"), None);
        assert_eq!(
            KnownNamespace::lookup("http://schemas.openxmlformats.org/wordprocessingml/2006/MAIN"),
            None
        );
        assert_eq!(
            KnownNamespace::lookup("http://schemas.openxmlformats.org/wordprocessingml/2006/main/"),
            None
        );
        assert_eq!(KnownNamespace::lookup(""), None);
    }

    #[test]
    fn namespace_names_match_either_name_of_a_known_namespace() {
        let transitional =
            Namespace::new("http://schemas.openxmlformats.org/wordprocessingml/2006/main");
        let strict = Namespace::new("http://purl.oclc.org/ooxml/wordprocessingml/main");
        let other = Namespace::new("urn:example:other");
        assert!(transitional.is(KnownNamespace::WordprocessingMl) && !transitional.is_strict());
        assert!(strict.is(KnownNamespace::WordprocessingMl) && strict.is_strict());
        assert_eq!(other.known(), None);
        let word = NamespaceName::from(KnownNamespace::WordprocessingMl);
        assert!(word.matches(Some(transitional)));
        assert!(word.matches(Some(strict)));
        assert!(!word.matches(Some(other)));
        assert!(!word.matches(None));
        assert_eq!(
            NamespaceName::from_uri("http://purl.oclc.org/ooxml/wordprocessingml/main"),
            word
        );
        let by_name = NamespaceName::from_uri("urn:example:other");
        assert_eq!(by_name, NamespaceName::Uri("urn:example:other".to_owned()));
        assert!(by_name.matches(Some(other)));
        assert!(!by_name.matches(Some(transitional)));
        assert!(!by_name.matches(None));
        assert!(NamespaceName::None.matches(None));
        assert!(!NamespaceName::None.matches(Some(other)));
    }
}
