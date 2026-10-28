//! The reader: a streaming, namespace-aware reader of XML 1.0 that refuses what XML forbids and what could exhaust resources.

use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use quick_xml::errors::{Error as TokenError, IllFormedError, SyntaxError};
use quick_xml::events::Event as Token;

use crate::chars::{find_invalid_char, is_name, is_ncname, is_space, split_qname};
use crate::event::{
    AttributeData, CData, Comment, ElementData, EndTag, Event, NamespaceId, NamespaceTable,
    ProcessingInstruction, Space, StartTag, Text,
};
use crate::fragment::{Context, ContextNode, Fragment, Uses};
use crate::markup;
use crate::namespace::KnownNamespace;
use crate::{Error, ErrorKind, Limits, Source};

/// The namespace of the `xml` prefix, bound in every document.
pub(crate) const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
/// The namespace of namespace declarations, which no prefix may be bound to.
pub(crate) const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";
/// The identifiers the reader gives the two namespaces above, which every table starts with.
pub(crate) const XML_ID: NamespaceId = NamespaceId(0);
pub(crate) const XMLNS_ID: NamespaceId = NamespaceId(1);
/// How many namespaces every table starts with; they do not count toward [`Limits::max_namespaces`].
const BUILT_IN_NAMESPACES: usize = 2;

/// A namespace binding in scope: a prefix (`""` for the default namespace) bound to a namespace, or, for the default namespace, to none (`xmlns=""`).
#[derive(Debug)]
pub(crate) struct Binding<'a> {
    pub(crate) prefix: Cow<'a, str>,
    pub(crate) namespace: Option<NamespaceId>,
    /// The depth of the element that declared it; 0 for the context a fragment is read in.
    pub(crate) depth: usize,
}

/// The namespace bindings in scope, with an index by prefix so that resolving a name takes logarithmic time however many bindings are in scope.
#[derive(Debug, Default)]
pub(crate) struct Scope<'a> {
    bindings: Vec<Binding<'a>>,
    by_prefix: BTreeMap<Cow<'a, str>, Vec<usize>>,
}

impl<'a> Scope<'a> {
    fn push(&mut self, prefix: Cow<'a, str>, namespace: Option<NamespaceId>, depth: usize) {
        let index = self.bindings.len();
        self.by_prefix
            .entry(prefix.clone())
            .or_default()
            .push(index);
        self.bindings.push(Binding {
            prefix,
            namespace,
            depth,
        });
    }

    fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Removes the bindings from position `length` on.
    fn truncate(&mut self, length: usize) {
        while self.bindings.len() > length {
            let Some(binding) = self.bindings.pop() else {
                break;
            };
            if let Some(stack) = self.by_prefix.get_mut(binding.prefix.as_ref()) {
                stack.pop();
                if stack.is_empty() {
                    self.by_prefix.remove(binding.prefix.as_ref());
                }
            }
        }
    }

    /// The innermost binding of `prefix`.
    pub(crate) fn resolve(&self, prefix: &str) -> Option<&Binding<'a>> {
        let index = *self.by_prefix.get(prefix)?.last()?;
        self.bindings.get(index)
    }

    /// The bindings in scope, oldest first.
    pub(crate) fn bindings(&self) -> &[Binding<'a>] {
        &self.bindings
    }
}

/// An open element.
#[derive(Debug)]
struct Frame<'a> {
    element: ElementData<'a>,
    /// The length of the scope before the element's own declarations.
    bindings: usize,
    /// The context inside the element, made the first time it is asked for.
    context: OnceCell<Option<Arc<ContextNode>>>,
}

/// Where the reader is in the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    BeforeRoot,
    InRoot,
    AfterRoot,
    Done,
}

/// The event the reader has just read.
#[derive(Debug)]
enum Current<'a> {
    Nothing,
    Start(ElementData<'a>),
    End(ElementData<'a>, Range<usize>),
    Text {
        span: Range<usize>,
        buffered: bool,
        space: Space,
    },
    CData(Cow<'a, str>, Range<usize>),
    Comment(Range<usize>),
    ProcessingInstruction {
        span: Range<usize>,
        target: Range<usize>,
        data: Range<usize>,
    },
}

/// Reads a document event by event, and refuses it at the first thing that XML 1.0 or Namespaces in XML forbids, or that goes beyond the [`Limits`].
///
/// ```
/// use bayan_xml::{Event, KnownNamespace, Limits, Reader, Source};
///
/// # fn main() -> Result<(), bayan_xml::Error> {
/// let part = br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t xml:space="preserve"> Hello </w:t></w:r></w:p></w:body></w:document>"#;
/// let limits = Limits::default();
/// let source = Source::new(part, &limits)?;
/// let mut reader = Reader::new(&source, &limits);
/// let mut text = String::new();
/// while let Some(event) = reader.next_event()? {
///     if let Event::Text(run) = event {
///         text.push_str(run.value());
///     }
/// }
/// assert_eq!(text, " Hello ");
/// # Ok(())
/// # }
/// ```
pub struct Reader<'a> {
    text: &'a str,
    declaration_end: Option<usize>,
    tokens: quick_xml::Reader<&'a [u8]>,
    limits: Limits,
    namespaces: NamespaceTable<'a>,
    scope: Scope<'a>,
    elements: Vec<Frame<'a>>,
    phase: Phase,
    lookahead: Option<(Token<'a>, Range<usize>)>,
    current: Current<'a>,
    attributes: Vec<AttributeData<'a>>,
    duplicates: Vec<(Option<NamespaceId>, &'a str, usize)>,
    text_buffer: String,
    end_of_empty_element: bool,
    initial_space: Space,
    /// The context the root element is in.
    base: Option<Arc<ContextNode>>,
}

impl<'a> Reader<'a> {
    /// A reader of `source`, within `limits`.
    pub fn new(source: &'a Source<'_>, limits: &Limits) -> Reader<'a> {
        let text = source.text();
        let mut tokens = quick_xml::Reader::from_str(text);
        let config = tokens.config_mut();
        // bayan-xml checks end tags, comments and everything else inside markup itself.
        config.check_end_names = false;
        config.check_comments = false;
        config.trim_markup_names_in_closing_tags = false;
        config.expand_empty_elements = false;
        config.allow_dangling_amp = false;
        config.allow_unmatched_ends = false;
        config.trim_text(false);
        let mut namespaces = NamespaceTable::default();
        namespaces.intern(Cow::Borrowed(XML_NAMESPACE), None);
        namespaces.intern(Cow::Borrowed(XMLNS_NAMESPACE), None);
        let mut scope = Scope::default();
        scope.push(Cow::Borrowed("xml"), Some(XML_ID), 0);
        Reader {
            text,
            declaration_end: source
                .declaration()
                .map(|declaration| declaration.span().end),
            tokens,
            limits: *limits,
            namespaces,
            scope,
            elements: Vec::new(),
            phase: Phase::BeforeRoot,
            lookahead: None,
            current: Current::Nothing,
            attributes: Vec::new(),
            duplicates: Vec::new(),
            text_buffer: String::new(),
            end_of_empty_element: false,
            initial_space: Space::Default,
            base: None,
        }
    }

    /// A reader of `source`, a fragment of a document, read in `context`: its bindings and white space mode apply to the root element, as they did where the fragment came from. Every binding of the context is checked, so this takes time in proportion to the context's bindings and their names, besides the text.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidNamespaceDeclaration`] if a binding of the context breaks the rules of Namespaces in XML (a prefix that is not an NCName, the `xmlns` prefix, the `xml` prefix bound elsewhere, a reserved namespace name, a prefix bound to an empty name, or a name with a character XML does not allow), [`ErrorKind::NameTooLong`] for a prefix longer than [`Limits::max_name_length`], [`ErrorKind::NamespaceNameTooLong`] for a namespace name longer than [`Limits::max_namespace_name_length`], [`ErrorKind::TooManyNamespaces`] beyond [`Limits::max_namespaces`]; all at offset 0.
    pub fn with_context(
        source: &'a Source<'_>,
        limits: &Limits,
        context: &Context,
    ) -> Result<Reader<'a>, Error> {
        let mut reader = Reader::new(source, limits);
        let invalid = Error::new(0, ErrorKind::InvalidNamespaceDeclaration);
        for (prefix, shared) in context.shared_bindings() {
            let uri: &str = shared;
            if prefix.len() > limits.max_name_length {
                return Err(Error::new(0, ErrorKind::NameTooLong));
            }
            if uri.len() > limits.max_namespace_name_length {
                return Err(Error::new(0, ErrorKind::NamespaceNameTooLong));
            }
            if find_invalid_char(uri).is_some() {
                return Err(invalid);
            }
            let reserved = uri == XML_NAMESPACE || uri == XMLNS_NAMESPACE;
            let namespace = match prefix {
                "xml" if uri == XML_NAMESPACE => continue,
                "" if uri.is_empty() => None,
                "" if !reserved => Some(reader.intern_shared(shared)?),
                _ if is_ncname(prefix)
                    && prefix != "xml"
                    && prefix != "xmlns"
                    && !uri.is_empty()
                    && !reserved =>
                {
                    Some(reader.intern_shared(shared)?)
                }
                _ => return Err(invalid),
            };
            reader
                .scope
                .push(Cow::Owned(prefix.to_owned()), namespace, 0);
        }
        reader.initial_space = context.space;
        reader.base.clone_from(&context.node);
        Ok(reader)
    }

    /// The namespace bindings and white space mode in which the current event is: for a start tag, those of the place where its element is (without its own declarations); for anything else, those inside the innermost open element.
    pub fn context(&self) -> Context {
        let inside = match self.current {
            Current::Start(_) => self.elements.len().checked_sub(2),
            _ => self.elements.len().checked_sub(1),
        };
        let space = inside
            .and_then(|index| self.elements.get(index))
            .map_or(self.initial_space, |frame| frame.element.space);
        Context {
            node: self.context_inside(inside),
            space,
        }
    }

    /// The chain of declarations in scope inside the open element at `index` (`None` for outside the root), made once for each element and shared.
    fn context_inside(&self, index: Option<usize>) -> Option<Arc<ContextNode>> {
        let Some(index) = index else {
            return self.base.clone();
        };
        if let Some(node) = self
            .elements
            .get(index)
            .and_then(|frame| frame.context.get())
        {
            return node.clone();
        }
        let mut first = index;
        while first > 0
            && self
                .elements
                .get(first - 1)
                .is_some_and(|frame| frame.context.get().is_none())
        {
            first -= 1;
        }
        let mut node = match first.checked_sub(1) {
            None => self.base.clone(),
            Some(parent) => self
                .elements
                .get(parent)
                .and_then(|frame| frame.context.get())
                .cloned()
                .flatten(),
        };
        for position in first..=index {
            let Some(frame) = self.elements.get(position) else {
                break;
            };
            let end = self
                .elements
                .get(position + 1)
                .map_or(self.scope.len(), |next| next.bindings);
            let own = self
                .scope
                .bindings()
                .get(frame.bindings..end)
                .unwrap_or_default();
            if !own.is_empty() {
                node = Some(Arc::new(ContextNode {
                    parent: node,
                    bindings: own
                        .iter()
                        .map(|binding| (Box::from(binding.prefix.as_ref()), self.shared(binding)))
                        .collect(),
                }));
            }
            let _ = frame.context.set(node.clone());
        }
        node
    }

    /// Reads the element whose start tag is the current event to its end, and returns it as a fragment, with its context and the bindings of the context that it uses. Afterwards the current event is the element's end tag, and reading continues after it. Everything inside is checked as usual, and nothing is read twice: capturing takes the time reading the element takes.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::NotAtStartTag`] if the current event is not a start tag; otherwise the first problem found inside the element.
    pub fn capture(&mut self) -> Result<Fragment<'a>, Error> {
        let Current::Start(element) = &self.current else {
            return Err(Error::new(self.position(), ErrorKind::NotAtStartTag));
        };
        let start = element.span.start;
        let depth = element.depth;
        let name_end = 1 + element.qname.len();
        let compatibility = element
            .namespace
            .and_then(|id| self.namespaces.get(id))
            .is_some_and(|namespace| namespace.is(KnownNamespace::MarkupCompatibility));
        let context = self.context();
        let mut uses = Uses::new();
        self.note_uses(depth, &mut uses);
        let sets_space = self.sets_space();
        loop {
            if !self.advance()? {
                return Err(Error::new(self.text.len(), ErrorKind::UnexpectedEnd));
            }
            match &self.current {
                Current::End(element, span) if element.depth == depth => {
                    let text = Cow::Borrowed(self.slice(&(start..span.end)));
                    return Ok(Fragment::from_parts(
                        text,
                        context,
                        name_end,
                        &uses,
                        sets_space,
                        compatibility,
                    ));
                }
                Current::Start(_) => self.note_uses(depth, &mut uses),
                _ => {}
            }
        }
    }

    /// Adds to `uses` the bindings that the current start tag (if it is one) uses and that are declared outside the element at depth `root_depth`: by an ancestor of it, or in the context the reader was given. Uses are the prefixes of the element's and attributes' names, those that its Markup Compatibility attributes (and `Requires` on `mc:Choice`) list, and the prefix of the qualified name in `xsi:type`; an unprefixed element name, or `xsi:type` value, uses the default namespace, or the absence of one (`""` bound to `""`).
    pub(crate) fn note_uses(&self, root_depth: usize, uses: &mut Uses) {
        let Current::Start(element) = &self.current else {
            return;
        };
        self.note_prefix(element.prefix, root_depth, uses);
        let compatibility = KnownNamespace::MarkupCompatibility.uri();
        let instance = KnownNamespace::XmlSchemaInstance.uri();
        let choice = element.local == "Choice"
            && element
                .namespace
                .and_then(|id| self.namespaces.get(id))
                .is_some_and(|namespace| namespace.uri() == compatibility);
        for attribute in &self.attributes {
            if attribute.declaration {
                continue;
            }
            if !attribute.prefix.is_empty() {
                self.note_prefix(attribute.prefix, root_depth, uses);
            }
            let uri = attribute
                .namespace
                .and_then(|id| self.namespaces.get(id))
                .map(|namespace| namespace.uri());
            let tokens = attribute.value.split_ascii_whitespace();
            match (uri, attribute.local) {
                (Some(uri), "Ignorable" | "MustUnderstand") if uri == compatibility => {
                    tokens.for_each(|prefix| self.note_prefix(prefix, root_depth, uses));
                }
                (None, "Requires") if choice => {
                    tokens.for_each(|prefix| self.note_prefix(prefix, root_depth, uses));
                }
                (Some(uri), "ProcessContent" | "PreserveElements" | "PreserveAttributes")
                    if uri == compatibility =>
                {
                    for token in tokens {
                        if let Some((prefix, _)) = token.split_once(':') {
                            self.note_prefix(prefix, root_depth, uses);
                        }
                    }
                }
                (Some(uri), "type") if uri == instance => {
                    let value = attribute
                        .value
                        .trim_matches(|c: char| u8::try_from(c).is_ok_and(is_space));
                    if let Some((prefix, _)) = split_qname(value) {
                        self.note_prefix(prefix, root_depth, uses);
                    }
                }
                _ => {}
            }
        }
    }

    /// Whether the current start tag has an `xml:space` attribute.
    pub(crate) fn sets_space(&self) -> bool {
        matches!(self.current, Current::Start(_))
            && self
                .attributes
                .iter()
                .any(|attribute| attribute.namespace == Some(XML_ID) && attribute.local == "space")
    }

    /// Adds `prefix` to `uses` if its binding is declared outside the element at depth `root_depth` (or, for `""`, if no default namespace is declared at all).
    fn note_prefix(&self, prefix: &str, root_depth: usize, uses: &mut Uses) {
        if prefix == "xml" || uses.contains_key(prefix) {
            return;
        }
        match self.scope.resolve(prefix) {
            None if prefix.is_empty() => {
                uses.insert(Box::from(""), (Arc::from(""), 0));
            }
            Some(binding) if binding.depth < root_depth => {
                uses.insert(Box::from(prefix), (self.shared(binding), binding.depth));
            }
            _ => {}
        }
    }

    /// The shared copy of the namespace name that `binding` binds its prefix to, `""` for none.
    fn shared(&self, binding: &Binding<'_>) -> Arc<str> {
        binding
            .namespace
            .and_then(|id| self.namespaces.shared(id))
            .unwrap_or_else(|| Arc::from(""))
    }

    /// The element and the attributes of the current start tag, if the current event is one.
    pub(crate) fn start_data(&self) -> Option<(&ElementData<'a>, &[AttributeData<'a>])> {
        match &self.current {
            Current::Start(element) => Some((element, &self.attributes)),
            _ => None,
        }
    }

    /// The identifier of the namespace that `prefix` is bound to where the reader is, if any.
    pub(crate) fn namespace_id_of(&self, prefix: &str) -> Option<NamespaceId> {
        self.scope.resolve(prefix)?.namespace
    }

    /// The namespace with identifier `id`.
    pub(crate) fn namespace(&self, id: NamespaceId) -> Option<crate::Namespace<'_>> {
        self.namespaces.get(id)
    }

    /// The text of the document at `span`.
    pub(crate) fn slice_of(&self, span: &Range<usize>) -> &'a str {
        self.slice(span)
    }

    /// Where the reader is in the text.
    pub(crate) fn offset(&self) -> usize {
        self.position()
    }

    /// The next event, or `None` at the end of the document.
    ///
    /// # Errors
    ///
    /// The first problem found in the document, with its offset. After an error, and after the end, every call returns `Ok(None)`.
    pub fn next_event(&mut self) -> Result<Option<Event<'_>>, Error> {
        if self.advance()? {
            Ok(self.event())
        } else {
            Ok(None)
        }
    }

    /// The nesting depth at the current position: the number of open elements, counting the current start tag's element and not counting the current end tag's.
    pub fn depth(&self) -> usize {
        self.elements.len()
    }

    /// Moves to the next event. Returns `false` at the end of the document.
    pub(crate) fn advance(&mut self) -> Result<bool, Error> {
        if self.phase == Phase::Done {
            return Ok(false);
        }
        match self.step() {
            Ok(true) => Ok(true),
            Ok(false) => {
                self.phase = Phase::Done;
                self.current = Current::Nothing;
                Ok(false)
            }
            Err(error) => {
                self.phase = Phase::Done;
                self.current = Current::Nothing;
                Err(error)
            }
        }
    }

    /// The current event, if there is one.
    pub(crate) fn event(&self) -> Option<Event<'_>> {
        Some(match &self.current {
            Current::Nothing => return None,
            Current::Start(element) => Event::Start(StartTag {
                element,
                attributes: &self.attributes,
                namespaces: &self.namespaces,
            }),
            Current::End(element, span) => Event::End(EndTag {
                element,
                span: span.clone(),
                namespaces: &self.namespaces,
            }),
            Current::Text {
                span,
                buffered,
                space,
            } => Event::Text(Text {
                value: if *buffered {
                    &self.text_buffer
                } else {
                    self.slice(span)
                },
                raw: self.slice(span),
                span: span.clone(),
                space: *space,
            }),
            Current::CData(value, span) => Event::CData(CData {
                value,
                span: span.clone(),
            }),
            Current::Comment(span) => Event::Comment(Comment {
                text: self.slice(&(span.start + 4..span.end.saturating_sub(3))),
                span: span.clone(),
            }),
            Current::ProcessingInstruction { span, target, data } => {
                Event::ProcessingInstruction(ProcessingInstruction {
                    target: self.slice(target),
                    data: self.slice(data),
                    span: span.clone(),
                })
            }
        })
    }

    fn slice(&self, span: &Range<usize>) -> &'a str {
        self.text.get(span.clone()).unwrap_or_default()
    }

    fn position(&self) -> usize {
        usize::try_from(self.tokens.buffer_position()).unwrap_or(self.text.len())
    }

    /// The next token from quick-xml, with its span.
    fn token(&mut self) -> Result<(Token<'a>, Range<usize>), Error> {
        if let Some(token) = self.lookahead.take() {
            return Ok(token);
        }
        let start = self.position();
        match self.tokens.read_event() {
            Ok(token) => Ok((token, start..self.position())),
            Err(error) => Err(self.token_error(&error)),
        }
    }

    /// Translates an error of quick-xml into ours, without any of the document text that quick-xml's errors carry.
    fn token_error(&self, error: &TokenError) -> Error {
        let offset = usize::try_from(self.tokens.error_position()).unwrap_or(self.text.len());
        let rest = self.text.get(offset..).unwrap_or_default();
        let starts_with = |prefix: &str| {
            rest.get(..prefix.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        };
        let kind = match error {
            // quick-xml reports `<![CDATA` and `<!--` that are malformed, rather than unclosed, as unclosed too.
            TokenError::Syntax(SyntaxError::UnclosedCData) if !starts_with("<![CDATA[") => {
                ErrorKind::MalformedMarkup
            }
            TokenError::Syntax(SyntaxError::UnclosedComment) if !starts_with("<!--") => {
                ErrorKind::MalformedMarkup
            }
            TokenError::Syntax(SyntaxError::UnclosedDoctype) => {
                if starts_with("<!DOCTYPE") {
                    ErrorKind::DoctypeNotAllowed
                } else {
                    ErrorKind::MalformedMarkup
                }
            }
            TokenError::Syntax(SyntaxError::InvalidBangMarkup) => ErrorKind::MalformedMarkup,
            TokenError::Syntax(_) => ErrorKind::UnexpectedEnd,
            TokenError::IllFormed(IllFormedError::UnclosedReference) => {
                ErrorKind::MalformedReference
            }
            TokenError::IllFormed(
                IllFormedError::UnmatchedEndTag(_) | IllFormedError::MismatchedEndTag { .. },
            ) => ErrorKind::MismatchedEndTag,
            TokenError::IllFormed(IllFormedError::MissingDoctypeName) => {
                ErrorKind::DoctypeNotAllowed
            }
            TokenError::IllFormed(IllFormedError::DoubleHyphenInComment) => {
                ErrorKind::MalformedComment
            }
            TokenError::IllFormed(IllFormedError::MissingEndTag(_)) => ErrorKind::UnexpectedEnd,
            TokenError::IllFormed(
                IllFormedError::MissingDeclVersion(_) | IllFormedError::UnknownVersion,
            ) => ErrorKind::MalformedDeclaration,
            TokenError::Encoding(_) => ErrorKind::InvalidEncoding,
            TokenError::Io(_)
            | TokenError::InvalidAttr(_)
            | TokenError::Escape(_)
            | TokenError::Namespace(_) => ErrorKind::MalformedMarkup,
        };
        Error::new(offset, kind)
    }

    /// Reads the next event into `current`. Returns `false` at the end of the document.
    fn step(&mut self) -> Result<bool, Error> {
        if self.end_of_empty_element {
            self.end_of_empty_element = false;
            let end = match &self.current {
                Current::Start(element) => element.span.end,
                _ => self.position(),
            };
            self.close_element(end..end);
            return Ok(true);
        }
        if self.phase == Phase::BeforeRoot && self.text.starts_with('\u{FEFF}') {
            // quick-xml would drop a byte order mark here without counting it; after the one that `Source` removed, it is a character before the root.
            return Err(Error::new(0, ErrorKind::ContentOutsideRoot));
        }
        loop {
            let (token, span) = self.token()?;
            match token {
                Token::Eof => return self.finish(),
                Token::Decl(_) => {
                    if span.start == 0 && self.declaration_end == Some(span.end) {
                        continue;
                    }
                    return Err(Error::new(span.start, ErrorKind::MisplacedDeclaration));
                }
                Token::DocType(_) => {
                    return Err(Error::new(span.start, ErrorKind::DoctypeNotAllowed));
                }
                Token::Start(_) => {
                    self.start_element(span, false)?;
                    return Ok(true);
                }
                Token::Empty(_) => {
                    self.start_element(span, true)?;
                    self.end_of_empty_element = true;
                    return Ok(true);
                }
                Token::End(_) => {
                    self.end_element(span)?;
                    return Ok(true);
                }
                Token::Text(_) => {
                    if self.text_run(false, span)? {
                        return Ok(true);
                    }
                }
                Token::GeneralRef(_) => {
                    if self.text_run(true, span)? {
                        return Ok(true);
                    }
                }
                Token::CData(_) => {
                    if self.phase != Phase::InRoot {
                        return Err(Error::new(span.start, ErrorKind::ContentOutsideRoot));
                    }
                    let content = self.slice(&(span.start + 9..span.end.saturating_sub(3)));
                    self.current = Current::CData(markup::normalized_line_ends(content), span);
                    return Ok(true);
                }
                Token::Comment(_) => {
                    let content = self.slice(&(span.start + 4..span.end.saturating_sub(3)));
                    if content.contains("--") || content.ends_with('-') {
                        return Err(Error::new(span.start, ErrorKind::MalformedComment));
                    }
                    self.current = Current::Comment(span);
                    return Ok(true);
                }
                Token::PI(_) => {
                    self.processing_instruction(span)?;
                    return Ok(true);
                }
            }
        }
    }

    /// The end of the input: every element must be closed, and there must have been one.
    fn finish(&mut self) -> Result<bool, Error> {
        if !self.elements.is_empty() {
            return Err(Error::new(self.text.len(), ErrorKind::UnexpectedEnd));
        }
        if self.phase == Phase::BeforeRoot {
            return Err(Error::new(self.text.len(), ErrorKind::NoRootElement));
        }
        Ok(false)
    }

    /// A start tag or empty-element tag at `span`.
    fn start_element(&mut self, span: Range<usize>, empty: bool) -> Result<(), Error> {
        if self.phase == Phase::AfterRoot {
            return Err(Error::new(span.start, ErrorKind::ContentOutsideRoot));
        }
        let depth = self.elements.len() + 1;
        if depth > self.limits.max_depth {
            return Err(Error::new(span.start, ErrorKind::TooDeep));
        }
        let (name, _) = markup::start_tag(self.text, &span, &self.limits, &mut self.attributes)?;
        let bindings = self.scope.len();
        // Declarations first: they apply to the element's own name and attributes, wherever they are written.
        for index in 0..self.attributes.len() {
            if self
                .attributes
                .get(index)
                .is_some_and(|attribute| attribute.declaration)
            {
                self.declare(index, depth)?;
            }
        }
        let namespace = self.resolve_element(name.prefix, name.offset)?;
        for index in 0..self.attributes.len() {
            let Some(attribute) = self.attributes.get(index) else {
                break;
            };
            let resolved = if attribute.declaration {
                Some(XMLNS_ID)
            } else {
                self.resolve_attribute(attribute.prefix, attribute.span.start)?
            };
            if let Some(attribute) = self.attributes.get_mut(index) {
                attribute.namespace = resolved;
            }
        }
        self.check_duplicates()?;
        let parent_space = self
            .elements
            .last()
            .map_or(self.initial_space, |frame| frame.element.space);
        let space = self
            .attributes
            .iter()
            .find(|attribute| attribute.namespace == Some(XML_ID) && attribute.local == "space")
            .and_then(|attribute| match attribute.value.as_ref() {
                "preserve" => Some(Space::Preserve),
                "default" => Some(Space::Default),
                _ => None,
            })
            .unwrap_or(parent_space);
        let element = ElementData {
            qname: name.qname,
            prefix: name.prefix,
            local: name.local,
            namespace,
            span,
            empty,
            depth,
            space,
        };
        self.elements.push(Frame {
            element: element.clone(),
            bindings,
            context: OnceCell::new(),
        });
        self.phase = Phase::InRoot;
        self.current = Current::Start(element);
        Ok(())
    }

    /// Applies the namespace declaration that is attribute `index` of the current start tag.
    fn declare(&mut self, index: usize, depth: usize) -> Result<(), Error> {
        let Some(attribute) = self.attributes.get(index) else {
            return Ok(());
        };
        let offset = attribute.span.start;
        let invalid = Error::new(offset, ErrorKind::InvalidNamespaceDeclaration);
        // `xmlns` declares the default namespace, `xmlns:p` the prefix p.
        let prefix: &'a str = if attribute.prefix.is_empty() {
            ""
        } else {
            attribute.local
        };
        let uri = attribute.value.clone();
        let reserved = uri == XML_NAMESPACE || uri == XMLNS_NAMESPACE;
        let namespace = match prefix {
            "xmlns" => return Err(invalid),
            "xml" => {
                if uri != XML_NAMESPACE {
                    return Err(invalid);
                }
                Some(XML_ID)
            }
            "" if uri.is_empty() => None,
            _ if uri.is_empty() || reserved => return Err(invalid),
            _ => Some(self.intern(uri, offset)?),
        };
        self.scope.push(Cow::Borrowed(prefix), namespace, depth);
        Ok(())
    }

    /// The identifier of the namespace `uri`, declared at `offset`.
    fn intern(&mut self, uri: Cow<'a, str>, offset: usize) -> Result<NamespaceId, Error> {
        if uri.len() > self.limits.max_namespace_name_length {
            return Err(Error::new(offset, ErrorKind::NamespaceNameTooLong));
        }
        if let Some(id) = self.namespaces.find(&uri) {
            return Ok(id);
        }
        if self.namespaces.len() - BUILT_IN_NAMESPACES >= self.limits.max_namespaces {
            return Err(Error::new(offset, ErrorKind::TooManyNamespaces));
        }
        Ok(self.namespaces.intern(uri, None))
    }

    /// The identifier of the namespace `uri`, a binding of the context the reader was given, whose shared copy the reader shares in turn.
    fn intern_shared(&mut self, uri: &Arc<str>) -> Result<NamespaceId, Error> {
        if let Some(id) = self.namespaces.find(uri) {
            return Ok(id);
        }
        if self.namespaces.len() - BUILT_IN_NAMESPACES >= self.limits.max_namespaces {
            return Err(Error::new(0, ErrorKind::TooManyNamespaces));
        }
        Ok(self
            .namespaces
            .intern(Cow::Owned(uri.as_ref().to_owned()), Some(uri)))
    }

    /// The namespace of an element name with `prefix`, written at `offset`.
    fn resolve_element(&self, prefix: &str, offset: usize) -> Result<Option<NamespaceId>, Error> {
        match prefix {
            "xmlns" => Err(Error::new(offset, ErrorKind::ReservedPrefix)),
            "" => Ok(self.scope.resolve("").and_then(|binding| binding.namespace)),
            _ => self.resolve_prefix(prefix, offset).map(Some),
        }
    }

    /// The namespace of an attribute name (not a declaration) with `prefix`, written at `offset`.
    fn resolve_attribute(&self, prefix: &str, offset: usize) -> Result<Option<NamespaceId>, Error> {
        if prefix.is_empty() {
            Ok(None)
        } else {
            self.resolve_prefix(prefix, offset).map(Some)
        }
    }

    fn resolve_prefix(&self, prefix: &str, offset: usize) -> Result<NamespaceId, Error> {
        self.scope
            .resolve(prefix)
            .and_then(|binding| binding.namespace)
            .ok_or(Error::new(offset, ErrorKind::UndeclaredPrefix))
    }

    /// Refuses two attributes of the current start tag with the same expanded name (Namespaces in XML §6.3), reporting the later one.
    fn check_duplicates(&mut self) -> Result<(), Error> {
        if self.attributes.len() < 2 {
            return Ok(());
        }
        self.duplicates.clear();
        for (index, attribute) in self.attributes.iter().enumerate() {
            // A declaration's expanded name is the xmlns namespace with the prefix it declares (`""` for the default namespace).
            let local = if attribute.declaration && attribute.prefix.is_empty() {
                ""
            } else {
                attribute.local
            };
            self.duplicates.push((attribute.namespace, local, index));
        }
        self.duplicates.sort_unstable();
        let later = self
            .duplicates
            .windows(2)
            .filter_map(|pair| match pair {
                [first, second] if first.0 == second.0 && first.1 == second.1 => Some(second.2),
                _ => None,
            })
            .min();
        match later.and_then(|index| self.attributes.get(index)) {
            Some(attribute) => Err(Error::new(
                attribute.span.start,
                ErrorKind::DuplicateAttribute,
            )),
            None => Ok(()),
        }
    }

    /// An end tag at `span`.
    fn end_element(&mut self, span: Range<usize>) -> Result<(), Error> {
        let name = markup::end_tag(self.text, &span)?;
        match self.elements.last() {
            Some(frame) if frame.element.qname == name => {
                self.close_element(span);
                Ok(())
            }
            _ => Err(Error::new(span.start, ErrorKind::MismatchedEndTag)),
        }
    }

    /// Closes the innermost element with the end tag at `span`.
    fn close_element(&mut self, span: Range<usize>) {
        if let Some(frame) = self.elements.pop() {
            self.scope.truncate(frame.bindings);
            if self.elements.is_empty() {
                self.phase = Phase::AfterRoot;
            }
            self.current = Current::End(frame.element, span);
        }
    }

    /// A run of text and references that starts with the token at `span` (a reference if `reference`). Returns whether it is an event: white space outside the root element is skipped.
    ///
    /// Each piece is checked and decoded as soon as it is read, so that the run keeps nothing for each piece: its text is borrowed from the document as long as decoding leaves it unchanged, and from the first piece that decoding changes (a reference, or line ends to normalize) on, it is decoded into one copy of the run.
    fn text_run(&mut self, reference: bool, span: Range<usize>) -> Result<bool, Error> {
        let in_root = self.phase == Phase::InRoot;
        let start = span.start;
        let mut end = span.end;
        let mut buffered = false;
        let mut piece = (reference, span);
        loop {
            let (reference, span) = piece;
            let raw = self.slice(&span);
            if !in_root {
                if reference || !raw.bytes().all(is_space) {
                    return Err(Error::new(span.start, ErrorKind::ContentOutsideRoot));
                }
            } else if reference {
                let name = raw.get(1..raw.len().saturating_sub(1)).unwrap_or_default();
                let character = markup::reference(name, span.start, &self.limits)?;
                if !buffered {
                    self.start_copy(start, span.start);
                    buffered = true;
                }
                self.text_buffer.push(character);
            } else {
                if let Some(offset) = raw.find("]]>") {
                    return Err(Error::new(span.start + offset, ErrorKind::CDataEndInText));
                }
                if !buffered && raw.contains('\r') {
                    self.start_copy(start, span.start);
                    buffered = true;
                }
                if buffered {
                    markup::push_normalized_line_ends(raw, &mut self.text_buffer);
                }
            }
            let (token, next) = self.token()?;
            piece = match token {
                Token::Text(_) => (false, next),
                Token::GeneralRef(_) => (true, next),
                other => {
                    self.lookahead = Some((other, next));
                    break;
                }
            };
            end = piece.1.end;
        }
        if !in_root {
            return Ok(false);
        }
        let space = self
            .elements
            .last()
            .map_or(self.initial_space, |frame| frame.element.space);
        self.current = Current::Text {
            span: start..end,
            buffered,
            space,
        };
        Ok(true)
    }

    /// Starts the copy of the run of text that begins at `start` with `piece`, the first of its pieces that decoding changes: the text before it is the same decoded. The copy gets room for the whole run, which ends at the next `<` (neither character data nor a reference contains one), because decoding never makes text longer, so it needs no more and is never moved.
    fn start_copy(&mut self, start: usize, piece: usize) {
        let rest = self.text.get(piece..).unwrap_or_default();
        let end = piece + rest.find('<').unwrap_or(rest.len());
        self.text_buffer.clear();
        self.text_buffer.reserve_exact(end - start);
        self.text_buffer.push_str(self.slice(&(start..piece)));
    }

    /// A processing instruction at `span`.
    fn processing_instruction(&mut self, span: Range<usize>) -> Result<(), Error> {
        let content_start = span.start + 2;
        let content_end = span.end.saturating_sub(2).max(content_start);
        let content = self.slice(&(content_start..content_end));
        let target_length = content.bytes().position(is_space).unwrap_or(content.len());
        let target = content.get(..target_length).unwrap_or_default();
        let malformed = Error::new(span.start, ErrorKind::MalformedProcessingInstruction);
        if target.is_empty() {
            return Err(malformed);
        }
        if target.len() > self.limits.max_name_length {
            return Err(Error::new(span.start, ErrorKind::NameTooLong));
        }
        if target.eq_ignore_ascii_case("xml") {
            return Err(Error::new(span.start, ErrorKind::MisplacedDeclaration));
        }
        if !is_name(target) || target.contains(':') {
            return Err(malformed);
        }
        let data_start = content
            .bytes()
            .skip(target_length)
            .position(|byte| !is_space(byte))
            .map_or(content.len(), |skipped| target_length + skipped);
        self.current = Current::ProcessingInstruction {
            target: content_start..content_start + target_length,
            data: content_start + data_start..content_end,
            span,
        };
        Ok(())
    }
}

