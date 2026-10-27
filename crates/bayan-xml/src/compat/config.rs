//! The configuration of Markup Compatibility processing: which namespaces the application understands, and which elements are its extension elements.

use std::collections::{BTreeMap, BTreeSet};

use crate::namespace::{KnownNamespace, Namespace, NamespaceName};

/// The namespaces whose schemas define an `extLst` element: the six vocabularies of ECMA-376 Part 1 that have one (Part 1 §10 names `extLst` the application-defined extension element of Office Open XML), and Microsoft's extension namespaces of the table whose schemas define one ([MS-ODRAWXML] and [MS-DOCX]).
const EXTENSION_LISTS: [KnownNamespace; 18] = [
    KnownNamespace::DrawingMl,
    KnownNamespace::DrawingMlChart,
    KnownNamespace::DrawingMlDiagram,
    KnownNamespace::DrawingMlWordprocessingDrawing,
    KnownNamespace::PresentationMl,
    KnownNamespace::SpreadsheetMl,
    KnownNamespace::A14,
    KnownNamespace::Am3d,
    KnownNamespace::Cx,
    KnownNamespace::Dsp,
    KnownNamespace::Pic14,
    KnownNamespace::Thm15,
    KnownNamespace::W14,
    KnownNamespace::W16Cei,
    KnownNamespace::W16Cex,
    KnownNamespace::Wpc,
    KnownNamespace::Wpg,
    KnownNamespace::Wps,
];

/// What Markup Compatibility processing (ECMA-376 Part 3 §9) is configured with: the namespaces the application understands (Part 3 calls them its application configuration) and the expanded names of its application-defined extension elements (the markup configuration).
///
/// Names in no namespace are understood only if [`NamespaceName::None`] is: Office Open XML uses them for most attributes (`w:val` aside), so its applications understand them. A known namespace is understood, or names an extension element, by both its Strict and its Transitional name.
///
/// ```
/// use bayan_xml::{KnownNamespace, McConfig, NamespaceName};
///
/// let config = McConfig::new()
///     .understand(NamespaceName::None)
///     .understand(KnownNamespace::WordprocessingMl)
///     .understand(KnownNamespace::W14)
///     .with_ooxml_extension_elements();
/// assert!(config.understands_name(&NamespaceName::from_uri("http://purl.oclc.org/ooxml/wordprocessingml/main")));
/// ```
#[derive(Clone, Debug, Default)]
pub struct McConfig {
    no_namespace: bool,
    known: BTreeSet<KnownNamespace>,
    uris: BTreeSet<String>,
    extensions_without_namespace: BTreeSet<String>,
    known_extensions: BTreeMap<KnownNamespace, BTreeSet<String>>,
    uri_extensions: BTreeMap<String, BTreeSet<String>>,
}

/// A namespace name with a known namespace given by any of its names turned into [`NamespaceName::Known`], so that lookups by a reader's [`Namespace`] find it.
fn normalized(namespace: NamespaceName) -> NamespaceName {
    match namespace {
        NamespaceName::Uri(uri) => NamespaceName::from_uri(&uri),
        other => other,
    }
}

impl McConfig {
    /// A configuration that understands nothing and has no extension elements.
    pub fn new() -> McConfig {
        McConfig::default()
    }

    /// The configuration, also understanding `namespace`.
    #[must_use]
    pub fn understand(mut self, namespace: impl Into<NamespaceName>) -> McConfig {
        match normalized(namespace.into()) {
            NamespaceName::None => self.no_namespace = true,
            NamespaceName::Known(known) => {
                self.known.insert(known);
            }
            NamespaceName::Uri(uri) => {
                self.uris.insert(uri);
            }
        }
        self
    }

    /// The configuration, with the element `local` in `namespace` as an application-defined extension element (Part 3 §8): Markup Compatibility leaves it, and everything inside it, as it is.
    #[must_use]
    pub fn extension_element(
        mut self,
        namespace: impl Into<NamespaceName>,
        local: &str,
    ) -> McConfig {
        match normalized(namespace.into()) {
            NamespaceName::None => {
                self.extensions_without_namespace.insert(local.to_owned());
            }
            NamespaceName::Known(known) => {
                self.known_extensions
                    .entry(known)
                    .or_default()
                    .insert(local.to_owned());
            }
            NamespaceName::Uri(uri) => {
                self.uri_extensions
                    .entry(uri)
                    .or_default()
                    .insert(local.to_owned());
            }
        }
        self
    }

    /// The configuration, with the extension lists of Office Open XML as extension elements: `extLst` in every namespace of the table whose schema defines one (ECMA-376 Part 1 §10 and its schemas; [MS-ODRAWXML]; [MS-DOCX]). Extension lists of namespaces outside the table are added with [`McConfig::extension_element`].
    #[must_use]
    pub fn with_ooxml_extension_elements(self) -> McConfig {
        EXTENSION_LISTS.into_iter().fold(self, |config, namespace| {
            config.extension_element(namespace, "extLst")
        })
    }

    /// Whether names in `namespace` (`None` for no namespace) are understood.
    pub fn understands(&self, namespace: Option<Namespace<'_>>) -> bool {
        match namespace {
            None => self.no_namespace,
            Some(namespace) => match namespace.known() {
                Some(known) => self.known.contains(&known),
                None => self.uris.contains(namespace.uri()),
            },
        }
    }

    /// Whether names in `namespace` are understood.
    pub fn understands_name(&self, namespace: &NamespaceName) -> bool {
        match normalized(namespace.clone()) {
            NamespaceName::None => self.no_namespace,
            NamespaceName::Known(known) => self.known.contains(&known),
            NamespaceName::Uri(uri) => self.uris.contains(&uri),
        }
    }

    /// Whether the element `local` in `namespace` is an application-defined extension element.
    pub fn is_extension_element(&self, namespace: Option<Namespace<'_>>, local: &str) -> bool {
        match namespace {
            None => self.extensions_without_namespace.contains(local),
            Some(namespace) => match namespace.known() {
                Some(known) => self
                    .known_extensions
                    .get(&known)
                    .is_some_and(|locals| locals.contains(local)),
                None => self
                    .uri_extensions
                    .get(namespace.uri())
                    .is_some_and(|locals| locals.contains(local)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn understands_known_namespaces_by_both_names() {
        let config = McConfig::new()
            .understand(KnownNamespace::WordprocessingMl)
            .understand(NamespaceName::from_uri("urn:a"));
        for uri in [
            "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
            "http://purl.oclc.org/ooxml/wordprocessingml/main",
            "urn:a",
        ] {
            assert!(config.understands(Some(Namespace::new(uri))), "{uri}");
        }
        assert!(!config.understands(Some(Namespace::new("urn:b"))));
        assert!(!config.understands(None));
        assert!(
            McConfig::new()
                .understand(NamespaceName::None)
                .understands(None)
        );
        // A known namespace given by its name as a string is the known namespace.
        let config = McConfig::new().understand(NamespaceName::Uri(
            "http://purl.oclc.org/ooxml/drawingml/main".to_owned(),
        ));
        assert!(config.understands(Some(Namespace::new(
            "http://schemas.openxmlformats.org/drawingml/2006/main"
        ))));
    }

    #[test]
    fn knows_the_extension_lists_of_office_open_xml() {
        let config = McConfig::new().with_ooxml_extension_elements();
        for uri in [
            "http://schemas.openxmlformats.org/drawingml/2006/main",
            "http://purl.oclc.org/ooxml/drawingml/main",
            "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
            "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
        ] {
            assert!(
                config.is_extension_element(Some(Namespace::new(uri)), "extLst"),
                "{uri}"
            );
            assert!(
                !config.is_extension_element(Some(Namespace::new(uri)), "ext"),
                "{uri}"
            );
        }
        assert!(!config.is_extension_element(
            Some(Namespace::new(
                "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            )),
            "extLst"
        ));
        assert!(!config.is_extension_element(None, "extLst"));
        let config = McConfig::new()
            .extension_element(NamespaceName::None, "island")
            .extension_element(NamespaceName::from_uri("urn:a"), "x");
        assert!(config.is_extension_element(None, "island"));
        assert!(config.is_extension_element(Some(Namespace::new("urn:a")), "x"));
        assert!(!config.is_extension_element(Some(Namespace::new("urn:a")), "island"));
    }
}
