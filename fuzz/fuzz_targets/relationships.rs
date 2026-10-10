//! The relationships part parser (bayan-opc's `Relationships`) and the minimal XML parser under it, on arbitrary bytes. For every part it accepts: written back unchanged it gives exactly the input; with a relationship added it parses again and holds it; with that relationship removed it gives the input again. Every target resolves or fails cleanly.

#![no_main]

use bayan_opc::{Limits, PartName, Relationship, RelationshipSource, Relationships, TargetMode};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(mut relationships) = Relationships::parse(data, &Limits::DEFAULT) else {
        return;
    };
    assert_eq!(relationships.to_xml(), data);
    let source = RelationshipSource::Part(PartName::new("/word/document.xml").unwrap());
    for relationship in relationships.iter() {
        let _ = relationship.resolve(&source);
        let _ = relationship.resolve(&RelationshipSource::Package);
    }
    let id = relationships.next_id();
    assert!(relationships.get(&id).is_none());
    relationships
        .add(Relationship::new(
            id.clone(),
            "urn:bayan:fuzz",
            "https://example.com/?a=1&b=\"2\"",
            TargetMode::External,
        ))
        .unwrap();
    let changed = Relationships::parse(&relationships.to_xml(), &Limits::DEFAULT).unwrap();
    assert_eq!(
        changed.get(&id).map(Relationship::target),
        Some("https://example.com/?a=1&b=\"2\"")
    );
    assert!(relationships.remove(&id).is_some());
    assert_eq!(relationships.to_xml(), data);
});
