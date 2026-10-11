//! The OOXML vocabulary the tagger knows: XML namespaces and relationship types, in both their Transitional and their Strict spelling (ECMA-376 Part 1, Part 4 and the Microsoft extensions that Word writes, as documented in [MS-DOCX] and [MS-ODRAWXML]).
//!
//! A namespace or relationship type that is not listed here is "unknown": BayanDocs would keep such content unread and write it back (the coverage matrix's "Unknown parts and extensions preserved").

/// An XML namespace the tagger knows. Transitional and Strict spellings of the same vocabulary map to the same value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ns {
    /// WordprocessingML (`w`).
    W,
    /// Word 2010 extensions (`w14`).
    W14,
    /// Word 2012 extensions (`w15`).
    W15,
    /// Word 2015 and later extensions (`w16`, `w16cex`, `w16cid`, `w16du`, `w16se`, `w16sdtdh`, `w16sdtfl`).
    W16,
    /// Word 2006 extensions for settings and key maps (`wne`).
    Wne,
    /// VML extensions for Word (`w10`).
    W10,
    /// WordprocessingML drawing placement (`wp`).
    Wp,
    /// Word 2010 drawing placement extensions (`wp14`).
    Wp14,
    /// Word 2010 shapes (`wps`).
    Wps,
    /// Word 2010 groups (`wpg`).
    Wpg,
    /// Word 2010 drawing canvases (`wpc`).
    Wpc,
    /// Word 2010 ink (`wpi`).
    Wpi,
    /// DrawingML main (`a`).
    A,
    /// DrawingML extensions of 2010 and later (`a14`, `a15`, `a16`, `adec`, `ahyp`, `aclsh`, `asvg`).
    AExtension,
    /// DrawingML pictures (`pic`).
    Pic,
    /// DrawingML locked canvases (`lc`).
    Lc,
    /// DrawingML charts and their extensions (`c`, `c14`, `c15`, `c16`, `c16r2`, `c16r3`, `cdr`).
    C,
    /// Chart styles and colors (`cs`).
    Cs,
    /// Office 2016 chart types (`cx`, `cx1`…`cx8`).
    Cx,
    /// DrawingML diagrams, SmartArt (`dgm`, `dgm14`).
    Dgm,
    /// SmartArt drawings cached by Word (`dsp`).
    Dsp,
    /// 3D models (`am3d`).
    Am3d,
    /// Ink (`aink`).
    Aink,
    /// Office Math (`m`).
    M,
    /// Relationship references (`r`).
    R,
    /// Markup Compatibility (`mc`).
    Mc,
    /// VML (`v`).
    V,
    /// VML Office extensions (`o`).
    O,
    /// Theme extensions (`thm15`).
    Thm15,
    /// Office 2019 extension lists (`oel`).
    Oel,
    /// OPC content types.
    ContentTypes,
    /// OPC relationships.
    Relationships,
    /// Core properties and the Dublin Core vocabularies they use.
    CoreProperties,
    /// Extended (application) properties.
    ExtendedProperties,
    /// Custom properties.
    CustomProperties,
    /// Variant types used by the properties.
    VariantTypes,
    /// Custom XML part properties (`ds`).
    CustomXml,
    /// Schema library (`sl`).
    SchemaLibrary,
    /// XML Schema instance (`xsi`).
    Xsi,
    /// The `xml` prefix.
    Xml,
}

/// The vocabulary of a namespace URI, and whether the URI is the Strict spelling; `None` for a namespace the tagger does not know.
#[must_use]
pub fn namespace(uri: &str) -> Option<(Ns, bool)> {
    let transitional = match uri {
        "http://schemas.openxmlformats.org/wordprocessingml/2006/main" => Ns::W,
        "http://schemas.microsoft.com/office/word/2010/wordml" => Ns::W14,
        "http://schemas.microsoft.com/office/word/2012/wordml" => Ns::W15,
        "http://schemas.microsoft.com/office/word/2018/wordml"
        | "http://schemas.microsoft.com/office/word/2018/wordml/cex"
        | "http://schemas.microsoft.com/office/word/2016/wordml/cid"
        | "http://schemas.microsoft.com/office/word/2023/wordml/word16du"
        | "http://schemas.microsoft.com/office/word/2015/wordml/symex"
        | "http://schemas.microsoft.com/office/word/2020/wordml/sdtdatahash"
        | "http://schemas.microsoft.com/office/word/2024/wordml/sdtformatlock" => Ns::W16,
        "http://schemas.microsoft.com/office/word/2006/wordml" => Ns::Wne,
        "urn:schemas-microsoft-com:office:word" => Ns::W10,
        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" => Ns::Wp,
        "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing" => Ns::Wp14,
        "http://schemas.microsoft.com/office/word/2010/wordprocessingShape" => Ns::Wps,
        "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" => Ns::Wpg,
        "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas" => Ns::Wpc,
        "http://schemas.microsoft.com/office/word/2010/wordprocessingInk" => Ns::Wpi,
        "http://schemas.openxmlformats.org/drawingml/2006/main" => Ns::A,
        "http://schemas.microsoft.com/office/drawing/2010/main"
        | "http://schemas.microsoft.com/office/drawing/2012/main"
        | "http://schemas.microsoft.com/office/drawing/2014/main"
        | "http://schemas.microsoft.com/office/drawing/2017/decorative"
        | "http://schemas.microsoft.com/office/drawing/2018/hyperlinkcolor"
        | "http://schemas.microsoft.com/office/drawing/2016/11/main"
        | "http://schemas.microsoft.com/office/drawing/2016/SVG/main" => Ns::AExtension,
        "http://schemas.openxmlformats.org/drawingml/2006/picture" => Ns::Pic,
        "http://schemas.openxmlformats.org/drawingml/2006/lockedCanvas" => Ns::Lc,
        "http://schemas.openxmlformats.org/drawingml/2006/chart"
        | "http://schemas.openxmlformats.org/drawingml/2006/chartDrawing"
        | "http://schemas.microsoft.com/office/drawing/2007/8/2/chart"
        | "http://schemas.microsoft.com/office/drawing/2012/chart"
        | "http://schemas.microsoft.com/office/drawing/2014/chart"
        | "http://schemas.microsoft.com/office/drawing/2015/06/chart"
        | "http://schemas.microsoft.com/office/drawing/2017/03/chart" => Ns::C,
        "http://schemas.microsoft.com/office/drawing/2012/chartStyle" => Ns::Cs,
        "http://schemas.microsoft.com/office/drawing/2014/chartex"
        | "http://schemas.microsoft.com/office/drawing/2015/9/8/chartex"
        | "http://schemas.microsoft.com/office/drawing/2015/10/21/chartex"
        | "http://schemas.microsoft.com/office/drawing/2016/5/9/chartex"
        | "http://schemas.microsoft.com/office/drawing/2016/5/10/chartex"
        | "http://schemas.microsoft.com/office/drawing/2016/5/11/chartex"
        | "http://schemas.microsoft.com/office/drawing/2016/5/12/chartex"
        | "http://schemas.microsoft.com/office/drawing/2016/5/13/chartex"
        | "http://schemas.microsoft.com/office/drawing/2016/5/14/chartex" => Ns::Cx,
        "http://schemas.openxmlformats.org/drawingml/2006/diagram"
        | "http://schemas.microsoft.com/office/drawing/2010/diagram" => Ns::Dgm,
        "http://schemas.microsoft.com/office/drawing/2008/diagram" => Ns::Dsp,
        "http://schemas.microsoft.com/office/drawing/2017/model3d" => Ns::Am3d,
        "http://schemas.microsoft.com/office/drawing/2016/ink" => Ns::Aink,
        "http://schemas.openxmlformats.org/officeDocument/2006/math" => Ns::M,
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships" => Ns::R,
        "http://schemas.openxmlformats.org/markup-compatibility/2006" => Ns::Mc,
        "urn:schemas-microsoft-com:vml" => Ns::V,
        "urn:schemas-microsoft-com:office:office" => Ns::O,
        "http://schemas.microsoft.com/office/thememl/2012/main" => Ns::Thm15,
        "http://schemas.microsoft.com/office/2019/extlst" => Ns::Oel,
        "http://schemas.openxmlformats.org/package/2006/content-types" => Ns::ContentTypes,
        "http://schemas.openxmlformats.org/package/2006/relationships" => Ns::Relationships,
        "http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
        | "http://purl.org/dc/elements/1.1/"
        | "http://purl.org/dc/terms/"
        | "http://purl.org/dc/dcmitype/" => Ns::CoreProperties,
        "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" => {
            Ns::ExtendedProperties
        }
        "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" => {
            Ns::CustomProperties
        }
        "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes" => Ns::VariantTypes,
        "http://schemas.openxmlformats.org/officeDocument/2006/customXml" => Ns::CustomXml,
        "http://schemas.openxmlformats.org/schemaLibrary/2006/main" => Ns::SchemaLibrary,
        "http://www.w3.org/2001/XMLSchema-instance" => Ns::Xsi,
        "http://www.w3.org/XML/1998/namespace" => Ns::Xml,
        _ => return strict_namespace(uri).map(|ns| (ns, true)),
    };
    Some((transitional, false))
}

/// The Strict spellings (ISO/IEC 29500-1 Strict, namespaces under `http://purl.oclc.org/ooxml/`).
fn strict_namespace(uri: &str) -> Option<Ns> {
    Some(match uri.strip_prefix("http://purl.oclc.org/ooxml/")? {
        "wordprocessingml/main" => Ns::W,
        "officeDocument/relationships" => Ns::R,
        "officeDocument/math" => Ns::M,
        "drawingml/main" => Ns::A,
        "drawingml/wordprocessingDrawing" => Ns::Wp,
        "drawingml/picture" => Ns::Pic,
        "drawingml/lockedCanvas" => Ns::Lc,
        "drawingml/chart" | "drawingml/chartDrawing" => Ns::C,
        "drawingml/diagram" => Ns::Dgm,
        "officeDocument/extendedProperties" => Ns::ExtendedProperties,
        "officeDocument/customProperties" => Ns::CustomProperties,
        "officeDocument/docPropsVTypes" => Ns::VariantTypes,
        "officeDocument/customXml" => Ns::CustomXml,
        "schemaLibrary/main" => Ns::SchemaLibrary,
        _ => return None,
    })
}

/// What a relationship points to, as far as tagging is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rel {
    /// The main document part of the package.
    OfficeDocument,
    /// Styles (`styles.xml`).
    Styles,
    /// Word 2010's copy of the styles with effects; not scanned, because it repeats the styles.
    StylesWithEffects,
    /// Numbering definitions.
    Numbering,
    /// Document settings.
    Settings,
    /// Web settings.
    WebSettings,
    /// The font table.
    FontTable,
    /// The theme.
    Theme,
    /// A header.
    Header,
    /// A footer.
    Footer,
    /// Footnotes.
    Footnotes,
    /// Endnotes.
    Endnotes,
    /// Comments.
    Comments,
    /// Word 2012 and later comment extensions and the people part.
    CommentExtensions,
    /// An image.
    Image,
    /// A hyperlink (always an external or in-document target).
    Hyperlink,
    /// A chart.
    Chart,
    /// A chart's user shapes, styles or colors.
    ChartExtras,
    /// An Office 2016 chart (chartEx).
    ChartEx,
    /// An embedded OLE object or package.
    OleObject,
    /// SmartArt data, layout, styles, colors or Word's drawing cache.
    Diagram,
    /// A custom XML part or its properties.
    CustomXml,
    /// The glossary document (building blocks).
    GlossaryDocument,
    /// An embedded font.
    Font,
    /// The attached template (an external reference written by Word).
    AttachedTemplate,
    /// Core properties.
    CoreProperties,
    /// Extended (application) properties.
    ExtendedProperties,
    /// Custom properties.
    CustomProperties,
    /// The package thumbnail.
    Thumbnail,
    /// Digital signatures.
    DigitalSignature,
    /// A VBA project or its data.
    Vba,
    /// A 3D model.
    Model3d,
    /// Printer settings.
    PrinterSettings,
    /// Sensitivity label metadata.
    ClassificationLabels,
    /// Any relationship type not listed above.
    Unknown,
}

/// The kind of a relationship type URI.
#[must_use]
pub fn relationship(rel_type: &str) -> Rel {
    let officedocument = rel_type
        .strip_prefix("http://schemas.openxmlformats.org/officeDocument/2006/relationships/")
        .or_else(|| {
            rel_type.strip_prefix("http://purl.oclc.org/ooxml/officeDocument/relationships/")
        });
    if let Some(name) = officedocument {
        return match name {
            "officeDocument" => Rel::OfficeDocument,
            "styles" => Rel::Styles,
            "numbering" => Rel::Numbering,
            "settings" => Rel::Settings,
            "webSettings" => Rel::WebSettings,
            "fontTable" => Rel::FontTable,
            "theme" => Rel::Theme,
            "header" => Rel::Header,
            "footer" => Rel::Footer,
            "footnotes" => Rel::Footnotes,
            "endnotes" => Rel::Endnotes,
            "comments" => Rel::Comments,
            "image" => Rel::Image,
            "hyperlink" => Rel::Hyperlink,
            "chart" => Rel::Chart,
            "chartUserShapes" => Rel::ChartExtras,
            "oleObject" | "package" => Rel::OleObject,
            "diagramData" | "diagramLayout" | "diagramQuickStyle" | "diagramColors" => Rel::Diagram,
            "customXml" | "customXmlProps" => Rel::CustomXml,
            "glossaryDocument" => Rel::GlossaryDocument,
            "font" => Rel::Font,
            "attachedTemplate" => Rel::AttachedTemplate,
            "extended-properties" | "extendedProperties" => Rel::ExtendedProperties,
            "custom-properties" | "customProperties" => Rel::CustomProperties,
            "printerSettings" => Rel::PrinterSettings,
            "metadata/core-properties" => Rel::CoreProperties,
            _ => Rel::Unknown,
        };
    }
    if let Some(name) =
        rel_type.strip_prefix("http://schemas.openxmlformats.org/package/2006/relationships/")
    {
        return match name {
            "metadata/core-properties" => Rel::CoreProperties,
            "metadata/thumbnail" => Rel::Thumbnail,
            "digital-signature/origin"
            | "digital-signature/signature"
            | "digital-signature/certificate" => Rel::DigitalSignature,
            _ => Rel::Unknown,
        };
    }
    match rel_type {
        "http://schemas.microsoft.com/office/2007/relationships/stylesWithEffects" => {
            Rel::StylesWithEffects
        }
        "http://schemas.microsoft.com/office/2011/relationships/commentsExtended"
        | "http://schemas.microsoft.com/office/2016/09/relationships/commentsIds"
        | "http://schemas.microsoft.com/office/2018/08/relationships/commentsExtensible"
        | "http://schemas.microsoft.com/office/2011/relationships/people" => Rel::CommentExtensions,
        "http://schemas.microsoft.com/office/2007/relationships/diagramDrawing" => Rel::Diagram,
        "http://schemas.microsoft.com/office/2014/relationships/chartEx" => Rel::ChartEx,
        "http://schemas.microsoft.com/office/2011/relationships/chartStyle"
        | "http://schemas.microsoft.com/office/2011/relationships/chartColorStyle" => {
            Rel::ChartExtras
        }
        "http://schemas.microsoft.com/office/2007/relationships/hdphoto" => Rel::Image,
        "http://schemas.microsoft.com/office/2006/relationships/vbaProject"
        | "http://schemas.microsoft.com/office/2006/relationships/wordVbaData" => Rel::Vba,
        "http://schemas.microsoft.com/office/2017/06/relationships/model3d" => Rel::Model3d,
        "http://schemas.microsoft.com/office/2020/02/relationships/classificationlabels" => {
            Rel::ClassificationLabels
        }
        _ => Rel::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_and_transitional_spellings_map_to_the_same_vocabulary() {
        assert_eq!(
            namespace("http://schemas.openxmlformats.org/wordprocessingml/2006/main"),
            Some((Ns::W, false))
        );
        assert_eq!(
            namespace("http://purl.oclc.org/ooxml/wordprocessingml/main"),
            Some((Ns::W, true))
        );
        assert_eq!(namespace("http://example.invalid/unknown"), None);
        assert_eq!(namespace("http://purl.oclc.org/ooxml/unknown"), None);
        assert_eq!(
            relationship("http://purl.oclc.org/ooxml/officeDocument/relationships/styles"),
            Rel::Styles
        );
        assert_eq!(
            relationship(
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/aFChunk"
            ),
            Rel::Unknown
        );
    }
}
