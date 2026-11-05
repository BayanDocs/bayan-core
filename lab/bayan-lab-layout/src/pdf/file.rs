//! A PDF file's structure (ISO 32000-2 §7.5): its header, cross-reference sections (tables and streams, incremental updates and hybrid files), trailer, indirect objects and object streams.
//!
//! The reader is strict: every cross-reference entry must point at the object it names, every stream's `Length` must end at `endstream`, and anything else is refused, never repaired. Encrypted files are refused.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use super::filter::{self, Budget, Filter, Predictor};
use super::lexer::{Lexer, Token, is_regular, is_whitespace};
use super::object::{Dict, Object, Ref, Stream, Values, parse_object};
use super::{Error, Limit, Limits, MAX_FILTERS};

/// Where the newest cross-reference section puts an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    /// The object is free (or the entry has a type this reader treats as free, §7.5.8.3).
    Free,
    /// The object is at `offset` in the file, with `generation`.
    InFile { offset: usize, generation: u16 },
    /// The object is member `index` of object stream `stream`.
    InStream { stream: u32, index: u32 },
}

/// A decoded object stream: its data and, for each member, its object number and offset in the data.
#[derive(Debug)]
struct ObjectStream {
    data: Vec<u8>,
    members: Vec<(u32, usize)>,
}

/// A value that may be borrowed from an object or loaded from the file.
pub(crate) enum Value<'o> {
    /// A direct object.
    Direct(&'o Object),
    /// An indirect object, shared with the file's cache.
    Indirect(Rc<Object>),
}

impl core::ops::Deref for Value<'_> {
    type Target = Object;

    fn deref(&self) -> &Object {
        match self {
            Self::Direct(object) => object,
            Self::Indirect(object) => object,
        }
    }
}

impl Value<'_> {
    /// The dictionary, if the value is one (a stream is not).
    pub(crate) fn dict(&self) -> Option<&Dict> {
        match &**self {
            Object::Dict(dict) => Some(dict),
            _ => None,
        }
    }
}

/// An open PDF file.
pub(crate) struct File<'a> {
    data: &'a [u8],
    limits: Limits,
    entries: BTreeMap<u32, Entry>,
    trailer: Dict,
    /// Every object read, kept for the whole extraction: the count of [`Limits::max_object_values`] bounds them.
    objects: BTreeMap<u32, Rc<Object>>,
    object_streams: BTreeMap<u32, Rc<ObjectStream>>,
    /// The members that the object streams read so far declare, together: [`Limits::max_objects`] bounds them.
    members: usize,
    /// The decoded data of the streams that are used again and again (form XObjects), decoded once.
    kept_streams: BTreeMap<Ref, Rc<Vec<u8>>>,
    loading: BTreeSet<u32>,
    /// What may still be read: [`Limits::max_read_bytes`] and [`Limits::max_stream_bytes`].
    budget: Budget,
    values: Values,
}

/// The bytes searched for `startxref` at the end of the file.
const TAIL: usize = 1024;

impl<'a> File<'a> {
    /// Reads the header, the cross-reference sections and the trailer.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] if the structure is broken, [`Error::Unsupported`] for encrypted files, and [`Error::Limit`] past the limits.
    pub(crate) fn open(data: &'a [u8], limits: Limits) -> Result<Self, Error> {
        if data.len() > limits.max_input_bytes {
            return Err(Error::Limit(Limit::InputSize));
        }
        check_header(data)?;
        let mut file = Self {
            data,
            limits,
            entries: BTreeMap::new(),
            trailer: Dict::default(),
            objects: BTreeMap::new(),
            object_streams: BTreeMap::new(),
            members: 0,
            kept_streams: BTreeMap::new(),
            loading: BTreeSet::new(),
            budget: Budget {
                max_stream: limits.max_stream_bytes,
                remaining: limits.max_read_bytes,
            },
            values: Values::kept(limits.max_object_values),
        };
        let start = find_startxref(data)?;
        file.read_sections(start)?;
        Ok(file)
    }

    /// The trailer of the newest cross-reference section.
    pub(crate) const fn trailer(&self) -> &Dict {
        &self.trailer
    }

    /// The limits the file is read with.
    pub(crate) const fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Follows the chain of cross-reference sections from the newest one at `start`.
    fn read_sections(&mut self, start: usize) -> Result<(), Error> {
        let mut visited = BTreeSet::new();
        let mut next = Some(start);
        let mut newest = true;
        while let Some(offset) = next {
            if !visited.insert(offset) {
                return Err(Error::malformed("the cross-reference sections form a loop"));
            }
            if visited.len() > self.limits.max_xref_sections {
                return Err(Error::Limit(Limit::XrefSections));
            }
            let (trailer, free) = self.read_section(offset)?;
            if trailer.contains(b"Encrypt") {
                return Err(Error::unsupported("encrypted files"));
            }
            // In a hybrid file, the cross-reference stream that `XRefStm` names adds the objects of object streams to the table's section (§7.5.8.4); its entries come after the table's objects in use and before the table's free ones.
            if let Some(stream) = trailer.get(b"XRefStm") {
                let stream = offset_of(stream, self.data.len())?;
                if !visited.insert(stream) {
                    return Err(Error::malformed("the cross-reference sections form a loop"));
                }
                if visited.len() > self.limits.max_xref_sections {
                    return Err(Error::Limit(Limit::XrefSections));
                }
                let (_, stream_free) = self.read_section(stream)?;
                self.add_free(&stream_free)?;
            }
            self.add_free(&free)?;
            next = match trailer.get(b"Prev") {
                Some(prev) => Some(offset_of(prev, self.data.len())?),
                None => None,
            };
            if newest {
                self.trailer = trailer;
                newest = false;
            }
        }
        Ok(())
    }

    /// Records an entry unless a newer section has one for the object.
    fn add(&mut self, number: u32, entry: Entry) -> Result<(), Error> {
        if !self.entries.contains_key(&number) {
            if self.entries.len() >= self.limits.max_objects {
                return Err(Error::Limit(Limit::Objects));
            }
            self.entries.insert(number, entry);
        }
        Ok(())
    }

    fn add_free(&mut self, free: &[u32]) -> Result<(), Error> {
        for &number in free {
            self.add(number, Entry::Free)?;
        }
        Ok(())
    }

    /// Reads the section at `offset`: adds its entries in use, and returns its trailer and its free objects, which the caller adds after a hybrid file's stream. A table and its trailer count as read here, a cross-reference stream as every object does.
    fn read_section(&mut self, offset: usize) -> Result<(Dict, Vec<u32>), Error> {
        let mut lexer = Lexer::new(self.data, offset);
        if lexer.eat_keyword(b"xref") {
            let section = self.read_table(&mut lexer)?;
            self.budget.spend(lexer.furthest().saturating_sub(offset))?;
            Ok(section)
        } else {
            self.read_xref_stream(offset)
        }
    }

    /// A cross-reference table (§7.5.4) and the trailer after it.
    fn read_table(&mut self, lexer: &mut Lexer<'a>) -> Result<(Dict, Vec<u32>), Error> {
        let mut free = Vec::new();
        loop {
            if lexer.eat_keyword(b"trailer") {
                break;
            }
            let (Some(Token::Integer(first)), Some(Token::Integer(count))) =
                (lexer.next_token()?, lexer.next_token()?)
            else {
                return Err(Error::malformed(
                    "a cross-reference table has a malformed subsection",
                ));
            };
            let first = u32::try_from(first).map_err(|_| {
                Error::malformed("a cross-reference subsection starts at a negative number")
            })?;
            let count = u32::try_from(count).map_err(|_| {
                Error::malformed("a cross-reference subsection has a negative count")
            })?;
            let last = u64::from(first) + u64::from(count);
            if last > u64::try_from(self.limits.max_objects).unwrap_or(u64::MAX) {
                return Err(Error::Limit(Limit::Objects));
            }
            let end = first
                .checked_add(count)
                .ok_or_else(|| Error::malformed("a cross-reference subsection is out of range"))?;
            lexer.skip_whitespace();
            for number in first..end {
                let start = lexer.position();
                let line = self
                    .data
                    .get(start..start + 20)
                    .ok_or_else(|| Error::malformed("a cross-reference table is cut short"))?;
                let entry = parse_table_entry(line)?;
                lexer.seek(start + 20);
                match entry {
                    Some((offset, generation)) => {
                        if offset >= self.data.len() {
                            return Err(Error::malformed(format!(
                                "the cross-reference entry of object {number} points beyond the end of the file"
                            )));
                        }
                        self.add(number, Entry::InFile { offset, generation })?;
                    }
                    None => free.push(number),
                }
            }
        }
        let trailer = parse_object(lexer, true, self.limits.max_nesting, &mut self.values)?;
        let Object::Dict(trailer) = trailer else {
            return Err(Error::malformed("the trailer is not a dictionary"));
        };
        Ok((trailer, free))
    }

    /// A cross-reference stream (§7.5.8).
    fn read_xref_stream(&mut self, offset: usize) -> Result<(Dict, Vec<u32>), Error> {
        let mut lexer = Lexer::new(self.data, offset);
        let (_, object) = self.parse_indirect(&mut lexer, None, false)?;
        let Object::Stream(stream) = object else {
            return Err(Error::malformed(
                "no cross-reference table or stream where `startxref` or `Prev` points",
            ));
        };
        if !stream.dict.has_name(b"Type", b"XRef") {
            return Err(Error::malformed(
                "a cross-reference stream's `Type` is not `XRef`",
            ));
        }
        let widths = stream
            .dict
            .get(b"W")
            .and_then(Object::as_array)
            .filter(|widths| widths.len() == 3)
            .ok_or_else(|| {
                Error::malformed("a cross-reference stream's `W` is not three integers")
            })?;
        let mut field = [0_usize; 3];
        for (slot, width) in field.iter_mut().zip(widths) {
            *slot = width
                .as_integer()
                .and_then(|width| usize::try_from(width).ok())
                .filter(|&width| width <= 8)
                .ok_or_else(|| {
                    Error::malformed("a cross-reference stream's field width is out of range")
                })?;
        }
        let entry_width = field.iter().sum::<usize>();
        if entry_width == 0 {
            return Err(Error::malformed(
                "a cross-reference stream has entries of no bytes",
            ));
        }
        let size = stream
            .dict
            .get(b"Size")
            .and_then(Object::as_integer)
            .and_then(|size| u32::try_from(size).ok())
            .ok_or_else(|| Error::malformed("a cross-reference stream's `Size` is not a count"))?;
        if !usize::try_from(size).is_ok_and(|size| size <= self.limits.max_objects) {
            return Err(Error::Limit(Limit::Objects));
        }
        let ranges: Vec<(u32, u32)> = match stream.dict.get(b"Index") {
            None => vec![(0, size)],
            Some(Object::Array(index)) if index.len().is_multiple_of(2) => index
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[first, count]| {
                    let first = first
                        .as_integer()
                        .and_then(|value| u32::try_from(value).ok());
                    let count = count
                        .as_integer()
                        .and_then(|value| u32::try_from(value).ok());
                    match (first, count) {
                        (Some(first), Some(count))
                            if u64::from(first) + u64::from(count) <= u64::from(size) =>
                        {
                            Ok((first, count))
                        }
                        _ => Err(Error::malformed(
                            "a cross-reference stream's `Index` is out of range",
                        )),
                    }
                })
                .collect::<Result<_, _>>()?,
            Some(_) => {
                return Err(Error::malformed(
                    "a cross-reference stream's `Index` is not pairs of integers",
                ));
            }
        };
        let data = self.decode_stream(&stream, true)?;
        let total: u64 = ranges.iter().map(|&(_, count)| u64::from(count)).sum();
        let expected = total.checked_mul(u64::try_from(entry_width).unwrap_or(u64::MAX));
        if expected != u64::try_from(data.len()).ok() {
            return Err(Error::malformed(
                "a cross-reference stream's data does not hold exactly its entries",
            ));
        }
        let mut free = Vec::new();
        let mut records = data.chunks_exact(entry_width);
        for (first, count) in ranges {
            for number in first..first + count {
                let record = records
                    .next()
                    .ok_or_else(|| Error::malformed("a cross-reference stream is cut short"))?;
                let (kind_bytes, rest) = record.split_at(field[0]);
                let (second, third) = rest.split_at(field[1]);
                let kind = if field[0] == 0 {
                    1
                } else {
                    big_endian(kind_bytes)
                };
                match kind {
                    1 => {
                        let offset = usize::try_from(big_endian(second))
                            .ok()
                            .filter(|&offset| offset < self.data.len())
                            .ok_or_else(|| {
                                Error::malformed(format!(
                                    "the cross-reference entry of object {number} points beyond the end of the file"
                                ))
                            })?;
                        let generation = u16::try_from(big_endian(third)).map_err(|_| {
                            Error::malformed(format!(
                                "object {number} has a generation number out of range"
                            ))
                        })?;
                        self.add(number, Entry::InFile { offset, generation })?;
                    }
                    2 => {
                        let stream_number = u32::try_from(big_endian(second)).map_err(|_| {
                            Error::malformed(format!(
                                "object {number} names an object stream out of range"
                            ))
                        })?;
                        let index = u32::try_from(big_endian(third)).map_err(|_| {
                            Error::malformed(format!("object {number} has an index out of range"))
                        })?;
                        self.add(
                            number,
                            Entry::InStream {
                                stream: stream_number,
                                index,
                            },
                        )?;
                    }
                    // Type 0 is a free object; other types are references to the null object (§7.5.8.3).
                    _ => free.push(number),
                }
            }
        }
        Ok((stream.dict, free))
    }

    /// Parses `n g obj … endobj` at the lexer, with its stream if it has one. With `expected`, the object must be that one. `indirect_length` allows a stream's `Length` to be an indirect object, which a cross-reference stream's may not be.
    ///
    /// Every byte the parse reads counts toward [`Limits::max_read_bytes`], except a stream's data, which counts whenever it is decoded. Objects can overlap, so that the same bytes are parsed again for another object; they count again each time.
    fn parse_indirect(
        &mut self,
        lexer: &mut Lexer<'a>,
        expected: Option<Ref>,
        indirect_length: bool,
    ) -> Result<(Ref, Object), Error> {
        let start = lexer.position();
        let header = (
            lexer.next_token()?,
            lexer.next_token()?,
            lexer.eat_keyword(b"obj"),
        );
        let (Some(Token::Integer(number)), Some(Token::Integer(generation)), true) = header else {
            return Err(Error::malformed(format!(
                "no object header at byte {start}"
            )));
        };
        let reference = match (u32::try_from(number), u16::try_from(generation)) {
            (Ok(number), Ok(generation)) => Ref { number, generation },
            _ => {
                return Err(Error::malformed(format!(
                    "an object header at byte {start} is out of range"
                )));
            }
        };
        if expected.is_some_and(|expected| expected != reference) {
            return Err(Error::malformed(format!(
                "object {} {} is not where its cross-reference entry points",
                reference.number, reference.generation
            )));
        }
        let object = parse_object(lexer, true, self.limits.max_nesting, &mut self.values)?;
        // What the parse has read counts at once, before a stream's `Length` can make the reader read another object.
        let parsed = lexer.furthest();
        self.budget.spend(parsed.saturating_sub(start))?;
        if lexer.eat_keyword(b"stream") {
            let Object::Dict(dict) = object else {
                return Err(Error::malformed(format!(
                    "object {} has stream data without a dictionary",
                    reference.number
                )));
            };
            let stream = self.stream_at(lexer, dict, indirect_length, reference)?;
            if !lexer.eat_keyword(b"endobj") {
                return Err(Error::malformed(format!(
                    "object {} has no `endobj`",
                    reference.number
                )));
            }
            let around = lexer
                .furthest()
                .saturating_sub(parsed)
                .saturating_sub(stream.length);
            self.budget.spend(around)?;
            return Ok((reference, Object::Stream(stream)));
        }
        if !lexer.eat_keyword(b"endobj") {
            return Err(Error::malformed(format!(
                "object {} has no `endobj`",
                reference.number
            )));
        }
        self.budget.spend(lexer.furthest().saturating_sub(parsed))?;
        Ok((reference, object))
    }

    /// The stream whose keyword `stream` the lexer has just read.
    fn stream_at(
        &mut self,
        lexer: &mut Lexer<'a>,
        dict: Dict,
        indirect_length: bool,
        reference: Ref,
    ) -> Result<Stream, Error> {
        // The keyword is followed by CR LF or LF, never CR alone (§7.3.8.1).
        let mut start = lexer.position();
        match (self.data.get(start), self.data.get(start + 1)) {
            (Some(b'\r'), Some(b'\n')) => start += 2,
            (Some(b'\n'), _) => start += 1,
            _ => {
                return Err(Error::malformed(format!(
                    "the stream of object {} does not start on a new line",
                    reference.number
                )));
            }
        }
        let length = match dict.get(b"Length") {
            Some(Object::Integer(length)) => *length,
            Some(Object::Ref(length)) if indirect_length => {
                let object = self.get(*length)?;
                object.as_integer().ok_or_else(|| {
                    Error::malformed(format!(
                        "the `Length` of object {} is not an integer",
                        reference.number
                    ))
                })?
            }
            _ => {
                return Err(Error::malformed(format!(
                    "the stream of object {} has no usable `Length`",
                    reference.number
                )));
            }
        };
        let length = usize::try_from(length).map_err(|_| {
            Error::malformed(format!(
                "the stream of object {} has a negative `Length`",
                reference.number
            ))
        })?;
        let end = start
            .checked_add(length)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| {
                Error::malformed(format!(
                    "the stream of object {} runs past the end of the file",
                    reference.number
                ))
            })?;
        // Only an end of line may stand between the data and `endstream`.
        let mut after = end;
        while self
            .data
            .get(after)
            .is_some_and(|&byte| is_whitespace(byte))
        {
            after += 1;
        }
        let keyword = b"endstream";
        let ends = self.data.get(after..after + keyword.len()) == Some(&keyword[..])
            && self
                .data
                .get(after + keyword.len())
                .is_none_or(|&byte| !is_regular(byte));
        if !ends {
            return Err(Error::malformed(format!(
                "the `Length` of object {} does not end at `endstream`",
                reference.number
            )));
        }
        lexer.seek(after + keyword.len());
        Ok(Stream {
            dict,
            start,
            length,
        })
    }

    /// The indirect object `reference`, or null if it does not exist (§7.3.10).
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] if the object cannot be read where its entry points, or refers to itself while it is read.
    pub(crate) fn get(&mut self, reference: Ref) -> Result<Rc<Object>, Error> {
        let Some(&entry) = self.entries.get(&reference.number) else {
            return Ok(Rc::new(Object::Null));
        };
        match entry {
            Entry::Free => return Ok(Rc::new(Object::Null)),
            Entry::InFile { generation, .. } if generation != reference.generation => {
                return Ok(Rc::new(Object::Null));
            }
            Entry::InStream { .. } if reference.generation != 0 => {
                return Ok(Rc::new(Object::Null));
            }
            _ => {}
        }
        if let Some(object) = self.objects.get(&reference.number) {
            return Ok(Rc::clone(object));
        }
        if !self.loading.insert(reference.number) {
            return Err(Error::malformed(format!(
                "object {} refers to itself while it is read",
                reference.number
            )));
        }
        if self.loading.len() > self.limits.max_nesting {
            return Err(Error::Limit(Limit::Nesting));
        }
        let loaded = self.load(entry, reference);
        self.loading.remove(&reference.number);
        let object = Rc::new(loaded?);
        self.objects.insert(reference.number, Rc::clone(&object));
        Ok(object)
    }

    fn load(&mut self, entry: Entry, reference: Ref) -> Result<Object, Error> {
        match entry {
            Entry::Free => Ok(Object::Null),
            Entry::InFile { offset, .. } => {
                let mut lexer = Lexer::new(self.data, offset);
                let (_, object) = self.parse_indirect(&mut lexer, Some(reference), true)?;
                Ok(object)
            }
            Entry::InStream { stream, index } => self.load_member(stream, index, reference.number),
        }
    }

    /// Member `index` of object stream `stream`, which must be object `number` (§7.5.7).
    fn load_member(&mut self, stream: u32, index: u32, number: u32) -> Result<Object, Error> {
        let object_stream = self.object_stream(stream)?;
        let &(member, offset) = usize::try_from(index)
            .ok()
            .and_then(|index| object_stream.members.get(index))
            .ok_or_else(|| {
                Error::malformed(format!("object {number} is not in object stream {stream}"))
            })?;
        if member != number {
            return Err(Error::malformed(format!(
                "object stream {stream} holds another object where object {number} should be"
            )));
        }
        let mut lexer = Lexer::new(&object_stream.data, offset);
        let object = parse_object(&mut lexer, true, self.limits.max_nesting, &mut self.values)?;
        if lexer.eat_keyword(b"stream") {
            return Err(Error::malformed(format!(
                "object stream {stream} holds a stream"
            )));
        }
        // Members can overlap as objects in the file can, and each counts what its parse read, looking ahead included.
        self.budget.spend(lexer.furthest().saturating_sub(offset))?;
        Ok(object)
    }

    /// The decoded object stream `number`, read once and kept.
    fn object_stream(&mut self, number: u32) -> Result<Rc<ObjectStream>, Error> {
        if let Some(stream) = self.object_streams.get(&number) {
            return Ok(Rc::clone(stream));
        }
        let Some(Entry::InFile { .. }) = self.entries.get(&number) else {
            return Err(Error::malformed(format!(
                "object stream {number} is not an object of the file"
            )));
        };
        let object = self.get(Ref {
            number,
            generation: 0,
        })?;
        let Object::Stream(stream) = &*object else {
            return Err(Error::malformed(format!(
                "object stream {number} is not a stream"
            )));
        };
        if !stream.dict.has_name(b"Type", b"ObjStm") {
            return Err(Error::malformed(format!(
                "object stream {number}'s `Type` is not `ObjStm`"
            )));
        }
        let count = stream
            .dict
            .get(b"N")
            .and_then(Object::as_integer)
            .and_then(|count| usize::try_from(count).ok())
            .ok_or_else(|| Error::malformed(format!("object stream {number} has no count `N`")))?;
        let first = stream
            .dict
            .get(b"First")
            .and_then(Object::as_integer)
            .and_then(|first| usize::try_from(first).ok())
            .ok_or_else(|| {
                Error::malformed(format!("object stream {number} has no offset `First`"))
            })?;
        // Every member is an object of the file, so all object streams together may declare no more members than the cross-reference sections may list objects.
        self.members = self
            .members
            .checked_add(count)
            .filter(|&members| members <= self.limits.max_objects)
            .ok_or(Error::Limit(Limit::Objects))?;
        let data = self.decode_stream(stream, false)?;
        let mut lexer = Lexer::new(&data, 0);
        let mut members = Vec::with_capacity(count.min(data.len() / 4 + 1));
        for _ in 0..count {
            let (Some(Token::Integer(member)), Some(Token::Integer(offset))) =
                (lexer.next_token()?, lexer.next_token()?)
            else {
                return Err(Error::malformed(format!(
                    "object stream {number} has a malformed header"
                )));
            };
            let member = u32::try_from(member).map_err(|_| {
                Error::malformed(format!(
                    "object stream {number} names an object out of range"
                ))
            })?;
            let offset = usize::try_from(offset)
                .ok()
                .and_then(|offset| first.checked_add(offset))
                .filter(|&offset| offset < data.len())
                .ok_or_else(|| {
                    Error::malformed(format!("object stream {number} has an offset out of range"))
                })?;
            members.push((member, offset));
        }
        if lexer.position() > first {
            return Err(Error::malformed(format!(
                "object stream {number}'s header runs past `First`"
            )));
        }
        let object_stream = Rc::new(ObjectStream { data, members });
        self.object_streams
            .insert(number, Rc::clone(&object_stream));
        Ok(object_stream)
    }

    /// Follows `object` if it is a reference, through at most `max_nesting` references.
    ///
    /// # Errors
    ///
    /// As [`File::get`], and [`Error::Limit`] with [`Limit::Nesting`] for a longer chain of references.
    pub(crate) fn resolve<'o>(&mut self, object: &'o Object) -> Result<Value<'o>, Error> {
        let &Object::Ref(mut reference) = object else {
            return Ok(Value::Direct(object));
        };
        for _ in 0..self.limits.max_nesting {
            let target = self.get(reference)?;
            match &*target {
                Object::Ref(next) => reference = *next,
                _ => return Ok(Value::Indirect(target)),
            }
        }
        Err(Error::Limit(Limit::Nesting))
    }

    /// A dictionary entry, resolved; `None` if it is missing or null.
    ///
    /// # Errors
    ///
    /// As [`File::resolve`].
    pub(crate) fn lookup<'o>(
        &mut self,
        dict: &'o Dict,
        key: &[u8],
    ) -> Result<Option<Value<'o>>, Error> {
        match dict.get(key) {
            None => Ok(None),
            Some(object) => {
                let value = self.resolve(object)?;
                Ok(if matches!(&*value, Object::Null) {
                    None
                } else {
                    Some(value)
                })
            }
        }
    }

    /// The decoded data of `stream`. Every use reads and decodes it again, and every use takes its bytes from the budget of [`Limits::max_read_bytes`].
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for unknown filters or damaged data, [`Error::Unsupported`] for filters the reader does not decode, [`Error::Limit`] past the decoding budget.
    pub(crate) fn stream_data(&mut self, stream: &Stream) -> Result<Vec<u8>, Error> {
        self.decode_stream(stream, false)
    }

    /// The decoded data of the stream `reference` (whose stream is `stream`), decoded once and kept, for streams used again and again: a form XObject drawn many times. Every later use takes the data's length from the budget again, since it is read again.
    ///
    /// # Errors
    ///
    /// As [`File::stream_data`].
    pub(crate) fn kept_stream_data(
        &mut self,
        reference: Ref,
        stream: &Stream,
    ) -> Result<Rc<Vec<u8>>, Error> {
        if let Some(data) = self.kept_streams.get(&reference) {
            let data = Rc::clone(data);
            self.budget.spend(data.len())?;
            return Ok(data);
        }
        let data = Rc::new(self.decode_stream(stream, false)?);
        self.kept_streams.insert(reference, Rc::clone(&data));
        Ok(data)
    }

    /// Decodes `stream`; for a cross-reference stream (`direct_only`), its filters and parameters must be direct objects.
    ///
    /// The stream's data counts toward the budget as it lies in the file, filtered or not (a decoder reads every byte of it), and so does every filter's output.
    fn decode_stream(&mut self, stream: &Stream, direct_only: bool) -> Result<Vec<u8>, Error> {
        let raw = self
            .data
            .get(stream.start..stream.start + stream.length)
            .ok_or_else(|| Error::malformed("a stream lies outside the file"))?;
        let filters = self.filters(&stream.dict, direct_only)?;
        if filters.is_empty() && raw.len() > self.budget.max_stream {
            return Err(Error::Limit(Limit::StreamSize));
        }
        self.budget.spend(raw.len())?;
        if filters.is_empty() {
            return Ok(raw.to_vec());
        }
        filter::decode(raw, &filters, &mut self.budget)
    }

    /// A stream's filters, each FlateDecode with its predictor; no more than [`MAX_FILTERS`]. Nothing is copied: the filter names and parameters are read where they are.
    fn filters(
        &mut self,
        dict: &Dict,
        direct_only: bool,
    ) -> Result<Vec<(Filter, Option<Predictor>)>, Error> {
        let direct = |object: &Object| -> Result<(), Error> {
            if direct_only && matches!(object, Object::Ref(_)) {
                return Err(Error::malformed(
                    "a cross-reference stream's filters must be direct objects",
                ));
            }
            Ok(())
        };
        let Some(names) = dict.get(b"Filter") else {
            return Ok(Vec::new());
        };
        direct(names)?;
        let names = self.resolve(names)?;
        let names: &[Object] = match &*names {
            Object::Name(_) => core::slice::from_ref(&*names),
            Object::Array(items) => items,
            Object::Null => return Ok(Vec::new()),
            _ => {
                return Err(Error::malformed(
                    "a stream's `Filter` is not a name or an array",
                ));
            }
        };
        if names.len() > MAX_FILTERS {
            return Err(Error::malformed(format!(
                "a stream has more than {MAX_FILTERS} filters"
            )));
        }
        let params = match dict.get(b"DecodeParms") {
            None => None,
            Some(object) => {
                direct(object)?;
                Some(self.resolve(object)?)
            }
        };
        let params: &[Object] = match params.as_deref() {
            None | Some(Object::Null) => &[],
            Some(Object::Array(items)) => items,
            Some(single) => core::slice::from_ref(single),
        };
        if !params.is_empty() && params.len() != names.len() {
            return Err(Error::malformed(
                "a stream's `DecodeParms` do not match its filters",
            ));
        }
        let mut filters = Vec::with_capacity(names.len());
        for (index, name) in names.iter().enumerate() {
            direct(name)?;
            let filter = Filter::from_name(
                self.resolve(name)?
                    .as_name()
                    .ok_or_else(|| Error::malformed("a stream's filter is not a name"))?,
            )?;
            let predictor = match params.get(index) {
                None => None,
                Some(object) => {
                    direct(object)?;
                    match &*self.resolve(object)? {
                        // Only FlateDecode among the filters read has parameters: a predictor.
                        Object::Dict(params) if filter == Filter::Flate => {
                            Predictor::from_params(params)?
                        }
                        Object::Dict(_) | Object::Null => None,
                        _ => {
                            return Err(Error::malformed(
                                "a stream's `DecodeParms` entry is not a dictionary",
                            ));
                        }
                    }
                }
            };
            filters.push((filter, predictor));
        }
        Ok(filters)
    }
}

/// The file must start with `%PDF-1.x` or `%PDF-2.0`.
fn check_header(data: &[u8]) -> Result<(), Error> {
    let header = data.get(..8);
    let valid = matches!(
        header,
        Some([b'%', b'P', b'D', b'F', b'-', b'1', b'.', b'0'..=b'7']) | Some(b"%PDF-2.0")
    );
    if valid {
        Ok(())
    } else {
        Err(Error::malformed(
            "the file does not start with a PDF header (`%PDF-1.x` or `%PDF-2.0`)",
        ))
    }
}

/// The offset after the last `startxref` near the end of the file, which must be followed by `%%EOF` and nothing but white space.
fn find_startxref(data: &[u8]) -> Result<usize, Error> {
    let tail_start = data.len().saturating_sub(TAIL);
    let tail = data.get(tail_start..).unwrap_or_default();
    let keyword = b"startxref";
    let found = tail
        .windows(keyword.len())
        .rposition(|window| window == keyword)
        .ok_or_else(|| Error::malformed("the file does not end with `startxref`"))?;
    let mut lexer = Lexer::new(data, tail_start + found + keyword.len());
    let Some(Token::Integer(offset)) = lexer.next_token()? else {
        return Err(Error::malformed("`startxref` is not followed by an offset"));
    };
    let mut position = lexer.position();
    while data.get(position).is_some_and(|&byte| is_whitespace(byte)) {
        position += 1;
    }
    if data.get(position..position + 5) != Some(b"%%EOF") {
        return Err(Error::malformed("the file does not end with `%%EOF`"));
    }
    if !data[position + 5..].iter().all(|&byte| is_whitespace(byte)) {
        return Err(Error::malformed("there is data after the last `%%EOF`"));
    }
    usize::try_from(offset)
        .ok()
        .filter(|&offset| offset < data.len())
        .ok_or_else(|| Error::malformed("`startxref` points outside the file"))
}

/// A `Prev` or `XRefStm` offset.
fn offset_of(object: &Object, length: usize) -> Result<usize, Error> {
    object
        .as_integer()
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|&offset| offset < length)
        .ok_or_else(|| Error::malformed("a cross-reference offset is not a position in the file"))
}

/// One 20-byte entry of a cross-reference table: `Some((offset, generation))` for an object in use, `None` for a free one.
fn parse_table_entry(line: &[u8]) -> Result<Option<(usize, u16)>, Error> {
    let malformed = || Error::malformed("a cross-reference table has a malformed entry");
    let (digits, rest) = line.split_at(10);
    let (space, rest) = rest.split_at(1);
    let (generation, rest) = rest.split_at(5);
    let (space_2, rest) = rest.split_at(1);
    let (kind, end) = rest.split_at(1);
    if space != b" " || space_2 != b" " || !matches!(end, b" \r" | b" \n" | b"\r\n") {
        return Err(malformed());
    }
    let number = |bytes: &[u8]| -> Option<u64> {
        bytes.iter().try_fold(0_u64, |value, &byte| {
            byte.is_ascii_digit()
                .then(|| value * 10 + u64::from(byte - b'0'))
        })
    };
    let offset = number(digits).ok_or_else(malformed)?;
    let generation = number(generation).ok_or_else(malformed)?;
    match kind {
        b"n" => {
            let offset = usize::try_from(offset).map_err(|_| malformed())?;
            let generation = u16::try_from(generation).map_err(|_| malformed())?;
            Ok(Some((offset, generation)))
        }
        b"f" => Ok(None),
        _ => Err(malformed()),
    }
}

/// A big-endian number of up to eight bytes.
fn big_endian(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0, |value, &byte| (value << 8) | u64::from(byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::test_pdf;

    #[test]
    fn reads_table_entries() {
        assert_eq!(
            parse_table_entry(b"0000000017 00000 n\r\n").unwrap(),
            Some((17, 0))
        );
        assert_eq!(parse_table_entry(b"0000000000 65535 f \n").unwrap(), None);
        assert_eq!(
            parse_table_entry(b"0000001234 00002 n \r").unwrap(),
            Some((1234, 2))
        );
        for bad in [
            b"000000017 00000 n \r\n",
            b"0000000017 00000 x\r\n",
            b"0000000017 99999 n\r\n",
            b"00000000a7 00000 n\r\n",
            b"0000000017 00000 n\n\n",
        ] {
            assert!(
                parse_table_entry(bad).is_err(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn checks_the_header_and_the_end() {
        assert!(check_header(b"%PDF-1.7\n").is_ok());
        assert!(check_header(b"%PDF-2.0\n").is_ok());
        for bad in [
            &b"%PDF-1.8"[..],
            b"%PDF-3.0",
            b"\xEF\xBB\xBF%PDF-1.7",
            b"%PDF",
        ] {
            assert!(check_header(bad).is_err());
        }
        assert_eq!(
            find_startxref(b"...startxref\n123\n%%EOF\n").unwrap_err(),
            Error::malformed("`startxref` points outside the file")
        );
        assert_eq!(
            find_startxref(b"................startxref\n3\n%%EOF\r\n").unwrap(),
            3
        );
        assert!(find_startxref(b"startxref\n3\n%%EOF garbage").is_err());
        assert!(find_startxref(b"startxref\n3\n").is_err());
        assert!(find_startxref(b"no keyword").is_err());
    }

    /// The bytes read so far, of the budget of `limits`.
    fn read(file: &File<'_>, limits: &Limits) -> u64 {
        limits.max_read_bytes - file.budget.remaining
    }

    #[test]
    fn every_object_counts_the_bytes_its_parse_reads_again_where_objects_overlap() {
        // Object 3 lies inside the string of object 2, where its cross-reference entry points.
        let inner = format!("3 0 obj ({}) endobj", "A".repeat(1000));
        let outer = format!("2 0 obj\n({inner})\nendobj");
        let mut data = b"%PDF-1.7\n".to_vec();
        let catalog = data.len();
        data.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
        let second = data.len();
        data.extend_from_slice(outer.as_bytes());
        data.push(b'\n');
        let third = second + "2 0 obj\n(".len();
        let xref = data.len();
        data.extend_from_slice(
            format!("xref\n0 4\n0000000000 65535 f \n{catalog:010} 00000 n \n{second:010} 00000 n \n{third:010} 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        let limits = Limits::DEFAULT;
        let mut file = File::open(&data, limits).unwrap();
        // Opening read the table and the trailer.
        let opened = read(&file, &limits);
        assert!(opened > 100);
        file.get(Ref {
            number: 2,
            generation: 0,
        })
        .unwrap();
        assert_eq!(
            read(&file, &limits),
            opened + u64::try_from(outer.len()).unwrap()
        );
        // Object 3's bytes count again, although object 2's parse read them already.
        file.get(Ref {
            number: 3,
            generation: 0,
        })
        .unwrap();
        let all = opened + u64::try_from(outer.len() + inner.len()).unwrap();
        assert_eq!(read(&file, &limits), all);
        // An object read again comes from the cache, and reads nothing.
        file.get(Ref {
            number: 2,
            generation: 0,
        })
        .unwrap();
        assert_eq!(read(&file, &limits), all);
        // With one byte less, the second object passes the limit.
        let short = Limits {
            max_read_bytes: all - 1,
            ..limits
        };
        let mut file = File::open(&data, short).unwrap();
        file.get(Ref {
            number: 2,
            generation: 0,
        })
        .unwrap();
        assert_eq!(
            file.get(Ref {
                number: 3,
                generation: 0
            })
            .unwrap_err(),
            Error::Limit(Limit::ReadTotal)
        );
        // A table and its trailer count too.
        let tiny = Limits {
            max_read_bytes: opened - 1,
            ..limits
        };
        assert_eq!(
            File::open(&data, tiny).err(),
            Some(Error::Limit(Limit::ReadTotal))
        );
    }

    #[test]
    fn members_of_object_streams_count_what_their_parse_reads_looking_ahead_included() {
        let data = test_pdf(&["<< /Type /Catalog >>"]);
        let limits = Limits::DEFAULT;
        let mut file = File::open(&data, limits).unwrap();
        // Two members at the same offset: the integer 5, after which the parse looks for a generation number and `R`, and then for `stream`, through a comment to the end.
        let stream = b"5 % a comment\n".to_vec();
        let length = u64::try_from(stream.len()).unwrap();
        file.object_streams.insert(
            9,
            Rc::new(ObjectStream {
                data: stream,
                members: vec![(10, 0), (11, 0)],
            }),
        );
        for (number, index) in [(10, 0), (11, 1)] {
            file.entries
                .insert(number, Entry::InStream { stream: 9, index });
        }
        let opened = read(&file, &limits);
        assert_eq!(
            *file
                .get(Ref {
                    number: 10,
                    generation: 0
                })
                .unwrap(),
            Object::Integer(5)
        );
        assert_eq!(read(&file, &limits), opened + length);
        file.get(Ref {
            number: 11,
            generation: 0,
        })
        .unwrap();
        assert_eq!(read(&file, &limits), opened + 2 * length);
    }

    #[test]
    fn all_object_streams_together_declare_at_most_max_objects_members() {
        // Two object streams of three members each: either fits a limit of five objects, both together do not.
        let object_stream = |members: [u32; 3]| {
            let header = format!("{} 0 {} 2 {} 4\n", members[0], members[1], members[2]);
            let data = format!("{header}1 2 3");
            format!(
                "<< /Type /ObjStm /N 3 /First {} /Length {} >>\nstream\n{data}\nendstream",
                header.len(),
                data.len()
            )
        };
        let data = test_pdf(&[
            "<< /Type /Catalog >>",
            &object_stream([20, 22, 24]),
            &object_stream([21, 23, 25]),
        ]);
        let limits = Limits {
            max_objects: 5,
            ..Limits::DEFAULT
        };
        let mut file = File::open(&data, limits).unwrap();
        file.entries.insert(
            20,
            Entry::InStream {
                stream: 2,
                index: 0,
            },
        );
        file.entries.insert(
            21,
            Entry::InStream {
                stream: 3,
                index: 0,
            },
        );
        assert_eq!(
            *file
                .get(Ref {
                    number: 20,
                    generation: 0
                })
                .unwrap(),
            Object::Integer(1)
        );
        assert_eq!(
            file.get(Ref {
                number: 21,
                generation: 0
            })
            .unwrap_err(),
            Error::Limit(Limit::Objects)
        );
        // Alone, the second stream fits.
        let mut file = File::open(&data, limits).unwrap();
        file.entries.insert(
            21,
            Entry::InStream {
                stream: 3,
                index: 0,
            },
        );
        assert_eq!(
            *file
                .get(Ref {
                    number: 21,
                    generation: 0
                })
                .unwrap(),
            Object::Integer(1)
        );
    }
}
