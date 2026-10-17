//! Markup Compatibility (AC-2): the processing model of ECMA-376 Part 3 (5th edition, 2015), checked with scenarios that re-create the standard's examples (each test names the clause), the preservation hints of the 2nd edition (2008), Word documents, and every error.
//!
//! The scenarios use their own names (`urn:t:…` namespaces, shapes instead of circles) but the structure, configurations and outcomes of the examples they cite. A scenario's input is processed with a configuration; the output document is what the events say is left (written with the writer, compared in canonical form), and the lossless events must recreate the input.

use bayan_xml::{
    AttributeKind, CompatibilityError, Error, ErrorKind, Fragment, KnownNamespace, Limits,
    McConfig, McEvent, McReader, NamespaceName, QName, Reader, Selection, Source, Writer,
    canonicalize,
};

use crate::common::first_difference;
use crate::documents::{SAMPLES, all_parts};

const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

/// `text` with `'MC'` standing for the Markup Compatibility namespace name.
fn document(text: &str) -> String {
    text.replace("'MC'", &format!("'{MC}'"))
}

/// A configuration that understands names in no namespace and `urn:t:<name>` for each of `names`.
fn understanding(names: &[&str]) -> McConfig {
    let mut config = McConfig::new().understand(NamespaceName::None);
    for name in names {
        config = config.understand(NamespaceName::from_uri(&format!("urn:t:{name}")));
    }
    config
}

fn fail(error: Error) -> (ErrorKind, usize) {
    (error.kind(), error.offset())
}

fn compatibility(error: CompatibilityError) -> ErrorKind {
    ErrorKind::Compatibility(error)
}

fn canonical(text: &str) -> String {
    let limits = Limits::default();
    canonicalize(&Source::new(text.as_bytes(), &limits).unwrap(), &limits).unwrap()
}

/// The output document of processing `text` with `config`: the elements, attributes and content that the events leave in it, written again.
fn output(text: &str, config: &McConfig) -> Result<String, (ErrorKind, usize)> {
    let limits = Limits::default();
    let text = document(text);
    let source = Source::new(text.as_bytes(), &limits).map_err(fail)?;
    let mut reader = McReader::new(&source, &limits, config);
    let mut writer = Writer::new();
    while let Some(event) = reader.next_event().map_err(fail)? {
        match event {
            McEvent::Start(tag) => {
                writer.start(QName::from_name(&tag.name())).unwrap();
                for (attribute, kind) in tag.attributes() {
                    let name = attribute.name();
                    match kind {
                        AttributeKind::NamespaceDeclaration => {
                            let prefix = if name.prefix().is_empty() {
                                ""
                            } else {
                                name.local()
                            };
                            writer.declare(prefix, attribute.value()).unwrap();
                        }
                        AttributeKind::Output
                        | AttributeKind::NotUnderstood
                        | AttributeKind::Unprocessed => {
                            writer
                                .attribute(QName::from_name(&name), attribute.value())
                                .unwrap();
                        }
                        AttributeKind::Ignored { .. } | AttributeKind::Compatibility => {}
                    }
                }
            }
            McEvent::End(_) => writer.end().unwrap(),
            McEvent::Text(text) => writer.text(text.value()).unwrap(),
            McEvent::CData(cdata) => writer.cdata(cdata.value()).unwrap(),
            McEvent::Comment(comment) => writer.comment(comment.text()).unwrap(),
            McEvent::ProcessingInstruction(pi) => writer
                .processing_instruction(pi.target(), pi.data())
                .unwrap(),
            McEvent::Ignored(_)
            | McEvent::UnwrapStart(_)
            | McEvent::UnwrapEnd(_)
            | McEvent::AlternateContentStart(_)
            | McEvent::AlternateContentEnd(_) => {}
        }
    }
    Ok(writer.finish().unwrap())
}

/// Checks that processing `text` with `config` leaves `expected`, compared in canonical form.
fn assert_output(text: &str, config: &McConfig, expected: &str) {
    let written = output(text, config).unwrap();
    assert_eq!(
        first_difference(&canonical(&written), &canonical(&document(expected))),
        None
    );
}

/// The error processing `text` with `config` stops at.
fn refusal(text: &str, config: &McConfig) -> (ErrorKind, usize) {
    match output(text, config) {
        Ok(_) => panic!("accepted a document that must be refused"),
        Err(error) => error,
    }
}

/// The offset of `needle` in `text` (after `'MC'` is expanded): where an error is expected.
fn at(text: &str, needle: &str) -> usize {
    document(text).find(needle).unwrap()
}

fn qualified(prefix: &str, local: &str) -> String {
    if prefix.is_empty() {
        local.to_owned()
    } else {
        format!("{prefix}:{local}")
    }
}

/// The name of the element a fragment holds, as written.
fn fragment_name(text: &str) -> &str {
    let end = text
        .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
        .unwrap();
    &text[1..end]
}

/// A compact description of the events of processing `text` with `config`: `<name a=kind …>` for start tags (declarations left out; `[extension]` and `[not understood]` as flags), `</name>`, `ignored name` (with `preserve`), `unwrap name` and `/unwrap name`, `alternate` and `/alternate <selection>`, and `text "…"`.
fn describe(text: &str, config: &McConfig) -> Result<Vec<String>, (ErrorKind, usize)> {
    let limits = Limits::default();
    let text = document(text);
    let source = Source::new(text.as_bytes(), &limits).map_err(fail)?;
    let mut reader = McReader::new(&source, &limits, config);
    let mut events = Vec::new();
    while let Some(event) = reader.next_event().map_err(fail)? {
        events.push(match event {
            McEvent::Start(tag) => {
                let name = tag.name();
                let mut description = format!("<{}", qualified(name.prefix(), name.local()));
                for (attribute, kind) in tag.attributes() {
                    let kind = match kind {
                        AttributeKind::NamespaceDeclaration => continue,
                        AttributeKind::Output => "output",
                        AttributeKind::NotUnderstood => "not-understood",
                        AttributeKind::Ignored { preserve: false } => "ignored",
                        AttributeKind::Ignored { preserve: true } => "preserved",
                        AttributeKind::Compatibility => "compatibility",
                        AttributeKind::Unprocessed => "unprocessed",
                    };
                    let name = attribute.name();
                    description.push_str(&format!(
                        " {}={kind}",
                        qualified(name.prefix(), name.local())
                    ));
                }
                if tag.is_extension() {
                    description.push_str(" [extension]");
                }
                if !tag.is_understood() {
                    description.push_str(" [not understood]");
                }
                description.push('>');
                description
            }
            McEvent::End(tag) => {
                format!("</{}>", qualified(tag.name().prefix(), tag.name().local()))
            }
            McEvent::Text(text) => format!("text {:?}", text.value()),
            McEvent::CData(cdata) => format!("cdata {:?}", cdata.value()),
            McEvent::Comment(comment) => format!("comment {:?}", comment.text()),
            McEvent::ProcessingInstruction(pi) => format!("pi {}", pi.target()),
            McEvent::Ignored(ignored) => {
                let name = fragment_name(ignored.fragment().text());
                if ignored.preserve() {
                    format!("ignored {name} preserve")
                } else {
                    format!("ignored {name}")
                }
            }
            McEvent::UnwrapStart(tag) => format!(
                "unwrap {}",
                qualified(tag.name().prefix(), tag.name().local())
            ),
            McEvent::UnwrapEnd(tag) => format!(
                "/unwrap {}",
                qualified(tag.name().prefix(), tag.name().local())
            ),
            McEvent::AlternateContentStart(_) => "alternate".to_owned(),
            McEvent::AlternateContentEnd(alternate) => match alternate.selection() {
                Selection::Choice(index) => format!("/alternate choice {index}"),
                Selection::Fallback => "/alternate fallback".to_owned(),
                Selection::Nothing => "/alternate nothing".to_owned(),
            },
        });
    }
    Ok(events)
}

/// The input written again from the lossless events: every start tag as read, ignored elements and whole alternate content from their fragments, and nothing of the interpretation inside alternate content.
fn recreate(text: &str, config: &McConfig) -> String {
    let limits = Limits::default();
    let source = Source::new(text.as_bytes(), &limits).unwrap();
    let mut reader = McReader::new(&source, &limits, config);
    let mut writer = Writer::new();
    if let Some(declaration) = source.declaration() {
        writer.declaration(declaration.standalone()).unwrap();
    }
    // How deep inside alternate content the events are: its content is written from its fragment.
    let mut alternate = 0usize;
    while let Some(event) = reader.next_event().unwrap() {
        match event {
            McEvent::AlternateContentStart(_) => alternate += 1,
            McEvent::AlternateContentEnd(content) => {
                alternate -= 1;
                if alternate == 0 {
                    writer.fragment(content.fragment()).unwrap();
                }
            }
            _ if alternate > 0 => {}
            McEvent::Start(tag) | McEvent::UnwrapStart(tag) => {
                writer.start_tag(tag.tag()).unwrap();
            }
            McEvent::End(_) | McEvent::UnwrapEnd(_) => writer.end().unwrap(),
            McEvent::Text(text) => writer.text(text.value()).unwrap(),
            McEvent::CData(cdata) => writer.cdata(cdata.value()).unwrap(),
            McEvent::Comment(comment) => writer.comment(comment.text()).unwrap(),
            McEvent::ProcessingInstruction(pi) => writer
                .processing_instruction(pi.target(), pi.data())
                .unwrap(),
            McEvent::Ignored(ignored) => writer.fragment(ignored.fragment()).unwrap(),
        }
    }
    writer.finish().unwrap()
}

// Part 3 §7.2 and Annex A.2.2: attributes in ignorable namespaces that the configuration does not understand are left out.
const CIRCLES_IGNORABLE: &str = "<shapes xmlns='urn:t:base' xmlns:mc='MC' xmlns:v2='urn:t:v2' xmlns:v3='urn:t:v3' mc:Ignorable='v2 v3'>\n  <shape size='4' v2:shade='0.3' v3:glow='7'/>\n</shapes>";

#[test]
fn leaves_out_ignorable_attributes_it_does_not_understand() {
    assert_output(
        CIRCLES_IGNORABLE,
        &understanding(&["base", "v2", "v3"]),
        "<shapes xmlns='urn:t:base' xmlns:v2='urn:t:v2' xmlns:v3='urn:t:v3'>\n  <shape size='4' v2:shade='0.3' v3:glow='7'/>\n</shapes>",
    );
    assert_output(
        CIRCLES_IGNORABLE,
        &understanding(&["base", "v2"]),
        "<shapes xmlns='urn:t:base' xmlns:v2='urn:t:v2'>\n  <shape size='4' v2:shade='0.3'/>\n</shapes>",
    );
    assert_output(
        CIRCLES_IGNORABLE,
        &understanding(&["base"]),
        "<shapes xmlns='urn:t:base'>\n  <shape size='4'/>\n</shapes>",
    );
    assert_eq!(
        describe(CIRCLES_IGNORABLE, &understanding(&["base", "v2"])).unwrap(),
        [
            "<shapes mc:Ignorable=compatibility>",
            "text \"\\n  \"",
            "<shape size=output v2:shade=output v3:glow=ignored>",
            "</shape>",
            "text \"\\n\"",
            "</shapes>",
        ]
    );
}

// Part 3 §7.3, §9.2 and Annex A.2.3: elements in ignorable namespaces are left out with their content, unless ProcessContent names them, which keeps their content in their place.
const CIRCLES_PROCESS_CONTENT: &str = "<shapes xmlns='urn:t:base' xmlns:mc='MC' xmlns:v2='urn:t:v2' mc:Ignorable='v2' mc:ProcessContent='v2:flash'><v2:overlay level='low'><shape size='1'/></v2:overlay><v2:flash><shape size='2'/></v2:flash></shapes>";

#[test]
fn keeps_the_content_of_elements_that_process_content_names() {
    assert_output(
        CIRCLES_PROCESS_CONTENT,
        &understanding(&["base", "v2"]),
        "<shapes xmlns='urn:t:base' xmlns:v2='urn:t:v2'><v2:overlay level='low'><shape size='1'/></v2:overlay><v2:flash><shape size='2'/></v2:flash></shapes>",
    );
    assert_output(
        CIRCLES_PROCESS_CONTENT,
        &understanding(&["base"]),
        "<shapes xmlns='urn:t:base'><shape size='2'/></shapes>",
    );
    assert_eq!(
        describe(CIRCLES_PROCESS_CONTENT, &understanding(&["base"])).unwrap(),
        [
            "<shapes mc:Ignorable=compatibility mc:ProcessContent=compatibility>",
            "ignored v2:overlay",
            "unwrap v2:flash",
            "<shape size=output>",
            "</shape>",
            "/unwrap v2:flash",
            "</shapes>",
        ]
    );
}

// Part 3 Annex A.2.4 and the 2nd edition's §10.1.1: names in a namespace that is neither understood nor ignorable are a mismatch, which the events report and the application decides about.
#[test]
fn reports_names_it_neither_understands_nor_may_ignore() {
    let text = "<shapes xmlns='urn:t:base' xmlns:mc='MC' xmlns:v2='urn:t:v2'><shape size='4' v2:shade='0.3'/><v2:shape/></shapes>";
    assert_eq!(
        describe(text, &understanding(&["base"])).unwrap(),
        [
            "<shapes>",
            "<shape size=output v2:shade=not-understood>",
            "</shape>",
            "<v2:shape [not understood]>",
            "</v2:shape>",
            "</shapes>",
        ]
    );
    assert_eq!(
        describe(text, &understanding(&["base", "v2"])).unwrap(),
        [
            "<shapes>",
            "<shape size=output v2:shade=output>",
            "</shape>",
            "<v2:shape>",
            "</v2:shape>",
            "</shapes>",
        ]
    );
    // Names in no namespace are understood only when the configuration says so (Part 3 §9.1).
    assert_eq!(
        describe(
            "<shape size='4'/>",
            &McConfig::new().understand(NamespaceName::from_uri("urn:t:base"))
        )
        .unwrap(),
        ["<shape size=not-understood [not understood]>", "</shape>"]
    );
}

// Part 3 §7.4, Annex A.2.5 and the 2nd edition's example 10-6: a namespace that must be understood and is not stops processing.
#[test]
fn stops_at_namespaces_that_must_be_understood() {
    let text = "<shapes xmlns='urn:t:base' xmlns:mc='MC' xmlns:v2='urn:t:v2' mc:MustUnderstand='v2'><shape size='4' v2:shade='0.3'/></shapes>";
    assert_eq!(
        refusal(text, &understanding(&["base"])),
        (
            compatibility(CompatibilityError::MustUnderstand),
            at(text, "mc:MustUnderstand")
        )
    );
    assert_output(
        text,
        &understanding(&["base", "v2"]),
        "<shapes xmlns='urn:t:base' xmlns:v2='urn:t:v2'><shape size='4' v2:shade='0.3'/></shapes>",
    );
}

// Part 3 §7.5–7.7, §9.3, §9.4 and Annex A.2.6: AlternateContent is replaced by the content of its first Choice whose required namespaces are all understood, else of its Fallback.
const CIRCLES_ALTERNATE: &str = "<shapes xmlns='urn:t:base' xmlns:mc='MC' xmlns:v2='urn:t:v2' xmlns:v3='urn:t:v3' mc:Ignorable='v2 v3'>\n<mc:AlternateContent>\n  <mc:Choice Requires='v3'><v3:shape size='3' shade='0.3' glow='7'/></mc:Choice>\n  <mc:Fallback><glowFilter glow='7'><shape size='3' v2:shade='0.3'/></glowFilter></mc:Fallback>\n</mc:AlternateContent>\n</shapes>";

#[test]
fn replaces_alternate_content_with_its_selected_choice() {
    assert_output(
        CIRCLES_ALTERNATE,
        &understanding(&["base", "v2", "v3"]),
        "<shapes xmlns='urn:t:base' xmlns:v3='urn:t:v3'>\n<v3:shape size='3' shade='0.3' glow='7'/>\n</shapes>",
    );
    assert_output(
        CIRCLES_ALTERNATE,
        &understanding(&["base", "v2"]),
        "<shapes xmlns='urn:t:base' xmlns:v2='urn:t:v2'>\n<glowFilter glow='7'><shape size='3' v2:shade='0.3'/></glowFilter>\n</shapes>",
    );
    assert_output(
        CIRCLES_ALTERNATE,
        &understanding(&["base"]),
        "<shapes xmlns='urn:t:base'>\n<glowFilter glow='7'><shape size='3'/></glowFilter>\n</shapes>",
    );
    assert_eq!(
        describe(CIRCLES_ALTERNATE, &understanding(&["base", "v2", "v3"])).unwrap(),
        [
            "<shapes mc:Ignorable=compatibility>",
            "text \"\\n\"",
            "alternate",
            "<v3:shape size=output shade=output glow=output>",
            "</v3:shape>",
            "/alternate choice 0",
            "text \"\\n\"",
            "</shapes>",
        ]
    );
}

// Part 3 §9.3: nested alternate content, selected at every level.
const NESTED_ALTERNATE: &str = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1' xmlns:n2='urn:t:n2' xmlns:n3='urn:t:n3'><mc:AlternateContent><mc:Choice Requires='n1 n2'><mc:AlternateContent><mc:Choice Requires='n3'><c1-1/></mc:Choice><mc:Fallback><f1-1/></mc:Fallback></mc:AlternateContent></mc:Choice><mc:Choice Requires='n1'><mc:AlternateContent><mc:Choice Requires='n3'><c2-1/></mc:Choice><mc:Fallback><f2-1/></mc:Fallback></mc:AlternateContent></mc:Choice><mc:Fallback><f1/></mc:Fallback></mc:AlternateContent></root>";

#[test]
fn selects_nested_alternate_content_at_every_level() {
    let cases: [(&[&str], &str, [&str; 2]); 4] = [
        (&["n1", "n2", "n3"], "c1-1", ["choice 0", "choice 0"]),
        (&["n1", "n2"], "f1-1", ["fallback", "choice 0"]),
        (&["n1"], "f2-1", ["fallback", "choice 1"]),
        (&[], "f1", ["", "fallback"]),
    ];
    for (understood, kept, selections) in cases {
        let mut names = vec!["base"];
        names.extend_from_slice(understood);
        let config = understanding(&names);
        assert_output(
            NESTED_ALTERNATE,
            &config,
            &format!("<root xmlns='urn:t:base'><{kept}/></root>"),
        );
        let ends: Vec<String> = describe(NESTED_ALTERNATE, &config)
            .unwrap()
            .into_iter()
            .filter(|event| event.starts_with("/alternate"))
            .collect();
        let expected: Vec<String> = selections
            .iter()
            .filter(|selection| !selection.is_empty())
            .map(|selection| format!("/alternate {selection}"))
            .collect();
        assert_eq!(ends, expected, "{understood:?}");
    }
}

// Part 3 §9.4: ignored elements, unwrapped elements and alternate content together, with three configurations.
const SECTION_9_4: &str = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:p='urn:t:p' xmlns:q='urn:t:q' mc:Ignorable='p q' mc:ProcessContent='p:open'><mc:AlternateContent><mc:Choice Requires='p'><p:a/><q:b><mc:AlternateContent><mc:Choice Requires='q'><choice-1-1/></mc:Choice><mc:Fallback><fallback-1-1/></mc:Fallback></mc:AlternateContent></q:b></mc:Choice><mc:Choice Requires='q'><q:b/><p:open><mc:AlternateContent><mc:Choice Requires='p'><choice-2-1/></mc:Choice><mc:Fallback><fallback-2-1/></mc:Fallback></mc:AlternateContent></p:open></mc:Choice></mc:AlternateContent></root>";

#[test]
fn combines_ignoring_unwrapping_and_selecting() {
    assert_output(
        SECTION_9_4,
        &understanding(&["base", "p"]),
        "<root xmlns='urn:t:base' xmlns:p='urn:t:p'><p:a/></root>",
    );
    assert_output(
        SECTION_9_4,
        &understanding(&["base", "q"]),
        "<root xmlns='urn:t:base' xmlns:q='urn:t:q'><q:b/><fallback-2-1/></root>",
    );
    assert_output(
        SECTION_9_4,
        &understanding(&["base", "p", "q"]),
        "<root xmlns='urn:t:base' xmlns:p='urn:t:p' xmlns:q='urn:t:q'><p:a/><q:b><choice-1-1/></q:b></root>",
    );
}

// Part 3 §8 and §9.2: an application-defined extension element is never ignored or unwrapped, and Markup Compatibility leaves everything inside it as it is.
const SECTION_9_2: &str = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i='urn:t:i' mc:Ignorable='i' mc:ProcessContent='i:open' i:note='x'><i:skip><kept-1/></i:skip><i:open><kept-2/></i:open><i:ext i:flag='y'><i:inner/></i:ext></root>";

#[test]
fn leaves_extension_elements_as_they_are() {
    let config =
        understanding(&["base"]).extension_element(NamespaceName::from_uri("urn:t:i"), "ext");
    assert_eq!(
        describe(SECTION_9_2, &config).unwrap(),
        [
            "<root mc:Ignorable=compatibility mc:ProcessContent=compatibility i:note=ignored>",
            "ignored i:skip",
            "unwrap i:open",
            "<kept-2>",
            "</kept-2>",
            "/unwrap i:open",
            "<i:ext i:flag=unprocessed [extension] [not understood]>",
            "<i:inner [not understood]>",
            "</i:inner>",
            "</i:ext>",
            "</root>",
        ]
    );
    assert_output(
        SECTION_9_2,
        &config,
        "<root xmlns='urn:t:base' xmlns:i='urn:t:i'><kept-2/><i:ext i:flag='y'><i:inner/></i:ext></root>",
    );
}

// Part 3 §8: elements of the Markup Compatibility namespace are never extension elements.
#[test]
fn never_takes_compatibility_elements_for_extension_elements() {
    let config =
        understanding(&["base"]).extension_element(NamespaceName::from_uri(MC), "AlternateContent");
    assert_output(
        CIRCLES_ALTERNATE,
        &config,
        "<shapes xmlns='urn:t:base'>\n<glowFilter glow='7'><shape size='3'/></glowFilter>\n</shapes>",
    );
}

// Part 3 §8, both examples: inside an extension element, names nobody understands are no error, and Markup Compatibility elements and attributes stay in the output.
#[test]
fn passes_markup_compatibility_through_extension_elements() {
    let config = understanding(&["base"])
        .extension_element(NamespaceName::from_uri("urn:t:n1"), "extension");
    let unknown = "<root xmlns='urn:t:base' xmlns:n1='urn:t:n1' xmlns:unknown='urn:t:unknown'><n1:extension><unknown:foo/></n1:extension></root>";
    assert_output(unknown, &config, unknown);
    let config = understanding(&["base"])
        .extension_element(NamespaceName::from_uri("urn:t:base"), "extension");
    let inside = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i1='urn:t:i1' xmlns:n1='urn:t:n1'><extension><foo1 mc:Ignorable='i1' mc:ProcessContent='i1:bar1' mc:MustUnderstand='n1'/><mc:AlternateContent mc:Ignorable='i1'><mc:Choice Requires='n1'/><mc:Fallback i1:foo=''/></mc:AlternateContent></extension></root>";
    assert_output(inside, &config, inside);
}

// Part 3 Annex A.1.2: Ignorable names namespaces, whatever prefixes are bound to them.
#[test]
fn treats_ignorable_namespaces_by_name_not_prefix() {
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC'><foo mc:Ignorable='i1' xmlns:i1='urn:t:i1' xmlns:i2='urn:t:i2'><bar mc:Ignorable='i2'><i1:x/><i2:y/></bar><bar mc:Ignorable='i1 i2'><i1:x/><i2:y/></bar><bar mc:Ignorable='i1alias i2' xmlns:i1alias='urn:t:i1'><i1:x/><i2:y/></bar></foo></root>";
    assert_output(
        text,
        &understanding(&["base"]),
        "<root xmlns='urn:t:base'><foo><bar/><bar/><bar/></foo></root>",
    );
}

// Part 3 Annex A.1.4 and the 2nd edition's example 10-4: ProcessContent names an expanded name, whatever prefix the element uses.
#[test]
fn matches_process_content_by_expanded_name() {
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1' xmlns:n1alias='urn:t:n1' mc:Ignorable='n1alias' mc:ProcessContent='n1:foo'><n1alias:foo><bar/></n1alias:foo><n1:foo><bar/></n1:foo><n1:other><bar/></n1:other></root>";
    assert_output(
        text,
        &understanding(&["base"]),
        "<root xmlns='urn:t:base'><bar/><bar/></root>",
    );
    // A wildcard names every element of the namespace.
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1' mc:Ignorable='n1' mc:ProcessContent='n1:*'><n1:foo><bar/></n1:foo><n1:other><baz/></n1:other></root>";
    assert_output(
        text,
        &understanding(&["base"]),
        "<root xmlns='urn:t:base'><bar/><baz/></root>",
    );
}

// Part 3 §9.2: an element can declare its own namespace ignorable, and Ignorable means nothing for namespaces that are understood.
#[test]
fn applies_ignorable_to_the_element_that_declares_it() {
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC'><x:e xmlns:x='urn:t:x' mc:Ignorable='x'><kept/></x:e><kept mc:Ignorable='b' xmlns:b='urn:t:base'><b:child/></kept></root>";
    assert_output(
        text,
        &understanding(&["base"]),
        "<root xmlns='urn:t:base'><kept><child/></kept></root>",
    );
}

// Part 3 §9.2: what an element's Markup Compatibility attributes declare ends with the element, whether it is ignored or not.
#[test]
fn ends_declarations_with_their_element() {
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:x='urn:t:x' xmlns:y='urn:t:y'><x:e mc:Ignorable='x y'/><y:f/><a mc:Ignorable='y' mc:ProcessContent='y:g' mc:PreserveElements='y:h'><y:g/></a><y:g/><y:h/></root>";
    assert_eq!(
        describe(text, &understanding(&["base"])).unwrap(),
        [
            "<root>",
            "ignored x:e",
            "<y:f [not understood]>",
            "</y:f>",
            "<a mc:Ignorable=compatibility mc:ProcessContent=compatibility mc:PreserveElements=compatibility>",
            "unwrap y:g",
            "/unwrap y:g",
            "</a>",
            "<y:g [not understood]>",
            "</y:g>",
            "<y:h [not understood]>",
            "</y:h>",
            "</root>",
        ]
    );
}

// Part 3 Annex A.1.7: an ignorable element may stand among the alternatives, for future versions of Markup Compatibility.
#[test]
fn accepts_ignorable_elements_among_alternatives() {
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:mc2='urn:t:mc2' xmlns:n1='urn:t:n1'><mc:AlternateContent mc:Ignorable='mc2' mc2:foo=''><mc2:NewChoice><a/></mc2:NewChoice><mc:Choice Requires='n1'><b/></mc:Choice><mc:Fallback><c/></mc:Fallback></mc:AlternateContent></root>";
    assert_output(
        text,
        &understanding(&["base", "n1"]),
        "<root xmlns='urn:t:base'><b/></root>",
    );
    assert_output(
        text,
        &understanding(&["base"]),
        "<root xmlns='urn:t:base'><c/></root>",
    );
}

// Part 3 Annex A.2.7: an application can process the content of an extension element again, with a configuration of its own.
#[test]
fn processes_the_content_of_extension_elements_again() {
    let text = "<sheet xmlns='urn:t:sheet' xmlns:mc='MC'><extLst><ext uri='u' xmlns:co1='urn:t:co1'><co1:sound mc:Ignorable='co2' xmlns:co2='urn:t:co2'><co1:file>a.wav</co1:file><co2:video>a.mp4</co2:video></co1:sound></ext></extLst></sheet>";
    let text = document(text);
    let limits = Limits::default();
    let source = Source::new(text.as_bytes(), &limits).unwrap();
    let outer = understanding(&["sheet"])
        .extension_element(NamespaceName::from_uri("urn:t:sheet"), "extLst");
    let mut reader = McReader::new(&source, &limits, &outer);
    let mut found = None;
    loop {
        let extension = match reader.next_event().unwrap() {
            None => break,
            Some(McEvent::Start(tag)) => tag.is_extension(),
            Some(_) => false,
        };
        if extension {
            found = Some(reader.capture().unwrap());
        }
    }
    let fragment = found.unwrap();
    // The outer processing left the extension as it was.
    assert!(fragment.text().contains("co2:video"));
    let inner = understanding(&["sheet", "co1"]);
    let source = Source::new(fragment.text().as_bytes(), &limits).unwrap();
    let mut reader = McReader::with_context(&source, &limits, &inner, fragment.context()).unwrap();
    let mut kept = Vec::new();
    while let Some(event) = reader.next_event().unwrap() {
        match event {
            McEvent::Start(tag) => kept.push(tag.name().local().to_owned()),
            McEvent::Ignored(ignored) => {
                kept.push(format!(
                    "ignored {}",
                    fragment_name(ignored.fragment().text())
                ));
            }
            _ => {}
        }
    }
    assert_eq!(
        kept,
        ["extLst", "ext", "sound", "file", "ignored co2:video"]
    );
}

// The 2nd edition's §10.1.3 and example 10-1: PreserveAttributes and PreserveElements suggest which ignored attributes and elements an editor keeps.
#[test]
fn reports_what_the_document_suggests_preserving() {
    let text = "<shapes xmlns='urn:t:base' xmlns:mc='MC' xmlns:v2='urn:t:v2' xmlns:v3='urn:t:v3' mc:Ignorable='v2 v3' mc:PreserveAttributes='v3:glow' mc:PreserveElements='v2:overlay'><shape v2:shade='0.3' v3:glow='13'/><v2:overlay/><v2:other/></shapes>";
    assert_eq!(
        describe(text, &understanding(&["base"])).unwrap(),
        [
            "<shapes mc:Ignorable=compatibility mc:PreserveAttributes=compatibility mc:PreserveElements=compatibility>",
            "<shape v2:shade=ignored v3:glow=preserved>",
            "</shape>",
            "ignored v2:overlay preserve",
            "ignored v2:other",
            "</shapes>",
        ]
    );
    let text = "<shapes xmlns='urn:t:base' xmlns:mc='MC' xmlns:v2='urn:t:v2' mc:Ignorable='v2' mc:PreserveAttributes='v2:*' mc:PreserveElements='v2:*'><shape v2:shade='0.3'/><v2:overlay/></shapes>";
    assert_eq!(
        describe(text, &understanding(&["base"])).unwrap(),
        [
            "<shapes mc:Ignorable=compatibility mc:PreserveAttributes=compatibility mc:PreserveElements=compatibility>",
            "<shape v2:shade=preserved>",
            "</shape>",
            "ignored v2:overlay preserve",
            "</shapes>",
        ]
    );
}

#[test]
fn reports_the_selected_alternative_and_keeps_the_whole_element() {
    let text = document(CIRCLES_ALTERNATE);
    let limits = Limits::default();
    let source = Source::new(text.as_bytes(), &limits).unwrap();
    let mut reader = McReader::new(&source, &limits, &understanding(&["base"]));
    let mut found = Vec::new();
    while let Some(event) = reader.next_event().unwrap() {
        if let McEvent::AlternateContentEnd(alternate) = event {
            found.push((
                alternate.selection(),
                alternate.fragment().text().to_owned(),
            ));
        }
    }
    let start = text.find("<mc:AlternateContent>").unwrap();
    let end = text.find("</mc:AlternateContent>").unwrap() + "</mc:AlternateContent>".len();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, Selection::Fallback);
    assert_eq!(first_difference(&found[0].1, &text[start..end]), None);
}

#[test]
fn selects_nothing_without_a_fallback() {
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent><mc:Choice Requires='n1'><a/></mc:Choice></mc:AlternateContent></root>";
    assert_output(
        text,
        &understanding(&["base"]),
        "<root xmlns='urn:t:base'/>",
    );
    assert_eq!(
        describe(text, &understanding(&["base"])).unwrap(),
        ["<root>", "alternate", "/alternate nothing", "</root>"]
    );
    // A Choice is selected only when every namespace it requires is understood.
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1' xmlns:n2='urn:t:n2'><mc:AlternateContent><mc:Choice Requires=' n1  n2 '><a/></mc:Choice><mc:Choice Requires='n2'><b/></mc:Choice></mc:AlternateContent></root>";
    assert_output(
        text,
        &understanding(&["base", "n1"]),
        "<root xmlns='urn:t:base'/>",
    );
    assert_output(
        text,
        &understanding(&["base", "n2"]),
        "<root xmlns='urn:t:base'><b/></root>",
    );
    assert_output(
        text,
        &understanding(&["base", "n1", "n2"]),
        "<root xmlns='urn:t:base'><a/></root>",
    );
}

// Part 3 §9.4: alternate content can be the root element, and the root element can be ignored.
#[test]
fn processes_the_root_element_like_any_other() {
    let text = "<mc:AlternateContent xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:Choice Requires='n1'><a/></mc:Choice><mc:Fallback><b/></mc:Fallback></mc:AlternateContent>";
    assert_eq!(
        describe(text, &understanding(&[])).unwrap(),
        ["alternate", "<b>", "</b>", "/alternate fallback"]
    );
    let text = "<x:root xmlns:x='urn:t:x' xmlns:mc='MC' mc:Ignorable='x'><a/></x:root>";
    assert_eq!(
        describe(text, &understanding(&[])).unwrap(),
        ["ignored x:root"]
    );
}

// Part 3 §9.4 and the 2nd edition's §10.2: nothing inside an alternative that is not selected, an ignored element or an extension element counts, not even a namespace that must be understood.
#[test]
fn checks_only_what_processing_reaches() {
    let config = understanding(&["base", "n1"])
        .extension_element(NamespaceName::from_uri("urn:t:base"), "ext");
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1' xmlns:n2='urn:t:n2' xmlns:i='urn:t:i' mc:Ignorable='i'><mc:AlternateContent><mc:Choice Requires='n1'><a/></mc:Choice><mc:Choice Requires='n2'><b mc:MustUnderstand='n2'/><mc:Unknown/></mc:Choice><mc:Fallback><c mc:MustUnderstand='n2'/></mc:Fallback></mc:AlternateContent><i:x><d mc:MustUnderstand='n2'/></i:x><ext><e mc:MustUnderstand='n2'/></ext></root>";
    assert_output(
        text,
        &config,
        "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n2='urn:t:n2'><a/><ext><e mc:MustUnderstand='n2'/></ext></root>",
    );
    // On the selected alternative, and on alternate content itself, it counts.
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n2='urn:t:n2'><mc:AlternateContent><mc:Choice Requires='n2'/><mc:Fallback mc:MustUnderstand='n2'/></mc:AlternateContent></root>";
    assert_eq!(
        refusal(text, &understanding(&["base"])),
        (
            compatibility(CompatibilityError::MustUnderstand),
            at(text, "mc:MustUnderstand")
        )
    );
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n2='urn:t:n2'><mc:AlternateContent mc:MustUnderstand='n2'><mc:Choice Requires='n2'/></mc:AlternateContent></root>";
    assert_eq!(
        refusal(text, &understanding(&["base"])),
        (
            compatibility(CompatibilityError::MustUnderstand),
            at(text, "mc:MustUnderstand")
        )
    );
    // The attributes of every alternative are checked, selected or not (2nd edition §10.2.1).
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent><mc:Choice Requires='n1'/><mc:Fallback bad='1'/></mc:AlternateContent></root>";
    assert_eq!(
        refusal(text, &understanding(&["base", "n1"])),
        (
            compatibility(CompatibilityError::UnexpectedAttribute),
            at(text, "bad=")
        )
    );
}

// Part 3 Annex A.1.3 and A.1.6: every prefix that a Markup Compatibility attribute lists must be declared.
#[test]
fn refuses_undeclared_prefixes_in_lists() {
    let config = understanding(&["base"]);
    for (text, attribute) in [
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC'><foo1 mc:Ignorable='i1'><foo2 xmlns:i1='urn:t:i1'/></foo1></root>",
            "mc:Ignorable",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><foo mc:MustUnderstand='n1 n2'/></root>",
            "mc:MustUnderstand",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i='urn:t:i' mc:Ignorable='i'><foo mc:ProcessContent='j:x'/></root>",
            "mc:ProcessContent",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC'><mc:AlternateContent><mc:Choice Requires='z'/></mc:AlternateContent></root>",
            "Requires",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC'><foo mc:PreserveElements='j:x'/></root>",
            "mc:PreserveElements",
        ),
    ] {
        assert_eq!(
            refusal(text, &config),
            (
                compatibility(CompatibilityError::UndeclaredPrefix),
                at(text, attribute)
            ),
            "{attribute}"
        );
    }
    // The default namespace has no prefix to list.
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC'><foo mc:Ignorable='xmlns'/></root>";
    assert_eq!(
        refusal(text, &config),
        (
            compatibility(CompatibilityError::UndeclaredPrefix),
            at(text, "mc:Ignorable")
        )
    );
}

// Part 3 §7.3, Annex A.1.5, and the 2nd edition's §10.1.3: ProcessContent, PreserveElements and PreserveAttributes may name only ignorable namespaces.
#[test]
fn refuses_process_content_outside_ignorable_namespaces() {
    let config = understanding(&["base"]);
    for (text, attribute) in [
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC'><foo1 xmlns:i2='urn:t:i2'><foo2 mc:ProcessContent='i2:*'/></foo1></root>",
            "mc:ProcessContent",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i2='urn:t:i2'><foo mc:PreserveElements='i2:x'/></root>",
            "mc:PreserveElements",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i2='urn:t:i2'><foo mc:PreserveAttributes='i2:x'/></root>",
            "mc:PreserveAttributes",
        ),
    ] {
        assert_eq!(
            refusal(text, &config),
            (
                compatibility(CompatibilityError::NotIgnorable),
                at(text, attribute)
            ),
            "{attribute}"
        );
    }
    // Declared ignorable on the same element, wherever the attributes stand, they are fine.
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i2='urn:t:i2'><foo mc:ProcessContent='i2:x' mc:Ignorable='i2'/></root>";
    assert!(output(text, &config).is_ok());
}

#[test]
fn refuses_the_compatibility_namespace_in_lists() {
    let config = understanding(&["base"]);
    for (text, attribute) in [
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' mc:Ignorable='mc'/>",
            "mc:Ignorable",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' mc:MustUnderstand='mc'/>",
            "mc:MustUnderstand",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' mc:ProcessContent='mc:Choice'/>",
            "mc:ProcessContent",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC'><mc:AlternateContent><mc:Choice Requires='mc'/></mc:AlternateContent></root>",
            "Requires",
        ),
    ] {
        assert_eq!(
            refusal(text, &config),
            (
                compatibility(CompatibilityError::CompatibilityNamespaceListed),
                at(text, attribute)
            ),
            "{attribute}"
        );
    }
}

#[test]
fn refuses_malformed_lists() {
    let config = understanding(&["base"]);
    for (text, attribute) in [
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i='urn:t:i' mc:Ignorable='i' mc:ProcessContent='foo'/>",
            "mc:ProcessContent",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i='urn:t:i' mc:Ignorable='i' mc:ProcessContent='i:a:b'/>",
            "mc:ProcessContent",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i='urn:t:i' mc:Ignorable='i' mc:PreserveElements='i:'/>",
            "mc:PreserveElements",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i='urn:t:i' mc:Ignorable='i:x'/>",
            "mc:Ignorable",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i='urn:t:i' mc:MustUnderstand='1i'/>",
            "mc:MustUnderstand",
        ),
        // Requires lists at least one prefix (Part 3 §7.6).
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC'><mc:AlternateContent><mc:Choice Requires=' '/></mc:AlternateContent></root>",
            "Requires",
        ),
    ] {
        assert_eq!(
            refusal(text, &config),
            (
                compatibility(CompatibilityError::InvalidList),
                at(text, attribute)
            ),
            "{text}"
        );
    }
    // Empty lists are allowed everywhere else (Part 3 §7.2–7.4).
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' mc:Ignorable='' mc:ProcessContent=' ' mc:MustUnderstand='\t'/>";
    assert!(output(text, &config).is_ok());
}

#[test]
fn refuses_unknown_compatibility_elements_and_attributes() {
    let config = understanding(&["base"]);
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC'><mc:Choose/></root>";
    assert_eq!(
        refusal(text, &config),
        (
            compatibility(CompatibilityError::UnknownElement),
            at(text, "<mc:Choose")
        )
    );
    // Requires belongs to no namespace (2nd edition §10.2.2).
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n='urn:t:n'><mc:AlternateContent><mc:Choice mc:Requires='n'/></mc:AlternateContent></root>";
    assert_eq!(
        refusal(text, &config),
        (
            compatibility(CompatibilityError::UnknownAttribute),
            at(text, "mc:Requires")
        )
    );
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' mc:Other=''/>";
    assert_eq!(
        refusal(text, &config),
        (
            compatibility(CompatibilityError::UnknownAttribute),
            at(text, "mc:Other")
        )
    );
}

#[test]
fn refuses_misplaced_alternatives() {
    let config = understanding(&["base"]);
    for (text, element, error) in [
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n='urn:t:n'><mc:Choice Requires='n'/></root>",
            "<mc:Choice",
            CompatibilityError::MisplacedChoice,
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n='urn:t:n'><mc:AlternateContent><mc:Fallback/><mc:Choice Requires='n'/></mc:AlternateContent></root>",
            "<mc:Choice",
            CompatibilityError::MisplacedChoice,
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC'><mc:Fallback/></root>",
            "<mc:Fallback",
            CompatibilityError::MisplacedFallback,
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n='urn:t:n'><mc:AlternateContent><mc:Choice Requires='n'/><mc:Fallback/><mc:Fallback/></mc:AlternateContent></root>",
            "<mc:Fallback/></mc:Alt",
            CompatibilityError::MisplacedFallback,
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n='urn:t:n'><mc:AlternateContent><mc:AlternateContent><mc:Choice Requires='n'/></mc:AlternateContent></mc:AlternateContent></root>",
            "<mc:AlternateContent><mc:Choice",
            CompatibilityError::UnexpectedContent,
        ),
    ] {
        assert_eq!(
            refusal(text, &config),
            (compatibility(error), at(text, element)),
            "{text}"
        );
    }
}

#[test]
fn refuses_alternate_content_without_choice() {
    let config = understanding(&["base"]);
    for text in [
        "<root xmlns='urn:t:base' xmlns:mc='MC'><mc:AlternateContent/></root>",
        "<root xmlns='urn:t:base' xmlns:mc='MC'><mc:AlternateContent><mc:Fallback><a/></mc:Fallback></mc:AlternateContent></root>",
    ] {
        assert_eq!(
            refusal(text, &config),
            (
                compatibility(CompatibilityError::MissingChoice),
                at(text, "<mc:AlternateContent")
            ),
            "{text}"
        );
    }
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC'><mc:AlternateContent><mc:Choice/></mc:AlternateContent></root>";
    assert_eq!(
        refusal(text, &config),
        (
            compatibility(CompatibilityError::MissingRequires),
            at(text, "<mc:Choice")
        )
    );
}

// Part 3 §7.1 and §7.5–7.7: the attributes that alternate content, Choice and Fallback may have.
#[test]
fn refuses_unexpected_attributes_on_alternatives() {
    let config = understanding(&["base", "n1"]);
    for (text, attribute) in [
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent bad=''><mc:Choice Requires='n1'/></mc:AlternateContent></root>",
            "bad=",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent><mc:Choice Requires='n1' bad=''/></mc:AlternateContent></root>",
            "bad=",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent><mc:Choice Requires='n1'/><mc:Fallback bad=''/></mc:AlternateContent></root>",
            "bad=",
        ),
        // A qualified attribute must be in an ignorable namespace, even an understood one (Part 3 §7.6).
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent><mc:Choice Requires='n1' n1:foo=''/></mc:AlternateContent></root>",
            "n1:foo",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent xml:lang='en'><mc:Choice Requires='n1'/></mc:AlternateContent></root>",
            "xml:lang",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent><mc:Choice Requires='n1' xml:space='preserve'/></mc:AlternateContent></root>",
            "xml:space",
        ),
        // Even where the XML namespace is declared ignorable.
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent mc:Ignorable='xml' xml:lang='en'><mc:Choice Requires='n1'/></mc:AlternateContent></root>",
            "xml:lang",
        ),
    ] {
        assert_eq!(
            refusal(text, &config),
            (
                compatibility(CompatibilityError::UnexpectedAttribute),
                at(text, attribute)
            ),
            "{text}"
        );
    }
    // Part 3 §7.6 and §7.7: attributes in ignorable namespaces are fine.
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i1='urn:t:i1' xmlns:n1='urn:t:n1'><mc:AlternateContent mc:Ignorable='i1' i1:a=''><mc:Choice Requires='n1' i1:foo=''/><mc:Fallback i1:foo=''/></mc:AlternateContent></root>";
    assert!(output(text, &config).is_ok());
}

// Part 3 §7.5 and §9.4: alternate content holds alternatives and ignored elements only.
#[test]
fn refuses_unexpected_content_in_alternate_content() {
    let config = understanding(&["base", "n1", "u"]);
    for (text, offending) in [
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent><other/><mc:Choice Requires='n1'/></mc:AlternateContent></root>",
            "<other",
        ),
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent>words<mc:Choice Requires='n1'/></mc:AlternateContent></root>",
            "words",
        ),
        // Ignorable but understood, so not ignored.
        (
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1' xmlns:u='urn:t:u' mc:Ignorable='u'><mc:AlternateContent><u:x/><mc:Choice Requires='n1'/></mc:AlternateContent></root>",
            "<u:x",
        ),
    ] {
        assert_eq!(
            refusal(text, &config),
            (
                compatibility(CompatibilityError::UnexpectedContent),
                at(text, offending)
            ),
            "{text}"
        );
    }
    // White space, comments and processing instructions are fine, and are not part of the output: only the selected alternative's content is.
    let text = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:n1='urn:t:n1'><mc:AlternateContent>\n <!-- c --><?p?>\n<mc:Choice Requires='n1'><!-- kept --></mc:Choice></mc:AlternateContent></root>";
    assert_output(
        text,
        &config,
        "<root xmlns='urn:t:base'><!-- kept --></root>",
    );
}

// Part 3 §9.2: an element whose content takes its place may not carry xml:base, xml:lang or xml:space.
#[test]
fn refuses_xml_attributes_on_unwrapped_elements() {
    let config = understanding(&["base"]);
    for attribute in ["xml:space='preserve'", "xml:lang='en'", "xml:base='a/'"] {
        let text = format!(
            "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i='urn:t:i' mc:Ignorable='i' mc:ProcessContent='i:x'><i:x {attribute}/></root>"
        );
        assert_eq!(
            refusal(&text, &config),
            (
                compatibility(CompatibilityError::XmlAttributeOnUnwrappedElement),
                at(&text, attribute)
            ),
            "{attribute}"
        );
    }
}

#[test]
fn limits_the_length_of_lists() {
    let limits = Limits {
        max_compatibility_tokens: 3,
        ..Limits::default()
    };
    let config = understanding(&["base", "a", "b", "c", "d"]);
    let run = |text: &str| {
        let text = document(text);
        let source = Source::new(text.as_bytes(), &limits).unwrap();
        let mut reader = McReader::new(&source, &limits, &config);
        loop {
            match reader.next_event() {
                Ok(Some(_)) => {}
                Ok(None) => return Ok(()),
                Err(error) => return Err(fail(error)),
            }
        }
    };
    let declarations = "xmlns='urn:t:base' xmlns:mc='MC' xmlns:a='urn:t:a' xmlns:b='urn:t:b' xmlns:c='urn:t:c' xmlns:d='urn:t:d'";
    // Every list, with as many names as allowed, one more, and many more: a list beyond the limit is refused where its attribute starts, however long it is.
    let lists = [
        ("mc:Ignorable", "a", ""),
        ("mc:MustUnderstand", "a", ""),
        ("mc:ProcessContent", "a:x", " mc:Ignorable='a'"),
        ("mc:PreserveElements", "a:x", " mc:Ignorable='a'"),
        ("mc:PreserveAttributes", "a:x", " mc:Ignorable='a'"),
        ("Requires", "a", ""),
    ];
    for (attribute, name, ignorable) in lists {
        for (count, refused) in [(3, false), (4, true), (100_000, true)] {
            let list = vec![name; count].join(" ");
            let text = if attribute == "Requires" {
                format!(
                    "<root {declarations}><mc:AlternateContent><mc:Choice Requires='{list}'><x/></mc:Choice></mc:AlternateContent></root>"
                )
            } else {
                format!("<root {declarations}{ignorable} {attribute}='{list}'/>")
            };
            let expected = if refused {
                Err((
                    ErrorKind::TooManyCompatibilityTokens,
                    at(&text, &format!(" {attribute}=")) + 1,
                ))
            } else {
                Ok(())
            };
            assert_eq!(run(&text), expected, "{attribute} with {count} names");
        }
    }
}

/// Understands every ECMA-376 vocabulary and names in no namespace, with the extension elements of Office Open XML, as a word processor that knows no Microsoft extensions would.
fn ooxml() -> McConfig {
    let mut config = McConfig::new()
        .understand(NamespaceName::None)
        .with_ooxml_extension_elements();
    for namespace in [
        KnownNamespace::WordprocessingMl,
        KnownNamespace::DrawingMl,
        KnownNamespace::DrawingMlWordprocessingDrawing,
        KnownNamespace::DrawingMlPicture,
        KnownNamespace::Relationships,
        KnownNamespace::Math,
        KnownNamespace::Vml,
        KnownNamespace::VmlOffice,
        KnownNamespace::VmlWord,
        KnownNamespace::ContentTypes,
        KnownNamespace::PackageRelationships,
        KnownNamespace::CoreProperties,
        KnownNamespace::DublinCore,
        KnownNamespace::DublinCoreTerms,
        KnownNamespace::DublinCoreTypes,
        KnownNamespace::XmlSchemaInstance,
        KnownNamespace::ExtendedProperties,
        KnownNamespace::CustomProperties,
        KnownNamespace::VariantTypes,
        KnownNamespace::CustomXml,
    ] {
        config = config.understand(namespace);
    }
    config
}

fn sample(name: &str) -> String {
    let (_, bytes) = SAMPLES.iter().find(|(sample, _)| *sample == name).unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn reads_word_documents_with_shapes() {
    let text = sample("word-shapes.xml");
    let with_shapes = ooxml()
        .understand(KnownNamespace::Wps)
        .understand(KnownNamespace::Wpg);
    let events = describe(&text, &with_shapes).unwrap();
    let ends: Vec<&String> = events
        .iter()
        .filter(|event| event.starts_with("/alternate"))
        .collect();
    assert_eq!(ends, ["/alternate choice 0", "/alternate choice 0"]);
    let ignored: Vec<&String> = events
        .iter()
        .filter(|event| event.starts_with("ignored"))
        .collect();
    assert_eq!(
        ignored,
        [
            "ignored wp14:sizeRelH",
            "ignored wp14:sizeRelV",
            "ignored w15:appearance",
            "ignored w14:checkbox"
        ]
    );
    assert!(
        events.contains(
            &"<w:p w14:paraId=ignored w14:textId=ignored w:rsidR=output w:rsidRDefault=output>"
                .to_owned()
        )
    );
    let written = output(&text, &with_shapes).unwrap();
    assert!(
        written.contains("<wps:wsp>")
            && written.contains("<wpg:wgp>")
            && !written.contains("<w:pict>")
    );
    // Without word-processing groups, the group inside the text box falls back to VML.
    let written = output(&text, &ooxml().understand(KnownNamespace::Wps)).unwrap();
    assert!(
        written.contains("<wps:wsp>")
            && !written.contains("<wpg:wgp>")
            && written.contains("<v:group ")
    );
    // Without shapes, the whole text box falls back to VML, and the alternatives inside the shape are never processed.
    let events = describe(&text, &ooxml()).unwrap();
    let ends: Vec<&String> = events
        .iter()
        .filter(|event| event.starts_with("/alternate"))
        .collect();
    assert_eq!(ends, ["/alternate fallback"]);
    let written = output(&text, &ooxml()).unwrap();
    assert!(!written.contains("<wps:wsp>") && written.contains("<v:shape "));
}

#[test]
fn reads_strict_documents_like_transitional_ones() {
    let text = sample("strict-document.xml");
    // Understanding a vocabulary means understanding both its names.
    let events = describe(&text, &ooxml()).unwrap();
    assert!(
        events
            .iter()
            .all(|event| !event.contains("not understood") && !event.contains("not-understood"))
    );
    assert!(events.contains(&"<w:p w14:paraId=ignored>".to_owned()));
}

#[test]
fn reads_every_part_of_real_documents() {
    for (name, bytes) in &all_parts() {
        let text = String::from_utf8(bytes.clone()).unwrap();
        let text = text.trim_start_matches('\u{feff}');
        for config in [ooxml(), McConfig::new()] {
            assert!(output(text, &config).is_ok(), "{name}");
        }
    }
}

// Alternate content whose fragment needs bindings that only its own start tag uses (j), that only what is inside an ignored child uses (k), and not the bindings declared inside it (the inner p).
const ALTERNATE_USES: &str = "<root xmlns='urn:t:base' xmlns:mc='MC' xmlns:i='urn:t:i' xmlns:x='urn:t:x' xmlns:k='urn:t:k' xmlns:n1='urn:t:n1' xmlns:p='urn:t:outer' xmlns:j='urn:t:j' mc:Ignorable='x i'><mc:AlternateContent mc:Ignorable='j' j:flag=''><x:future><k:deep/></x:future><mc:Choice Requires='n1'><wrap xmlns:p='urn:t:inner'><i:ign><p:x/></i:ign></wrap></mc:Choice><mc:Fallback><p:y/></mc:Fallback></mc:AlternateContent></root>";

/// Every scenario of this file, with its configurations.
fn scenarios() -> Vec<(String, McConfig)> {
    let mut all = Vec::new();
    let documents = [
        CIRCLES_IGNORABLE,
        CIRCLES_PROCESS_CONTENT,
        CIRCLES_ALTERNATE,
        NESTED_ALTERNATE,
        SECTION_9_4,
        SECTION_9_2,
        ALTERNATE_USES,
    ];
    let configs: [&[&str]; 5] = [
        &[],
        &["base"],
        &["base", "v2", "n1", "p"],
        &["base", "v3", "n2", "n3", "q"],
        &["base", "v2", "v3", "n1", "n2", "n3", "p", "q", "i"],
    ];
    for text in documents {
        for names in configs {
            all.push((
                document(text),
                understanding(names).extension_element(NamespaceName::from_uri("urn:t:i"), "ext"),
            ));
        }
    }
    all
}

#[test]
fn lossless_capture_recreates_the_input() {
    let mut inputs: Vec<(String, String, McConfig)> = scenarios()
        .into_iter()
        .enumerate()
        .map(|(index, (text, config))| (format!("scenario {index}"), text, config))
        .collect();
    for (name, bytes) in all_parts() {
        let text = String::from_utf8(bytes).unwrap();
        let text = text.trim_start_matches('\u{feff}').to_owned();
        for (label, config) in [
            ("ooxml", ooxml()),
            ("nothing", McConfig::new()),
            (
                "shapes",
                ooxml()
                    .understand(KnownNamespace::Wps)
                    .understand(KnownNamespace::W14),
            ),
        ] {
            inputs.push((format!("{name} ({label})"), text.clone(), config));
        }
    }
    for (name, text, config) in &inputs {
        let recreated = recreate(text, config);
        assert_eq!(
            first_difference(&canonical(text), &canonical(&recreated)),
            None,
            "{name}"
        );
    }
}

/// The fragments of every ignored element and alternate content that processing `text` with `config` reports.
fn reported_fragments(text: &str, config: &McConfig) -> Vec<Fragment<'static>> {
    let limits = Limits::default();
    let source = Source::new(text.as_bytes(), &limits).unwrap();
    let mut reader = McReader::new(&source, &limits, config);
    let mut fragments = Vec::new();
    while let Some(event) = reader.next_event().unwrap() {
        match event {
            McEvent::Ignored(ignored) => fragments.push(ignored.into_fragment().into_owned()),
            McEvent::AlternateContentEnd(alternate) => {
                fragments.push(alternate.into_fragment().into_owned());
            }
            _ => {}
        }
    }
    fragments
}

// What processing reports whole carries the namespaces it needs, also from alternatives that were not selected and elements that were ignored inside it.
#[test]
fn reported_fragments_keep_their_meaning_in_a_new_context() {
    let limits = Limits::default();
    let mut checked = 0;
    for (name, text, config) in scenarios()
        .into_iter()
        .map(|(text, config)| ("scenario".to_owned(), text, config))
        .chain([
            (
                "word-shapes.xml".to_owned(),
                sample("word-shapes.xml"),
                ooxml(),
            ),
            (
                "word-shapes.xml".to_owned(),
                sample("word-shapes.xml"),
                ooxml().understand(KnownNamespace::Wps),
            ),
        ])
    {
        for (index, fragment) in reported_fragments(&text, &config).iter().enumerate() {
            let before = canonical(&fragment.to_standalone());
            let mut writer = Writer::new();
            writer
                .start(QName::new(Some("urn:hostile"), "hostile", "root"))
                .unwrap();
            for prefix in [
                "", "mc", "w", "wp", "wps", "wpg", "a", "v", "o", "w14", "wp14", "n1", "n2", "n3",
                "p", "q", "v2", "v3", "mc2", "i",
            ] {
                writer
                    .declare(prefix, &format!("urn:hostile:{prefix}"))
                    .unwrap();
            }
            writer.fragment(fragment).unwrap();
            writer.end().unwrap();
            let written = writer.finish().unwrap();
            let source = Source::new(written.as_bytes(), &limits).unwrap();
            let mut reader = Reader::new(&source, &limits);
            reader.next_event().unwrap();
            reader.next_event().unwrap();
            let moved = reader.capture().unwrap();
            assert_eq!(
                first_difference(&before, &canonical(&moved.to_standalone())),
                None,
                "{name}, fragment {index}"
            );
            checked += 1;
        }
    }
    assert!(checked >= 30, "only {checked} fragments");
}

#[test]
fn capture_needs_a_start_tag() {
    let text = document(CIRCLES_PROCESS_CONTENT);
    let limits = Limits::default();
    let source = Source::new(text.as_bytes(), &limits).unwrap();
    let mut reader = McReader::new(&source, &limits, &understanding(&["base"]));
    assert_eq!(
        reader.capture().unwrap_err().kind(),
        ErrorKind::NotAtStartTag
    );
    reader.next_event().unwrap();
    // The ignored element.
    reader.next_event().unwrap();
    assert_eq!(
        reader.capture().unwrap_err().kind(),
        ErrorKind::NotAtStartTag
    );
    // The unwrapped element can be captured whole, and reading continues after it.
    assert!(matches!(
        reader.next_event().unwrap(),
        Some(McEvent::UnwrapStart(_))
    ));
    let fragment = reader.capture().unwrap();
    assert_eq!(fragment.text(), "<v2:flash><shape size='2'/></v2:flash>");
    assert!(matches!(
        reader.next_event().unwrap(),
        Some(McEvent::End(_))
    ));
    assert!(reader.next_event().unwrap().is_none());
}
