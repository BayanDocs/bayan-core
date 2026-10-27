//! Markup Compatibility and Extensibility (ECMA-376 Part 3): a reader that applies the processing model of Part 3 §9 in one streaming pass, and keeps everything it takes out of the output for lossless round trips.

mod config;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::str::SplitAsciiWhitespace;

pub use config::McConfig;

use crate::chars::{is_ncname, is_space};
use crate::event::{
    CData, Comment, EndTag, Event, Name, NamespaceId, ProcessingInstruction, StartTag, Text,
};
use crate::fragment::{Context, Fragment, Uses};
use crate::namespace::KnownNamespace;
use crate::reader::XML_ID;
use crate::{CompatibilityError, Error, ErrorKind, Limits, Reader, Source};

/// What Markup Compatibility makes of an attribute of a start tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributeKind {
    /// Part of the output document: in a namespace the configuration understands, in the XML namespace, or in no namespace if the configuration understands that.
    Output,
    /// In a namespace that is neither understood nor ignorable. Part 3 §9 calls this a mismatch, which the processor reports and the application decides about: Word refuses such documents; an application that keeps what it does not understand may keep it.
    NotUnderstood,
    /// In an ignorable namespace that the configuration does not understand: not part of the output document (Part 3 §9.2). `preserve` when `mc:PreserveAttributes` suggests that an editor keep it (2nd edition, §10.1.3).
    Ignored {
        /// Whether `mc:PreserveAttributes` names the attribute.
        preserve: bool,
    },
    /// A Markup Compatibility attribute (`mc:Ignorable`, `mc:ProcessContent`, `mc:MustUnderstand`, `mc:PreserveElements` or `mc:PreserveAttributes`), which the output document leaves out (Part 3 §9.4).
    Compatibility,
    /// A namespace declaration.
    NamespaceDeclaration,
    /// An attribute of an application-defined extension element or of an element inside one, which Markup Compatibility leaves as it is (Part 3 §8).
    Unprocessed,
}

/// Which alternative of `mc:AlternateContent` was selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// The `mc:Choice` at this index among the element's `mc:Choice` elements (from 0).
    Choice(usize),
    /// The `mc:Fallback`.
    Fallback,
    /// None: no `mc:Choice` could be selected and there is no `mc:Fallback`.
    Nothing,
}

/// A start tag, with what Markup Compatibility makes of it and its attributes.
#[derive(Clone, Copy, Debug)]
pub struct McStartTag<'r> {
    tag: StartTag<'r>,
    kinds: &'r [AttributeKind],
    understood: bool,
    extension: bool,
}

impl<'r> McStartTag<'r> {
    /// The start tag as it was read, with every attribute: write it with [`Writer::start_tag`](crate::Writer::start_tag) to keep it whole.
    pub fn tag(&self) -> &StartTag<'r> {
        &self.tag
    }

    /// The element's name.
    pub fn name(&self) -> Name<'r> {
        self.tag.name()
    }

    /// The attributes in the order they are written, namespace declarations included, each with what Markup Compatibility makes of it. The output document has those of kind [`AttributeKind::Output`], [`AttributeKind::NotUnderstood`] and [`AttributeKind::Unprocessed`].
    pub fn attributes(
        &self,
    ) -> impl ExactSizeIterator<Item = (crate::Attribute<'r>, AttributeKind)> {
        self.tag.attributes().zip(self.kinds.iter().copied())
    }

    /// Whether the configuration understands the element's namespace (the XML and Markup Compatibility namespaces always are). An element in a namespace that is neither understood nor ignorable is a mismatch (Part 3 §9.1) that the application decides about.
    pub fn is_understood(&self) -> bool {
        self.understood
    }

    /// Whether this is an application-defined extension element, left as it is with everything inside it (Part 3 §8).
    pub fn is_extension(&self) -> bool {
        self.extension
    }
}

/// An element that Markup Compatibility leaves out of the output document, whole.
#[derive(Clone, Debug)]
pub struct IgnoredElement<'a> {
    fragment: Fragment<'a>,
    preserve: bool,
}

impl<'a> IgnoredElement<'a> {
    /// The element, exactly as it was written, with its context.
    pub fn fragment(&self) -> &Fragment<'a> {
        &self.fragment
    }

    /// The element, exactly as it was written, with its context.
    pub fn into_fragment(self) -> Fragment<'a> {
        self.fragment
    }

    /// Whether `mc:PreserveElements` suggests that an editor keep it (2nd edition, §10.1.3).
    pub fn preserve(&self) -> bool {
        self.preserve
    }
}

/// An `mc:AlternateContent` element, whole, and which of its alternatives was selected.
#[derive(Clone, Debug)]
pub struct AlternateContent<'a> {
    fragment: Fragment<'a>,
    selection: Selection,
}

impl<'a> AlternateContent<'a> {
    /// The element, exactly as it was written, with its context: to write back where the selected alternative was not changed. `mc:AlternateContent` may have no attribute in the XML namespace, so it can be written only where its white space mode holds ([`Writer::fragment`](crate::Writer::fragment)).
    pub fn fragment(&self) -> &Fragment<'a> {
        &self.fragment
    }

    /// The element, exactly as it was written, with its context.
    pub fn into_fragment(self) -> Fragment<'a> {
        self.fragment
    }

    /// The alternative that was selected.
    pub fn selection(&self) -> Selection {
        self.selection
    }
}

/// What a [`McReader`] found: the output document of Markup Compatibility processing, event by event, with everything that the output leaves out reported in a form that keeps it whole.
#[derive(Clone, Debug)]
pub enum McEvent<'r, 'a> {
    /// The start tag of an element of the output document. Its attributes say which of them the output has.
    Start(McStartTag<'r>),
    /// The end of an element of the output document.
    End(EndTag<'r>),
    /// Text of the output document.
    Text(Text<'r>),
    /// A CDATA section of the output document.
    CData(CData<'r>),
    /// A comment of the output document.
    Comment(Comment<'r>),
    /// A processing instruction of the output document.
    ProcessingInstruction(ProcessingInstruction<'r>),
    /// An element that Markup Compatibility ignores (Part 3 §9.2), with everything inside it: not part of the output document.
    Ignored(IgnoredElement<'a>),
    /// The start tag of an element whose content takes its place (`mc:ProcessContent`, Part 3 §9.2): the events up to [`McEvent::UnwrapEnd`] are part of the output document, the element itself is not.
    UnwrapStart(McStartTag<'r>),
    /// The end of an element whose content took its place.
    UnwrapEnd(EndTag<'r>),
    /// The start tag of `mc:AlternateContent`: the events up to [`McEvent::AlternateContentEnd`] are the content of the selected alternative, which takes its place in the output document.
    AlternateContentStart(McStartTag<'r>),
    /// The end of `mc:AlternateContent`, with the whole element and which alternative was selected.
    AlternateContentEnd(AlternateContent<'a>),
}

/// The kind of the event the reader has just produced.
#[derive(Debug)]
enum Current<'a> {
    Nothing,
    Start { understood: bool, extension: bool },
    UnwrapStart { understood: bool },
    AlternateStart,
    End,
    UnwrapEnd,
    Content,
    Ignored(IgnoredElement<'a>),
    AlternateEnd(AlternateContent<'a>),
}

/// Where an open alternate-content element is in its alternatives.
#[derive(Debug)]
struct Alternatives {
    start: usize,
    name_end: usize,
    context: Context,
    choices: usize,
    fallback: bool,
    selection: Option<Selection>,
}

/// What an open element is to Markup Compatibility.
#[derive(Debug)]
enum FrameKind {
    /// An element of the output document.
    Output,
    /// An element whose content takes its place.
    Unwrapped,
    /// An extension element, or an element inside one.
    Unprocessed,
    /// `mc:AlternateContent`, while its alternatives are read.
    Alternate(Alternatives),
    /// The selected `mc:Choice` or `mc:Fallback`.
    Selected,
}

#[derive(Debug)]
struct Frame {
    kind: FrameKind,
    /// The length of the log of declarations before the element's own.
    undo: usize,
}

/// Which table a Markup Compatibility declaration went into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Table {
    ProcessContent,
    PreserveElements,
    PreserveAttributes,
}

/// A declaration of an element's Markup Compatibility attributes, taken back at its end.
#[derive(Debug)]
enum Undo {
    Ignorable(NamespaceId),
    Pair(Table, NamespaceId, Box<str>),
}

/// What an element's Markup Compatibility attributes say, read and checked.
#[derive(Debug, Default)]
struct Declarations {
    ignorable: Vec<NamespaceId>,
    must_understand: Vec<(NamespaceId, usize)>,
    pairs: Vec<(Table, NamespaceId, Box<str>, usize)>,
}

/// The result of one step.
enum Step {
    Event,
    Nothing,
    End,
}

/// Reads a document with Markup Compatibility processing (ECMA-376 Part 3, 5th edition, §9), in one streaming pass: its events are those of the output document, with what the output leaves out reported whole, so that an application can interpret the output and still write the document back without loss.
///
/// - Elements and attributes in ignorable namespaces that the configuration does not understand are left out: elements as [`McEvent::Ignored`] fragments, attributes as [`AttributeKind::Ignored`]. Elements that `mc:ProcessContent` names are replaced by their content, between [`McEvent::UnwrapStart`] and [`McEvent::UnwrapEnd`].
/// - `mc:AlternateContent` is replaced by the content of its first `mc:Choice` whose `Requires` namespaces are all understood, else of its `mc:Fallback`, between [`McEvent::AlternateContentStart`] and [`McEvent::AlternateContentEnd`], which carries the whole element. Alternatives are selected at every level of nesting; nothing inside an alternative that is not selected is processed or checked.
/// - Application-defined extension elements, and everything inside them, are passed through unchanged.
/// - A namespace that `mc:MustUnderstand` names and the configuration does not understand stops processing with [`CompatibilityError::MustUnderstand`]; so does every breach of the syntax of Part 3 §7 that processing reaches. Names in namespaces that are neither understood nor ignorable are reported ([`McStartTag::is_understood`], [`AttributeKind::NotUnderstood`]), not refused.
/// - `mc:PreserveElements` and `mc:PreserveAttributes` (Part 3, 2nd edition, §10.1.3, which the 5th edition removed) are accepted, checked and reported as suggestions.
///
/// Processing takes time linear in the size of the document; what it keeps in memory is bounded by the depth, the attributes of each start tag and [`Limits::max_compatibility_tokens`].
pub struct McReader<'a> {
    reader: Reader<'a>,
    config: McConfig,
    limits: Limits,
    /// Whether each namespace is understood, by identifier, once asked.
    understood: Vec<Option<bool>>,
    /// How many `mc:Ignorable` attributes in scope name each namespace, by identifier.
    ignorable: Vec<u32>,
    /// The process-content and preservation pairs in scope, with how many declarations name each.
    pairs: BTreeMap<(Table, NamespaceId), BTreeMap<Box<str>, u32>>,
    undo: Vec<Undo>,
    frames: Vec<Frame>,
    kinds: Vec<AttributeKind>,
    /// For each open `mc:AlternateContent`, innermost last: its depth, and the bindings declared outside it that its content uses, which its fragment needs.
    alternates: Vec<(usize, Uses)>,
    /// The namespace, local name and whether it is a declaration of each attribute of the current start tag, kept to save allocations.
    scratch: Vec<(Option<NamespaceId>, &'a str, bool)>,
    current: Current<'a>,
    done: bool,
}

/// Whether `namespace` is the Markup Compatibility namespace.
fn is_compatibility(namespace: Option<crate::Namespace<'_>>) -> bool {
    namespace.is_some_and(|namespace| namespace.is(KnownNamespace::MarkupCompatibility))
}

impl<'a> McReader<'a> {
    /// A reader of `source`, within `limits`, processing Markup Compatibility as `config` says.
    pub fn new(source: &'a Source<'_>, limits: &Limits, config: &McConfig) -> McReader<'a> {
        McReader::with_reader(Reader::new(source, limits), limits, config)
    }

    /// A reader of `source`, a fragment of a document, read in `context` (see [`Reader::with_context`]). Markup Compatibility starts afresh inside it, as an application that processes the content of an extension element again does (Part 3 §8; 2nd edition, §12).
    ///
    /// # Errors
    ///
    /// As [`Reader::with_context`].
    pub fn with_context(
        source: &'a Source<'_>,
        limits: &Limits,
        config: &McConfig,
        context: &Context,
    ) -> Result<McReader<'a>, Error> {
        let reader = Reader::with_context(source, limits, context)?;
        Ok(McReader::with_reader(reader, limits, config))
    }

    fn with_reader(reader: Reader<'a>, limits: &Limits, config: &McConfig) -> McReader<'a> {
        McReader {
            reader,
            config: config.clone(),
            limits: *limits,
            understood: Vec::new(),
            ignorable: Vec::new(),
            pairs: BTreeMap::new(),
            undo: Vec::new(),
            frames: Vec::new(),
            kinds: Vec::new(),
            alternates: Vec::new(),
            scratch: Vec::new(),
            current: Current::Nothing,
            done: false,
        }
    }

    /// The next event of the output document, or `None` at the end.
    ///
    /// # Errors
    ///
    /// The first problem [`Reader`] finds, or [`ErrorKind::Compatibility`] for a breach of Markup Compatibility, or [`ErrorKind::TooManyCompatibilityTokens`] for a list longer than [`Limits::max_compatibility_tokens`]. After an error, and after the end, every call returns `Ok(None)`.
    pub fn next_event(&mut self) -> Result<Option<McEvent<'_, 'a>>, Error> {
        if self.done {
            return Ok(None);
        }
        loop {
            match self.step() {
                Ok(Step::Event) => return Ok(self.event()),
                Ok(Step::Nothing) => {}
                Ok(Step::End) => {
                    self.done = true;
                    self.current = Current::Nothing;
                    return Ok(None);
                }
                Err(error) => {
                    self.done = true;
                    self.current = Current::Nothing;
                    return Err(error);
                }
            }
        }
    }

    /// Reads the element whose start tag is the current event ([`McEvent::Start`], [`McEvent::UnwrapStart`] or [`McEvent::AlternateContentStart`]) to its end, without processing anything inside it, and returns it as a fragment with its context. Reading continues after it. An element whose content Markup Compatibility processes in its place ([`McEvent::UnwrapStart`]) may not have `xml:space`, so the fragment is marked to keep its white space mode (see [`Writer::fragment`](crate::Writer::fragment)).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::NotAtStartTag`] if the current event is not such a start tag; otherwise the first problem [`Reader`] finds inside the element.
    pub fn capture(&mut self) -> Result<Fragment<'a>, Error> {
        if !matches!(
            self.current,
            Current::Start { .. } | Current::UnwrapStart { .. } | Current::AlternateStart
        ) {
            return Err(Error::new(self.reader.offset(), ErrorKind::NotAtStartTag));
        }
        if matches!(self.current, Current::AlternateStart) {
            self.alternates.pop();
        }
        let unwrapped = matches!(self.current, Current::UnwrapStart { .. });
        self.current = Current::Nothing;
        self.pop_frame();
        match self.reader.capture() {
            Ok(mut fragment) => {
                self.note_captured(&fragment);
                fragment.fixed_space |= unwrapped;
                Ok(fragment)
            }
            Err(error) => {
                self.done = true;
                Err(error)
            }
        }
    }

    /// The event the reader has just produced.
    fn event(&self) -> Option<McEvent<'_, 'a>> {
        let event = self.reader.event();
        Some(match (&self.current, event) {
            (
                &Current::Start {
                    understood,
                    extension,
                },
                Some(Event::Start(tag)),
            ) => McEvent::Start(McStartTag {
                tag,
                kinds: &self.kinds,
                understood,
                extension,
            }),
            (&Current::UnwrapStart { understood }, Some(Event::Start(tag))) => {
                McEvent::UnwrapStart(McStartTag {
                    tag,
                    kinds: &self.kinds,
                    understood,
                    extension: false,
                })
            }
            (Current::AlternateStart, Some(Event::Start(tag))) => {
                McEvent::AlternateContentStart(McStartTag {
                    tag,
                    kinds: &self.kinds,
                    understood: true,
                    extension: false,
                })
            }
            (Current::End, Some(Event::End(tag))) => McEvent::End(tag),
            (Current::UnwrapEnd, Some(Event::End(tag))) => McEvent::UnwrapEnd(tag),
            (Current::Content, Some(Event::Text(text))) => McEvent::Text(text),
            (Current::Content, Some(Event::CData(cdata))) => McEvent::CData(cdata),
            (Current::Content, Some(Event::Comment(comment))) => McEvent::Comment(comment),
            (Current::Content, Some(Event::ProcessingInstruction(pi))) => {
                McEvent::ProcessingInstruction(pi)
            }
            (Current::Ignored(ignored), _) => McEvent::Ignored(ignored.clone()),
            (Current::AlternateEnd(alternate), _) => {
                McEvent::AlternateContentEnd(alternate.clone())
            }
            _ => return None,
        })
    }

    /// Reads the next event of the underlying reader and decides what it is in the output document.
    fn step(&mut self) -> Result<Step, Error> {
        self.current = Current::Nothing;
        if !self.reader.advance()? {
            return Ok(Step::End);
        }
        enum Read {
            Start,
            End,
            Text { offset: usize, blank: bool },
            Other,
        }
        let read = match self.reader.event() {
            Some(Event::Start(_)) => Read::Start,
            Some(Event::End(_)) => Read::End,
            Some(Event::Text(text)) => Read::Text {
                offset: text.span().start,
                blank: text.value().bytes().all(is_space),
            },
            Some(Event::CData(cdata)) => Read::Text {
                offset: cdata.span().start,
                blank: cdata.value().bytes().all(is_space),
            },
            Some(Event::Comment(_) | Event::ProcessingInstruction(_)) => Read::Other,
            None => return Ok(Step::Nothing),
        };
        let inside_alternatives = matches!(
            self.frames.last(),
            Some(Frame {
                kind: FrameKind::Alternate(_),
                ..
            })
        );
        if matches!(read, Read::Start)
            && let Some((depth, uses)) = self.alternates.last_mut()
        {
            self.reader.note_uses(*depth, uses);
        }
        match read {
            Read::Start if inside_alternatives => self.alternative(),
            Read::Start => self.start(),
            Read::End => self.end(),
            Read::Text { offset, blank } if inside_alternatives => {
                if blank {
                    Ok(Step::Nothing)
                } else {
                    Err(compatibility_error(
                        offset,
                        CompatibilityError::UnexpectedContent,
                    ))
                }
            }
            Read::Other if inside_alternatives => Ok(Step::Nothing),
            Read::Text { .. } | Read::Other => {
                self.current = Current::Content;
                Ok(Step::Event)
            }
        }
    }

    /// The name, offset and namespace of the current start tag's element.
    fn element(&self) -> Option<(Option<NamespaceId>, &'a str, usize)> {
        let (element, _) = self.reader.start_data()?;
        Some((element.namespace, element.local, element.span.start))
    }

    fn is_compatibility_namespace(&self, id: Option<NamespaceId>) -> bool {
        is_compatibility(id.and_then(|id| self.reader.namespace(id)))
    }

    /// A start tag outside alternate content.
    fn start(&mut self) -> Result<Step, Error> {
        let Some((namespace, local, offset)) = self.element() else {
            return Ok(Step::Nothing);
        };
        let unprocessed = matches!(
            self.frames.last(),
            Some(Frame {
                kind: FrameKind::Unprocessed,
                ..
            })
        );
        // Elements of the Markup Compatibility namespace are never extension elements (Part 3 §8), whatever the configuration says.
        let compatibility = self.is_compatibility_namespace(namespace);
        let view = namespace.and_then(|id| self.reader.namespace(id));
        if unprocessed || (!compatibility && self.config.is_extension_element(view, local)) {
            let understood = self.understands(namespace);
            self.unprocessed_kinds();
            self.push_frame(FrameKind::Unprocessed, self.undo.len());
            self.current = Current::Start {
                understood,
                extension: !unprocessed,
            };
            return Ok(Step::Event);
        }
        if compatibility {
            return match local {
                "AlternateContent" => self.alternate_content(),
                "Choice" => Err(compatibility_error(
                    offset,
                    CompatibilityError::MisplacedChoice,
                )),
                "Fallback" => Err(compatibility_error(
                    offset,
                    CompatibilityError::MisplacedFallback,
                )),
                _ => Err(compatibility_error(
                    offset,
                    CompatibilityError::UnknownElement,
                )),
            };
        }
        let mark = self.undo.len();
        let declarations = self.read_declarations()?;
        self.declare(&declarations)?;
        let understood = self.understands(namespace);
        if let Some(id) = namespace
            && !understood
            && self.is_ignorable(id)
        {
            if self.has_pair(Table::ProcessContent, id, local) {
                self.check_unwrapped()?;
                self.check_must_understand(&declarations)?;
                self.output_kinds();
                self.push_frame(FrameKind::Unwrapped, mark);
                self.current = Current::UnwrapStart { understood };
                return Ok(Step::Event);
            }
            let preserve = self.has_pair(Table::PreserveElements, id, local);
            self.undo_to(mark);
            let fragment = self.reader.capture()?;
            self.note_captured(&fragment);
            self.current = Current::Ignored(IgnoredElement { fragment, preserve });
            return Ok(Step::Event);
        }
        self.check_must_understand(&declarations)?;
        self.output_kinds();
        self.push_frame(FrameKind::Output, mark);
        self.current = Current::Start {
            understood,
            extension: false,
        };
        Ok(Step::Event)
    }

    /// The start tag of `mc:AlternateContent` outside alternate content.
    fn alternate_content(&mut self) -> Result<Step, Error> {
        let Some((element, _)) = self.reader.start_data() else {
            return Ok(Step::Nothing);
        };
        let start = element.span.start;
        let depth = element.depth;
        let name_end = 1 + element.qname.len();
        let mark = self.undo.len();
        let declarations = self.read_declarations()?;
        self.declare(&declarations)?;
        self.check_alternative_attributes(false)?;
        self.check_must_understand(&declarations)?;
        self.output_kinds();
        let context = self.reader.context();
        let mut uses = Uses::new();
        self.reader.note_uses(depth, &mut uses);
        self.alternates.push((depth, uses));
        self.push_frame(
            FrameKind::Alternate(Alternatives {
                start,
                name_end,
                context,
                choices: 0,
                fallback: false,
                selection: None,
            }),
            mark,
        );
        self.current = Current::AlternateStart;
        Ok(Step::Event)
    }

    /// A start tag directly inside `mc:AlternateContent`: an alternative, or an element that must be ignored.
    fn alternative(&mut self) -> Result<Step, Error> {
        let Some((namespace, local, offset)) = self.element() else {
            return Ok(Step::Nothing);
        };
        let mark = self.undo.len();
        let declarations = self.read_declarations()?;
        self.declare(&declarations)?;
        let choice = match (self.is_compatibility_namespace(namespace), local) {
            (true, "Choice") => true,
            (true, "Fallback") => false,
            (true, _) => {
                return Err(compatibility_error(
                    offset,
                    CompatibilityError::UnexpectedContent,
                ));
            }
            (false, _) => {
                // Only elements that are ignored may stand among the alternatives (Part 3 §7.5, §9.4).
                let view = namespace.and_then(|id| self.reader.namespace(id));
                let extension = self.config.is_extension_element(view, local);
                let ignored = !extension
                    && namespace.is_some_and(|id| {
                        !self.understands(Some(id))
                            && self.is_ignorable(id)
                            && !self.has_pair(Table::ProcessContent, id, local)
                    });
                if !ignored {
                    return Err(compatibility_error(
                        offset,
                        CompatibilityError::UnexpectedContent,
                    ));
                }
                self.undo_to(mark);
                let fragment = self.reader.capture()?;
                self.note_captured(&fragment);
                return Ok(Step::Nothing);
            }
        };
        let requires = self.check_alternative_attributes(choice)?;
        let understood = requires.iter().all(|&id| self.understands(Some(id)));
        let Some(Frame {
            kind: FrameKind::Alternate(alternatives),
            ..
        }) = self.frames.last_mut()
        else {
            return Ok(Step::Nothing);
        };
        let selection = if choice {
            if alternatives.fallback {
                return Err(compatibility_error(
                    offset,
                    CompatibilityError::MisplacedChoice,
                ));
            }
            let index = alternatives.choices;
            alternatives.choices += 1;
            (alternatives.selection.is_none() && understood).then_some(Selection::Choice(index))
        } else {
            if alternatives.fallback {
                return Err(compatibility_error(
                    offset,
                    CompatibilityError::MisplacedFallback,
                ));
            }
            alternatives.fallback = true;
            alternatives
                .selection
                .is_none()
                .then_some(Selection::Fallback)
        };
        if selection.is_some() {
            alternatives.selection = selection;
            self.check_must_understand(&declarations)?;
            self.push_frame(FrameKind::Selected, mark);
        } else {
            // Nothing inside an alternative that is not selected is processed (Part 3 §9.4).
            self.undo_to(mark);
            let fragment = self.reader.capture()?;
            self.note_captured(&fragment);
        }
        Ok(Step::Nothing)
    }

    /// An end tag.
    fn end(&mut self) -> Result<Step, Error> {
        let Some(frame) = self.frames.pop() else {
            return Ok(Step::Nothing);
        };
        self.undo_to(frame.undo);
        match frame.kind {
            FrameKind::Output | FrameKind::Unprocessed => {
                self.current = Current::End;
                Ok(Step::Event)
            }
            FrameKind::Unwrapped => {
                self.current = Current::UnwrapEnd;
                Ok(Step::Event)
            }
            FrameKind::Selected => Ok(Step::Nothing),
            FrameKind::Alternate(alternatives) => {
                if alternatives.choices == 0 {
                    return Err(compatibility_error(
                        alternatives.start,
                        CompatibilityError::MissingChoice,
                    ));
                }
                let end = match self.reader.event() {
                    Some(Event::End(tag)) => tag.span().end,
                    _ => return Ok(Step::Nothing),
                };
                let uses = self
                    .alternates
                    .pop()
                    .map(|(_, uses)| uses)
                    .unwrap_or_default();
                // What it uses from outside the enclosing alternate content, the enclosing one uses too.
                if let Some((depth, outer)) = self.alternates.last_mut() {
                    for (prefix, (uri, binding)) in &uses {
                        if *binding < *depth && !outer.contains_key(prefix) {
                            outer.insert(prefix.clone(), (uri.clone(), *binding));
                        }
                    }
                }
                let fragment = Fragment::from_parts(
                    Cow::Borrowed(self.reader.slice_of(&(alternatives.start..end))),
                    alternatives.context,
                    alternatives.name_end,
                    &uses,
                    false,
                    true,
                );
                self.current = Current::AlternateEnd(AlternateContent {
                    fragment,
                    selection: alternatives.selection.unwrap_or(Selection::Nothing),
                });
                Ok(Step::Event)
            }
        }
    }

    /// Adds what a fragment captured inside alternate content uses from outside it to what the innermost one uses.
    fn note_captured(&mut self, fragment: &Fragment<'a>) {
        let Some((depth, uses)) = self.alternates.last_mut() else {
            return;
        };
        for binding in fragment.uses.iter() {
            if binding.depth < *depth && !uses.contains_key(&binding.prefix) {
                uses.insert(binding.prefix.clone(), (binding.uri.clone(), binding.depth));
            }
        }
    }

    fn push_frame(&mut self, kind: FrameKind, undo: usize) {
        self.frames.push(Frame { kind, undo });
    }

    /// Closes the innermost frame without an event, taking back its declarations.
    fn pop_frame(&mut self) {
        if let Some(frame) = self.frames.pop() {
            self.undo_to(frame.undo);
        }
    }

    /// Whether names in the namespace `id` (`None` for no namespace) are understood: by the configuration, or because they are XML's or Markup Compatibility's own.
    fn understands(&mut self, id: Option<NamespaceId>) -> bool {
        let Some(id) = id else {
            return self.config.understands(None);
        };
        let Ok(index) = usize::try_from(id.0) else {
            return false;
        };
        if let Some(&Some(known)) = self.understood.get(index) {
            return known;
        }
        let namespace = self.reader.namespace(id);
        let understood =
            id == XML_ID || is_compatibility(namespace) || self.config.understands(namespace);
        if self.understood.len() <= index {
            self.understood.resize(index + 1, None);
        }
        if let Some(slot) = self.understood.get_mut(index) {
            *slot = Some(understood);
        }
        understood
    }

    fn is_ignorable(&self, id: NamespaceId) -> bool {
        usize::try_from(id.0)
            .ok()
            .and_then(|index| self.ignorable.get(index))
            .is_some_and(|&count| count > 0)
    }

    /// Whether a pair of `table` in scope names `local` (or `*`) in the namespace `id`.
    fn has_pair(&self, table: Table, id: NamespaceId, local: &str) -> bool {
        self.pairs
            .get(&(table, id))
            .is_some_and(|locals| locals.contains_key(local) || locals.contains_key("*"))
    }

    /// Takes back the declarations made after position `mark` of the log.
    fn undo_to(&mut self, mark: usize) {
        while self.undo.len() > mark {
            match self.undo.pop() {
                Some(Undo::Ignorable(id)) => {
                    if let Some(count) = usize::try_from(id.0)
                        .ok()
                        .and_then(|index| self.ignorable.get_mut(index))
                    {
                        *count = count.saturating_sub(1);
                    }
                }
                Some(Undo::Pair(table, id, local)) => {
                    if let Some(locals) = self.pairs.get_mut(&(table, id)) {
                        if let Some(count) = locals.get_mut(&local) {
                            *count = count.saturating_sub(1);
                            if *count == 0 {
                                locals.remove(&local);
                            }
                        }
                        if locals.is_empty() {
                            self.pairs.remove(&(table, id));
                        }
                    }
                }
                None => break,
            }
        }
    }

    /// Reads and checks the Markup Compatibility attributes of the current start tag (Part 3 §7.2–7.4; 2nd edition, §10.1.3): every list well-formed and within the limits, every prefix declared and not bound to the Markup Compatibility namespace.
    fn read_declarations(&self) -> Result<Declarations, Error> {
        let mut declarations = Declarations::default();
        let Some((_, attributes)) = self.reader.start_data() else {
            return Ok(declarations);
        };
        for attribute in attributes {
            if attribute.declaration || !self.is_compatibility_namespace(attribute.namespace) {
                continue;
            }
            let offset = attribute.span.start;
            let table = match attribute.local {
                "Ignorable" | "MustUnderstand" => None,
                "ProcessContent" => Some(Table::ProcessContent),
                "PreserveElements" => Some(Table::PreserveElements),
                "PreserveAttributes" => Some(Table::PreserveAttributes),
                _ => {
                    return Err(compatibility_error(
                        offset,
                        CompatibilityError::UnknownAttribute,
                    ));
                }
            };
            let tokens = self.tokens(&attribute.value, offset)?;
            for token in tokens {
                match table {
                    None => {
                        let id = self.resolve_list_prefix(token, offset)?;
                        if attribute.local == "Ignorable" {
                            declarations.ignorable.push(id);
                        } else {
                            declarations.must_understand.push((id, offset));
                        }
                    }
                    Some(table) => {
                        let Some((prefix, local)) = token.split_once(':') else {
                            return Err(compatibility_error(
                                offset,
                                CompatibilityError::InvalidList,
                            ));
                        };
                        if !(local == "*" || is_ncname(local)) {
                            return Err(compatibility_error(
                                offset,
                                CompatibilityError::InvalidList,
                            ));
                        }
                        let id = self.resolve_list_prefix(prefix, offset)?;
                        declarations
                            .pairs
                            .push((table, id, Box::from(local), offset));
                    }
                }
            }
        }
        Ok(declarations)
    }

    /// The tokens of a list attribute's value, once it is known to hold no more than the limit allows. Counting stops at the first token beyond the limit, and keeps nothing.
    fn tokens<'v>(&self, value: &'v str, offset: usize) -> Result<SplitAsciiWhitespace<'v>, Error> {
        let tokens = value.split_ascii_whitespace();
        if tokens
            .clone()
            .nth(self.limits.max_compatibility_tokens)
            .is_some()
        {
            return Err(Error::new(offset, ErrorKind::TooManyCompatibilityTokens));
        }
        Ok(tokens)
    }

    /// The namespace a prefix in a Markup Compatibility list is bound to.
    fn resolve_list_prefix(&self, prefix: &str, offset: usize) -> Result<NamespaceId, Error> {
        if !is_ncname(prefix) {
            return Err(compatibility_error(offset, CompatibilityError::InvalidList));
        }
        let Some(id) = self.reader.namespace_id_of(prefix) else {
            return Err(compatibility_error(
                offset,
                CompatibilityError::UndeclaredPrefix,
            ));
        };
        if self.is_compatibility_namespace(Some(id)) {
            return Err(compatibility_error(
                offset,
                CompatibilityError::CompatibilityNamespaceListed,
            ));
        }
        Ok(id)
    }

    /// Puts an element's declarations in scope: first its ignorable namespaces, then its pairs, whose namespaces must be ignorable on the element or an ancestor (Part 3 §7.3).
    fn declare(&mut self, declarations: &Declarations) -> Result<(), Error> {
        for &id in &declarations.ignorable {
            let Ok(index) = usize::try_from(id.0) else {
                continue;
            };
            if self.ignorable.len() <= index {
                self.ignorable.resize(index + 1, 0);
            }
            if let Some(count) = self.ignorable.get_mut(index) {
                *count = count.saturating_add(1);
            }
            self.undo.push(Undo::Ignorable(id));
        }
        for (table, id, local, offset) in &declarations.pairs {
            if !self.is_ignorable(*id) {
                return Err(compatibility_error(
                    *offset,
                    CompatibilityError::NotIgnorable,
                ));
            }
            let count = self
                .pairs
                .entry((*table, *id))
                .or_default()
                .entry(local.clone())
                .or_insert(0);
            *count = count.saturating_add(1);
            self.undo.push(Undo::Pair(*table, *id, local.clone()));
        }
        Ok(())
    }

    /// Fails if a namespace that the element's `mc:MustUnderstand` names is not understood (Part 3 §9.4).
    fn check_must_understand(&mut self, declarations: &Declarations) -> Result<(), Error> {
        for &(id, offset) in &declarations.must_understand {
            if !self.understands(Some(id)) {
                return Err(compatibility_error(
                    offset,
                    CompatibilityError::MustUnderstand,
                ));
            }
        }
        Ok(())
    }

    /// Fails if the current start tag, of an element whose content takes its place, has `xml:base`, `xml:lang` or `xml:space` (Part 3 §9.2).
    fn check_unwrapped(&self) -> Result<(), Error> {
        let Some((_, attributes)) = self.reader.start_data() else {
            return Ok(());
        };
        for attribute in attributes {
            if attribute.namespace == Some(XML_ID)
                && matches!(attribute.local, "base" | "lang" | "space")
            {
                return Err(compatibility_error(
                    attribute.span.start,
                    CompatibilityError::XmlAttributeOnUnwrappedElement,
                ));
            }
        }
        Ok(())
    }

    /// Checks the attributes of the current start tag, `mc:AlternateContent`, `mc:Choice` (`choice`) or `mc:Fallback`: no attribute in no namespace but `Requires` on `mc:Choice`, which it must have; none in the XML namespace; others only in the Markup Compatibility namespace or an ignorable one (Part 3 §7.1, §7.5–7.7). Returns the namespaces `Requires` lists.
    fn check_alternative_attributes(&self, choice: bool) -> Result<Vec<NamespaceId>, Error> {
        let Some((element, attributes)) = self.reader.start_data() else {
            return Ok(Vec::new());
        };
        let is_choice = choice;
        let mut requires = None;
        for attribute in attributes {
            if attribute.declaration {
                continue;
            }
            let offset = attribute.span.start;
            let unexpected = compatibility_error(offset, CompatibilityError::UnexpectedAttribute);
            match attribute.namespace {
                None if is_choice && attribute.local == "Requires" => {
                    let tokens = self.tokens(&attribute.value, offset)?;
                    if tokens.clone().next().is_none() {
                        return Err(compatibility_error(offset, CompatibilityError::InvalidList));
                    }
                    let mut ids = Vec::new();
                    for token in tokens {
                        ids.push(self.resolve_list_prefix(token, offset)?);
                    }
                    requires = Some(ids);
                }
                None => return Err(unexpected),
                Some(id) if id == XML_ID => return Err(unexpected),
                Some(id) if self.is_compatibility_namespace(Some(id)) => {}
                Some(id) => {
                    if !self.is_ignorable(id) {
                        return Err(unexpected);
                    }
                }
            }
        }
        match requires {
            Some(ids) => Ok(ids),
            None if is_choice => Err(compatibility_error(
                element.span.start,
                CompatibilityError::MissingRequires,
            )),
            None => Ok(Vec::new()),
        }
    }

    /// Sets the kinds of the current start tag's attributes for an element that Markup Compatibility processes.
    fn output_kinds(&mut self) {
        let mut kinds = std::mem::take(&mut self.kinds);
        kinds.clear();
        let mut attributes = std::mem::take(&mut self.scratch);
        attributes.clear();
        if let Some((_, data)) = self.reader.start_data() {
            attributes
                .extend(data.iter().map(|attribute| {
                    (attribute.namespace, attribute.local, attribute.declaration)
                }));
        }
        for &(namespace, local, declaration) in &attributes {
            let kind = if declaration {
                AttributeKind::NamespaceDeclaration
            } else if self.is_compatibility_namespace(namespace) {
                AttributeKind::Compatibility
            } else if self.understands(namespace) {
                AttributeKind::Output
            } else {
                match namespace {
                    Some(id) if self.is_ignorable(id) => AttributeKind::Ignored {
                        preserve: self.has_pair(Table::PreserveAttributes, id, local),
                    },
                    _ => AttributeKind::NotUnderstood,
                }
            };
            kinds.push(kind);
        }
        self.kinds = kinds;
        self.scratch = attributes;
    }

    /// Sets the kinds of the current start tag's attributes for an element inside an extension element, or one.
    fn unprocessed_kinds(&mut self) {
        self.kinds.clear();
        if let Some((_, attributes)) = self.reader.start_data() {
            self.kinds.extend(attributes.iter().map(|attribute| {
                if attribute.declaration {
                    AttributeKind::NamespaceDeclaration
                } else {
                    AttributeKind::Unprocessed
                }
            }));
        }
    }
}

fn compatibility_error(offset: usize, error: CompatibilityError) -> Error {
    Error::new(offset, ErrorKind::Compatibility(error))
}
