//! Seeded random editing by several replicas, for the convergence tests and the spike's long runs (CORE-004 AC-1).
//!
//! A run is a list of [`Action`]s generated from a seed: replicas edit, send each other updates over links that delay and reorder them, are cut off by partitions and healed, and undo and redo. Every random choice inside an action is a raw number resolved against the replica's state when the action runs, so that any sub-list of a run is a valid run too; that is what lets [`shrink`] cut a failing run down to a few actions. At the end the network heals, every message is delivered, the replicas synchronize fully, and [`run`] checks that every replica shows the same view, that it satisfies the invariants I1–I7, and that normalizing is deterministic and idempotent.

use std::fmt;

use bayan_crdt::{ImportLimits, Value, VersionVector};

use crate::{AtomKind, Document, EntityId, View, check_invariants, marks, normalize};

/// A seeded pseudo-random number generator (SplitMix64): fast, reproducible and identical on every platform. Not cryptographic.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// A generator that always produces the same sequence for the same seed.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 random bits.
    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`, or 0 when `n` is 0.
    pub fn below(&mut self, n: usize) -> usize {
        below(self.next_u64(), n)
    }

    /// True with probability `numerator / denominator`.
    pub fn chance(&mut self, numerator: u64, denominator: u64) -> bool {
        denominator != 0 && self.next_u64() % denominator < numerator
    }

    /// A random element of a slice, or `None` when it is empty.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        items.get(self.below(items.len()))
    }
}

/// `value` reduced to `0..n` (0 when `n` is 0).
fn below(value: u64, n: usize) -> usize {
    match u64::try_from(n) {
        Ok(0) | Err(_) => 0,
        Ok(n) => usize::try_from(value % n).unwrap_or(0),
    }
}

/// The parameters of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// The number of replicas.
    pub replicas: usize,
    /// The number of editing operations (the run also contains sends, deliveries, partitions and undo).
    pub operations: usize,
    /// Check the invariants on one replica's view after every this many actions (0: only at the end).
    pub check_every: usize,
}

impl Config {
    /// The configuration of the brief: three replicas, 10,000 operations.
    pub const BRIEF: Self = Self {
        replicas: 3,
        operations: 10_000,
        check_every: 1_000,
    };
}

/// One step of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Replica `replica` performs an editing operation of `kind`, with raw random parameters.
    Edit {
        /// The replica.
        replica: usize,
        /// Which operation (see [`Kind`]).
        kind: Kind,
        /// Raw random numbers, resolved against the replica's state.
        params: [u64; 4],
    },
    /// Replica `from` sends `to` everything it has that it has not sent `to` before.
    Send {
        /// The sender.
        from: usize,
        /// The receiver.
        to: usize,
    },
    /// The message at a random position of the queue is delivered (out of order), unless a partition holds it.
    Deliver {
        /// Raw random number choosing the message.
        pick: u64,
    },
    /// A replica is cut off: messages to and from it wait.
    Partition {
        /// The replica.
        replica: usize,
    },
    /// The partition ends.
    Heal,
    /// A replica undoes its latest operation.
    Undo {
        /// The replica.
        replica: usize,
    },
    /// A replica redoes its latest undone operation.
    Redo {
        /// The replica.
        replica: usize,
    },
}

/// The editing operations a run draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Insert text.
    InsertText,
    /// Delete a range.
    Delete,
    /// Set a mark on a range (bold, italic, a hyperlink or a tracked insertion).
    Format,
    /// Remove a mark from a range.
    ClearFormat,
    /// Split a paragraph.
    Split,
    /// Merge a paragraph with the next.
    Merge,
    /// Set a paragraph property.
    ParagraphProperty,
    /// Insert a table.
    InsertTable,
    /// Insert a row.
    InsertRow,
    /// Delete a row.
    DeleteRow,
    /// Move a row.
    MoveRow,
    /// Insert a column.
    InsertColumn,
    /// Delete a column.
    DeleteColumn,
    /// Add a comment over a range.
    Comment,
    /// Insert a field.
    Field,
    /// Insert an object.
    Object,
    /// Insert a bookmark around a range.
    Bookmark,
    /// Move a range, within a story or to another story.
    Move,
}

/// Every kind with its weight (out of 100).
const KINDS: [(Kind, u64); 18] = [
    (Kind::InsertText, 30),
    (Kind::Delete, 10),
    (Kind::Format, 8),
    (Kind::ClearFormat, 2),
    (Kind::Split, 8),
    (Kind::Merge, 4),
    (Kind::ParagraphProperty, 2),
    (Kind::InsertTable, 4),
    (Kind::InsertRow, 3),
    (Kind::DeleteRow, 3),
    (Kind::MoveRow, 2),
    (Kind::InsertColumn, 2),
    (Kind::DeleteColumn, 2),
    (Kind::Comment, 4),
    (Kind::Field, 3),
    (Kind::Object, 3),
    (Kind::Bookmark, 3),
    (Kind::Move, 7),
];

/// The text a run types: Latin, Arabic, a space and a character outside the Basic Multilingual Plane.
const TEXTS: [&str; 6] = ["a", "xyz", " ", "\u{0628}\u{064A}", "\u{1F600}", "Ab c"];

/// Generates the actions of a run: `config.operations` editing operations, with sends, out-of-order deliveries, partitions and undo or redo in between.
#[must_use]
pub fn plan(seed: u64, config: &Config) -> Vec<Action> {
    let mut rng = Rng::new(seed);
    let replicas = config.replicas.max(1);
    let mut actions = Vec::new();
    let mut operations = 0;
    while operations < config.operations {
        let roll = rng.below(100);
        let replica = rng.below(replicas);
        actions.push(if roll < 70 {
            operations += 1;
            let mut weight = rng.below(100);
            let kind = KINDS
                .iter()
                .find(|(_, share)| {
                    let share = usize::try_from(*share).unwrap_or(0);
                    if weight < share {
                        true
                    } else {
                        weight -= share;
                        false
                    }
                })
                .map_or(Kind::InsertText, |(kind, _)| *kind);
            Action::Edit {
                replica,
                kind,
                params: [
                    rng.next_u64(),
                    rng.next_u64(),
                    rng.next_u64(),
                    rng.next_u64(),
                ],
            }
        } else if roll < 82 {
            Action::Send {
                from: replica,
                to: (replica + 1 + rng.below(replicas.saturating_sub(1).max(1))) % replicas,
            }
        } else if roll < 93 {
            Action::Deliver {
                pick: rng.next_u64(),
            }
        } else if roll < 95 {
            if rng.chance(1, 2) {
                Action::Partition { replica }
            } else {
                Action::Heal
            }
        } else if roll < 98 {
            Action::Undo { replica }
        } else {
            Action::Redo { replica }
        });
    }
    actions
}

/// Why a run failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The action at which it failed, or `None` for the final checks.
    pub step: Option<usize>,
    /// What went wrong.
    pub reason: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.step {
            Some(step) => write!(formatter, "at action {step}: {}", self.reason),
            None => write!(formatter, "at the end: {}", self.reason),
        }
    }
}

/// What a successful run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Operations applied.
    pub applied: usize,
    /// Operations the replica refused (the generator does not know the state, so some do not fit it).
    pub refused: usize,
    /// Messages delivered.
    pub delivered: usize,
    /// Undo and redo steps performed.
    pub undone: usize,
    /// Normalization repairs in the final view, per rule N1–N7.
    pub repairs: [usize; 7],
    /// Atoms in the final main story.
    pub main_len: usize,
}

struct Message {
    from: usize,
    to: usize,
    bytes: Vec<u8>,
}

/// Runs `actions` on fresh replicas and checks convergence and the view's properties at the end (and every `config.check_every` actions on one replica).
///
/// # Errors
///
/// A [`Failure`] describing the first broken property.
pub fn run(seed: u64, actions: &[Action], config: &Config) -> Result<Stats, Failure> {
    let fail = |step: Option<usize>, reason: String| Failure { step, reason };
    let replicas = config.replicas.max(1);
    let mut documents = initial_replicas(seed, replicas).map_err(|reason| fail(None, reason))?;
    let mut sent: Vec<Vec<VersionVector>> = vec![vec![VersionVector::new(); replicas]; replicas];
    let mut queue: Vec<Message> = Vec::new();
    let mut partitioned: Option<usize> = None;
    let mut stats = Stats::default();
    for (step, action) in actions.iter().enumerate() {
        match *action {
            Action::Edit {
                replica,
                kind,
                params,
            } => {
                let Some(document) = documents.get_mut(replica) else {
                    continue;
                };
                if edit(document, kind, params) {
                    stats.applied += 1;
                } else {
                    stats.refused += 1;
                }
            }
            Action::Send { from, to } => {
                if from >= replicas || to >= replicas || from == to {
                    continue;
                }
                let since = &sent[from][to];
                let bytes = documents[from]
                    .export_updates(since)
                    .map_err(|error| fail(Some(step), format!("export failed: {error}")))?;
                sent[from][to] = documents[from].version_vector();
                queue.push(Message { from, to, bytes });
            }
            Action::Deliver { pick } => {
                let deliverable: Vec<usize> = (0..queue.len())
                    .filter(|index| {
                        partitioned
                            .is_none_or(|cut| queue[*index].from != cut && queue[*index].to != cut)
                    })
                    .collect();
                if !deliverable.is_empty() {
                    let message = queue.remove(deliverable[below(pick, deliverable.len())]);
                    deliver(&mut documents, &message).map_err(|reason| fail(Some(step), reason))?;
                    stats.delivered += 1;
                }
            }
            Action::Partition { replica } => partitioned = Some(replica % replicas),
            Action::Heal => partitioned = None,
            Action::Undo { replica } => {
                if let Some(document) = documents.get_mut(replica)
                    && document.undo().unwrap_or(false)
                {
                    stats.undone += 1;
                }
            }
            Action::Redo { replica } => {
                if let Some(document) = documents.get_mut(replica)
                    && document.redo().unwrap_or(false)
                {
                    stats.undone += 1;
                }
            }
        }
        if config.check_every > 0 && step % config.check_every == config.check_every - 1 {
            let document = &documents[step / config.check_every % replicas];
            check_view(&document.view()).map_err(|reason| fail(Some(step), reason))?;
        }
    }
    // The network heals and delivers everything, in the order the messages were sent; then every replica asks every other for what it still lacks.
    for message in std::mem::take(&mut queue) {
        deliver(&mut documents, &message).map_err(|reason| fail(None, reason))?;
        stats.delivered += 1;
    }
    for _ in 0..2 {
        for from in 0..replicas {
            for to in 0..replicas {
                if from != to {
                    let bytes = documents[from]
                        .export_updates(&documents[to].version_vector())
                        .map_err(|error| fail(None, format!("export failed: {error}")))?;
                    documents[to]
                        .import(&bytes, &ImportLimits::UPDATE)
                        .map_err(|error| fail(None, format!("final import failed: {error}")))?;
                }
            }
        }
    }
    let first = documents[0].version_vector();
    if documents
        .iter()
        .any(|document| document.version_vector() != first)
    {
        return Err(fail(
            None,
            "the replicas did not receive the same changes".to_owned(),
        ));
    }
    let (view, report) = normalize(&documents[0].raw());
    for (index, document) in documents.iter().enumerate().skip(1) {
        if document.view() != view {
            return Err(fail(
                None,
                format!("replica {index} shows a different view than replica 0"),
            ));
        }
    }
    check_view(&view).map_err(|reason| fail(None, reason))?;
    if normalize(&documents[0].raw()).0 != view {
        return Err(fail(
            None,
            "normalizing twice gave different views".to_owned(),
        ));
    }
    let (again, repaired) = normalize(&view.to_raw());
    if again != view {
        return Err(fail(None, "normalization is not idempotent".to_owned()));
    }
    if repaired.n1
        + repaired.n2
        + repaired.n3
        + repaired.n4
        + repaired.n5
        + repaired.n6
        + repaired.n7
        != 0
    {
        return Err(fail(
            None,
            format!("normalizing the view repaired something: {repaired:?}"),
        ));
    }
    stats.repairs = [
        report.n1, report.n2, report.n3, report.n4, report.n5, report.n6, report.n7,
    ];
    stats.main_len = view.main().len();
    Ok(stats)
}

/// Checks the invariants and that no placeholder leaks into any story's plain text.
fn check_view(view: &View) -> Result<(), String> {
    let violations = check_invariants(view);
    if !violations.is_empty() {
        return Err(format!("invariants broken: {violations:?}"));
    }
    for story in view.stories.keys() {
        if view
            .plain_text(*story)
            .chars()
            .any(|character| character < ' ' && character != '\n' && character != '\t')
        {
            return Err(format!(
                "a placeholder leaks into the plain text of story {story}"
            ));
        }
    }
    Ok(())
}

fn deliver(documents: &mut [Document], message: &Message) -> Result<(), String> {
    documents[message.to]
        .import(&message.bytes, &ImportLimits::UPDATE)
        .map(|_| ())
        .map_err(|error| format!("import from replica {} failed: {error}", message.from))
}

/// The replicas at the start of a run: replica 0 creates a small document with paragraphs, a table, a comment and a field, and the others load it.
///
/// # Errors
///
/// A description of what failed (it does not, unless the model is broken).
pub fn initial_replicas(seed: u64, replicas: usize) -> Result<Vec<Document>, String> {
    let error = |error: crate::EditError| error.to_string();
    let main = EntityId::MAIN_STORY;
    let mut first = Document::new(1, seed).map_err(error)?;
    first.insert_text(main, 0, "Hello world").map_err(error)?;
    first.split_paragraph(main, 5).map_err(error)?;
    first.split_paragraph(main, 6).map_err(error)?;
    first.insert_table(main, 7, 2, 2).map_err(error)?;
    first
        .add_comment(main, 0..5, "Reviewer", "first")
        .map_err(error)?;
    first.insert_field(main, 2, "PAGE", "1").map_err(error)?;
    let snapshot = first.export_snapshot().map_err(error)?;
    let mut documents = vec![first];
    for index in 1..replicas {
        let peer = u64::try_from(index).unwrap_or(u64::MAX - 1) + 1;
        documents.push(
            Document::load(&snapshot, peer, seed, &ImportLimits::LOCAL_SNAPSHOT).map_err(error)?,
        );
    }
    Ok(documents)
}

/// Performs one random editing operation, drawn as [`plan`] draws them. Returns whether the replica applied it. For tools that edit documents randomly outside a run, such as the spike's fuzzer.
pub fn random_edit(document: &mut Document, rng: &mut Rng) -> bool {
    let mut weight = rng.below(100);
    let kind = KINDS
        .iter()
        .find(|(_, share)| {
            let share = usize::try_from(*share).unwrap_or(0);
            if weight < share {
                true
            } else {
                weight -= share;
                false
            }
        })
        .map_or(Kind::InsertText, |(kind, _)| *kind);
    let params = [
        rng.next_u64(),
        rng.next_u64(),
        rng.next_u64(),
        rng.next_u64(),
    ];
    edit(document, kind, params)
}

/// Performs one editing operation, resolving the raw parameters against the replica's state. Returns whether it was applied.
fn edit(document: &mut Document, kind: Kind, params: [u64; 4]) -> bool {
    let [a, b, c, d] = params;
    // Listing the stories costs time, so only when an operation needs it.
    let any_story = |document: &Document, pick: u64| {
        let stories = document.stories();
        stories[below(pick, stories.len())]
    };
    // The main story most of the time, another story (cell or comment) otherwise.
    let story = if below(a, 10) < 6 {
        EntityId::MAIN_STORY
    } else {
        any_story(document, a >> 8)
    };
    let len = document.story_len(story).unwrap_or(0);
    if len == 0 {
        return false;
    }
    let pos = below(b, len);
    let span =
        |start: usize, max: usize| start..(start + 1 + below(c, max)).min(len.saturating_sub(1));
    let table = || {
        let tables = document.tables();
        tables.get(below(d, tables.len())).copied()
    };
    let text = TEXTS[below(c, TEXTS.len())];
    let result = match kind {
        Kind::InsertText => document.insert_text(story, pos, text),
        Kind::Delete => document.delete(story, span(below(b, len.saturating_sub(1)), 6)),
        Kind::Format => {
            let (key, value) = match below(d, 4) {
                0 => (marks::BOLD, Value::Bool(true)),
                1 => (marks::ITALIC, Value::Bool(true)),
                2 => (marks::LINK, Value::from("https://example.org")),
                _ => (marks::INSERTED, Value::from("author")),
            };
            document.format(story, pos..(pos + 1 + below(c, 12)).min(len), key, &value)
        }
        Kind::ClearFormat => {
            document.clear_format(story, pos..(pos + 1 + below(c, 12)).min(len), marks::BOLD)
        }
        Kind::Split => document.split_paragraph(story, pos).map(|_| ()),
        Kind::Merge => {
            let ends = paragraph_ends(document, story);
            match ends.get(below(c, ends.len())) {
                Some(end) => document.merge_paragraph(story, *end),
                None => return false,
            }
        }
        Kind::ParagraphProperty => {
            let paragraphs = document.paragraphs_in(story);
            match paragraphs.get(below(c, paragraphs.len())) {
                Some(paragraph) => document.set_paragraph_property(
                    *paragraph,
                    "jc",
                    &Value::from(["left", "center", "both"][below(d, 3)]),
                ),
                None => return false,
            }
        }
        Kind::InsertTable => {
            let positions = block_positions(document, story);
            match positions.get(below(c, positions.len())) {
                Some(pos) => document
                    .insert_table(story, *pos, 1 + below(d, 3), 1 + below(d >> 8, 3))
                    .map(|_| ()),
                None => return false,
            }
        }
        Kind::InsertRow => match table() {
            Some(table) => document.insert_row(table, below(c, 4)).map(|_| ()),
            None => return false,
        },
        Kind::DeleteRow => match table() {
            Some(table) => document.delete_row(table, below(c, 4)),
            None => return false,
        },
        Kind::MoveRow => match table() {
            Some(table) => document.move_row(table, below(c, 4), below(c >> 8, 4)),
            None => return false,
        },
        Kind::InsertColumn => match table() {
            Some(table) => document.insert_column(table, below(c, 4)),
            None => return false,
        },
        Kind::DeleteColumn => match table() {
            Some(table) => document.delete_column(table, below(c, 4)),
            None => return false,
        },
        Kind::Comment => document
            .add_comment(story, span(pos, 20), "Reviewer", text)
            .map(|_| ()),
        Kind::Field => document.insert_field(story, pos, "PAGE", text).map(|_| ()),
        Kind::Object => document.insert_object(story, pos).map(|_| ()),
        Kind::Bookmark => document
            .insert_bookmark(story, span(pos, 20), "mark")
            .map(|_| ()),
        Kind::Move => {
            let target = if below(d, 3) == 0 {
                any_story(document, d >> 8)
            } else {
                story
            };
            let target_len = document.story_len(target).unwrap_or(0);
            let range = span(below(b, len.saturating_sub(1)), 8);
            document.move_range(story, range, target, below(d >> 16, target_len))
        }
    };
    result.is_ok()
}

/// The positions of the paragraph ends of a story that may be merged (all but the last).
fn paragraph_ends(document: &Document, story: EntityId) -> Vec<usize> {
    let text: Vec<char> = document
        .story_text(story)
        .unwrap_or_default()
        .chars()
        .collect();
    let last = text.len().saturating_sub(1);
    (0..last)
        .filter(|pos| text[*pos] == AtomKind::ParagraphEnd.placeholder())
        .collect()
}

/// The block positions of a story where a table may be inserted.
fn block_positions(document: &Document, story: EntityId) -> Vec<usize> {
    let text: Vec<char> = document
        .story_text(story)
        .unwrap_or_default()
        .chars()
        .collect();
    let block = |character: char| {
        character == AtomKind::ParagraphEnd.placeholder()
            || character == AtomKind::TableBlock.placeholder()
    };
    (0..text.len())
        .filter(|pos| *pos == 0 || block(text[pos - 1]))
        .collect()
}

/// Shrinks a failing run: removes chunks of actions (halves, quarters, … single actions) as long as the run still fails, trying at most `budget` runs. Returns the smallest failing list found.
#[must_use]
pub fn shrink(seed: u64, actions: &[Action], config: &Config, budget: usize) -> Vec<Action> {
    let mut current = actions.to_vec();
    let mut attempts = 0;
    let mut chunk = current.len() / 2;
    while chunk > 0 && attempts < budget {
        let mut start = 0;
        let mut removed_any = false;
        while start < current.len() && attempts < budget {
            let end = (start + chunk).min(current.len());
            let mut candidate = current[..start].to_vec();
            candidate.extend_from_slice(&current[end..]);
            attempts += 1;
            if run(seed, &candidate, config).is_err() {
                current = candidate;
                removed_any = true;
            } else {
                start = end;
            }
        }
        if !removed_any {
            chunk /= 2;
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_are_reproducible_and_have_the_requested_size() {
        let config = Config {
            replicas: 3,
            operations: 200,
            check_every: 0,
        };
        let first = plan(5, &config);
        assert_eq!(first, plan(5, &config));
        assert_ne!(first, plan(6, &config));
        let edits = first
            .iter()
            .filter(|action| matches!(action, Action::Edit { .. }))
            .count();
        assert_eq!(edits, 200);
    }

    #[test]
    fn the_weights_add_up_to_one_hundred() {
        assert_eq!(KINDS.iter().map(|(_, weight)| weight).sum::<u64>(), 100);
    }
}
