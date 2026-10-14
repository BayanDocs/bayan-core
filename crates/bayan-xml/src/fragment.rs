//! Lossless capture: fragments of a document that can be written back exactly as they were, in their own context or in another one.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::event::{Event, Space};
use crate::{Error, ErrorKind, Limits, Reader, Source};

/// One element's namespace declarations, in a chain that ends at the outermost element: a context is shared by everything that has it in scope, so taking it costs nothing, however many bindings are in scope.
#[derive(Debug)]
pub(crate) struct ContextNode {
    pub(crate) parent: Option<Arc<ContextNode>>,
    /// The declarations, as (prefix, namespace name): `""` as prefix for the default namespace, and `""` as name for `xmlns=""`.
    pub(crate) bindings: Box<[(Box<str>, Box<str>)]>,
}

/// The namespace bindings and the white space mode in scope at a place in a document: what an element found there inherits.
///
/// The prefix `xml` is always bound to its namespace and is never listed.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub(crate) node: Option<Arc<ContextNode>>,
    pub(crate) space: Space,
}

impl Context {
    /// The empty context: no namespace bindings, and the default white space mode. This is where a root element is.
    pub fn empty() -> Context {
        Context::default()
    }

    /// A context with `bindings`, as (prefix, namespace name) pairs: `""` as prefix for the default namespace, and `""` as name for no default namespace. The bindings are checked when the context is used ([`Reader::with_context`], [`Fragment::new`]).
    pub fn new(bindings: &[(&str, &str)], space: Space) -> Context {
        let node = (!bindings.is_empty()).then(|| {
            Arc::new(ContextNode {
                parent: None,
                bindings: bindings
                    .iter()
                    .map(|&(prefix, uri)| (Box::from(prefix), Box::from(uri)))
                    .collect(),
            })
        });
        Context { node, space }
    }

    /// The bindings in scope, as (prefix, namespace name) pairs sorted by prefix, each prefix once with its innermost binding.
    pub fn bindings(&self) -> Vec<(&str, &str)> {
        let mut found: BTreeMap<&str, &str> = BTreeMap::new();
        let mut node = self.node.as_deref();
        while let Some(current) = node {
            for (prefix, uri) in &*current.bindings {
                found.entry(prefix).or_insert(uri);
            }
            node = current.parent.as_deref();
        }
        found.into_iter().collect()
    }

    /// The namespace name bound to `prefix` (`""` for the default namespace), if any: `Some("")` when the default namespace was undeclared with `xmlns=""`.
    pub fn uri_of(&self, prefix: &str) -> Option<&str> {
        let mut node = self.node.as_deref();
        while let Some(current) = node {
            if let Some((_, uri)) = current
                .bindings
                .iter()
                .rev()
                .find(|(bound, _)| bound.as_ref() == prefix)
            {
                return Some(uri);
            }
            node = current.parent.as_deref();
        }
        None
    }

    /// The white space mode in scope.
    pub fn space(&self) -> Space {
        self.space
    }
}

/// An element of a document as it was written, from the `<` of its start tag to the `>` of its end tag, with the [`Context`] it was in.
///
/// A fragment keeps unknown content for lossless round trips: written back where it came from, it is the same bytes ([`Writer::fragment`](crate::Writer::fragment)); written somewhere else, it gets the namespace declarations it needs there. Its text uses the prefixes of the document it came from and may refer to namespaces declared outside it, which is why it carries its context.
#[derive(Clone, Debug)]
pub struct Fragment<'a> {
    pub(crate) text: Cow<'a, str>,
    pub(crate) context: Context,
    /// Where, in the text, the name of the root element ends: where declarations go.
    pub(crate) name_end: usize,
}

impl<'a> Fragment<'a> {
    /// A fragment of `text`, which must be exactly one element that is well-formed in `context`.
    ///
    /// # Errors
    ///
    /// The first problem [`Reader`] finds in `text` read in `context`, or [`ErrorKind::ContentOutsideRoot`] if it is not exactly one element (white space, comments or anything else before or after it), [`ErrorKind::MisplacedDeclaration`] for an XML declaration.
    pub fn new(
        text: impl Into<Cow<'a, str>>,
        context: Context,
        limits: &Limits,
    ) -> Result<Fragment<'a>, Error> {
        let text = text.into();
        let name_end = check_fragment(&text, &context, limits)?;
        Ok(Fragment {
            text,
            context,
            name_end,
        })
    }

    /// The element's text, exactly as it was written.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The namespace bindings and white space mode the element was in.
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// The fragment with its own copy of the text.
    pub fn into_owned(self) -> Fragment<'static> {
        Fragment {
            text: Cow::Owned(self.text.into_owned()),
            context: self.context,
            name_end: self.name_end,
        }
    }

    /// The bindings of the context that the fragment uses, as (prefix, namespace name) pairs sorted by prefix: the prefixes of its element and attribute names, those that its Markup Compatibility attributes (and the `Requires` attribute of `mc:Choice`) list, and the prefix of the qualified name in an `xsi:type` attribute, wherever they are not declared inside the fragment. `("", "")` means that unprefixed element names in the fragment need no default namespace to be in force. Other qualified names in attribute values or text are not recognized, as in Exclusive XML Canonicalization.
    ///
    /// # Errors
    ///
    /// The first problem [`Reader`] finds in the fragment (none, for a fragment that [`Reader::capture`] made).
    pub fn needed_namespaces(&self, limits: &Limits) -> Result<Vec<(String, String)>, Error> {
        Ok(self.analyse(limits)?.0.into_iter().collect())
    }

    /// The fragment as a standalone element: its text, with the declarations of the namespaces it needs from its context, and `xml:space="preserve"` if that was in force and the element does not set `xml:space` itself, added to its start tag.
    ///
    /// # Errors
    ///
    /// The first problem [`Reader`] finds in the fragment (none, for a fragment that [`Reader::capture`] made).
    pub fn to_standalone(&self, limits: &Limits) -> Result<String, Error> {
        let (needed, sets_space) = self.analyse(limits)?;
        let mut added = String::new();
        for (prefix, uri) in &needed {
            if prefix.is_empty() {
                if !uri.is_empty() {
                    added.push_str(" xmlns=\"");
                    crate::escape::attribute(uri, &mut added);
                    added.push('"');
                }
            } else {
                added.push_str(" xmlns:");
                added.push_str(prefix);
                added.push_str("=\"");
                crate::escape::attribute(uri, &mut added);
                added.push('"');
            }
        }
        if self.context.space == Space::Preserve && !sets_space {
            added.push_str(" xml:space=\"preserve\"");
        }
        Ok(self.with_inserted(&added))
    }

    /// The text with `added` inserted after the root element's name.
    pub(crate) fn with_inserted(&self, added: &str) -> String {
        let (head, tail) = self.text.split_at(self.name_end.min(self.text.len()));
        let mut out = String::with_capacity(self.text.len() + added.len());
        out.push_str(head);
        out.push_str(added);
        out.push_str(tail);
        out
    }

    /// The namespaces the fragment needs from its context, and whether its root element sets `xml:space`.
    pub(crate) fn analyse(
        &self,
        limits: &Limits,
    ) -> Result<(BTreeMap<String, String>, bool), Error> {
        let source = Source::new(self.text.as_bytes(), limits)?;
        let mut reader = Reader::with_context(&source, limits, &self.context)?;
        let mut needed = BTreeMap::new();
        let mut sets_space = false;
        while reader.advance()? {
            if let Some(root) = reader.note_uses(&mut needed) {
                sets_space |= root;
            }
        }
        Ok((needed, sets_space))
    }
}

/// Checks that `text` is exactly one element, well-formed in `context`, and returns where the name of its start tag ends.
fn check_fragment(text: &str, context: &Context, limits: &Limits) -> Result<usize, Error> {
    let outside = |offset| Error::new(offset, ErrorKind::ContentOutsideRoot);
    let source = Source::new(text.as_bytes(), limits)?;
    if source.declaration().is_some() {
        return Err(Error::new(0, ErrorKind::MisplacedDeclaration));
    }
    if source.text().len() != text.len() {
        // A byte order mark, which Source removed.
        return Err(outside(0));
    }
    let mut reader = Reader::with_context(&source, limits, context)?;
    let name_end = match reader.next_event()? {
        Some(Event::Start(tag)) if tag.span().start == 0 => {
            let name = tag.name();
            let colon = usize::from(!name.prefix().is_empty());
            1 + name.prefix().len() + colon + name.local().len()
        }
        Some(_) | None => return Err(outside(0)),
    };
    loop {
        match reader.next_event()? {
            Some(Event::End(tag)) if tag.depth() == 1 => {
                if tag.span().end != text.len() {
                    return Err(outside(tag.span().end));
                }
                break;
            }
            Some(_) => {}
            None => return Err(Error::new(text.len(), ErrorKind::UnexpectedEnd)),
        }
    }
    if reader.next_event()?.is_some() {
        return Err(outside(text.len()));
    }
    Ok(name_end)
}
