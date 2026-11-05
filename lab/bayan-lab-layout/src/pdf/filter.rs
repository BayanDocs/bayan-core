//! Stream filters (ISO 32000-2 §7.4): the ones extraction needs, each with its output limited.
//!
//! FlateDecode (with the PNG and TIFF predictors), ASCIIHexDecode, ASCII85Decode and RunLengthDecode are decoded. LZWDecode, the image codecs (DCTDecode, JPXDecode, JBIG2Decode, CCITTFaxDecode) and Crypt are refused: extraction never needs to decode an image, and the producers the lab reads compress with FlateDecode.

use miniz_oxide::inflate::{TINFLStatus, decompress_to_vec_zlib_with_limit};

use super::object::{Dict, Object};
use super::{Error, Limit};

/// How many bytes may still be read: what one filter may produce ([`super::Limits::max_stream_bytes`]), and what the reader may still read in all, the output of every filter included ([`super::Limits::max_read_bytes`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    /// The most bytes one filter may produce.
    pub(crate) max_stream: usize,
    /// The bytes the reader may still read, parse or decode, all together.
    pub(crate) remaining: u64,
}

impl Budget {
    /// The most bytes the next filter may produce, and the limit that stops it there.
    fn allowance(&self) -> (usize, Limit) {
        let remaining = usize::try_from(self.remaining).unwrap_or(usize::MAX);
        if remaining < self.max_stream {
            (remaining, Limit::ReadTotal)
        } else {
            (self.max_stream, Limit::StreamSize)
        }
    }

    /// Takes `read` bytes from the total.
    pub(crate) fn spend(&mut self, read: usize) -> Result<(), Error> {
        let read = u64::try_from(read).unwrap_or(u64::MAX);
        self.remaining = self
            .remaining
            .checked_sub(read)
            .ok_or(Error::Limit(Limit::ReadTotal))?;
        Ok(())
    }

    /// Checks that `length` more bytes fit, before they are produced.
    fn check(&self, length: usize) -> Result<(), Error> {
        let (allowed, limit) = self.allowance();
        if length > allowed {
            return Err(Error::Limit(limit));
        }
        Ok(())
    }
}

/// A filter the reader decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Filter {
    /// FlateDecode: zlib-wrapped DEFLATE.
    Flate,
    /// ASCIIHexDecode.
    AsciiHex,
    /// ASCII85Decode.
    Ascii85,
    /// RunLengthDecode.
    RunLength,
}

impl Filter {
    /// The filter a name stands for (the abbreviations are those of inline images, §8.9.7).
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] for the filters the reader does not decode, [`Error::Malformed`] for names that are no filter.
    pub(crate) fn from_name(name: &[u8]) -> Result<Self, Error> {
        match name {
            b"FlateDecode" | b"Fl" => Ok(Self::Flate),
            b"ASCIIHexDecode" | b"AHx" => Ok(Self::AsciiHex),
            b"ASCII85Decode" | b"A85" => Ok(Self::Ascii85),
            b"RunLengthDecode" | b"RL" => Ok(Self::RunLength),
            b"LZWDecode" | b"LZW" => Err(Error::unsupported("LZWDecode compression")),
            b"DCTDecode" | b"DCT" | b"JPXDecode" | b"JBIG2Decode" | b"CCITTFaxDecode" | b"CCF" => {
                Err(Error::unsupported(
                    "an image compression in a stream that is not an image",
                ))
            }
            b"Crypt" => Err(Error::unsupported("encrypted streams (the Crypt filter)")),
            _ => Err(Error::malformed("a stream names an unknown filter")),
        }
    }
}

/// Decodes `data` through `filters`, in order, each FlateDecode with its predictor, if any. Each filter's output is taken from the budget.
///
/// # Errors
///
/// [`Error::Limit`] when a filter's output would pass the budget, [`Error::Malformed`] for damaged data.
pub(crate) fn decode(
    data: &[u8],
    filters: &[(Filter, Option<Predictor>)],
    budget: &mut Budget,
) -> Result<Vec<u8>, Error> {
    let mut current: Option<Vec<u8>> = None;
    for (filter, predictor) in filters {
        let input = current.as_deref().unwrap_or(data);
        let output = match filter {
            Filter::Flate => {
                let inflated = inflate(input, budget)?;
                match predictor {
                    Some(predictor) => predict(inflated, predictor, budget)?,
                    None => inflated,
                }
            }
            Filter::AsciiHex => ascii_hex(input, budget)?,
            Filter::Ascii85 => ascii_85(input, budget)?,
            Filter::RunLength => run_length(input, budget)?,
        };
        budget.spend(output.len())?;
        current = Some(output);
    }
    let mut decoded = current.unwrap_or_else(|| data.to_vec());
    // The decoders grow their output as they go; what is kept holds no spare capacity.
    decoded.shrink_to_fit();
    Ok(decoded)
}

/// FlateDecode: a zlib stream (RFC 1950) whose Adler-32 checksum must match.
fn inflate(input: &[u8], budget: &Budget) -> Result<Vec<u8>, Error> {
    let (allowed, limit) = budget.allowance();
    decompress_to_vec_zlib_with_limit(input, allowed).map_err(|error| match error.status {
        TINFLStatus::HasMoreOutput => Error::Limit(limit),
        _ => Error::malformed("a FlateDecode stream is damaged or not in zlib format"),
    })
}

/// A FlateDecode stream's predictor (§7.4.4.4), read from its `DecodeParms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Predictor {
    /// 2 for TIFF, 10 to 15 for PNG.
    kind: i64,
    /// Bytes per row, without the PNG tag byte.
    row: usize,
    /// Bytes per pixel, at least 1.
    pixel: usize,
}

/// A whole-number parameter of a predictor, or its default.
fn parameter(params: &Dict, key: &[u8], default: i64) -> Result<i64, Error> {
    match params.get(key) {
        None => Ok(default),
        Some(Object::Integer(value)) => Ok(*value),
        Some(_) => Err(Error::malformed("a predictor parameter is not an integer")),
    }
}

impl Predictor {
    /// The predictor that `params` describe; `None` for predictor 1, which predicts nothing.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for unknown predictors and parameters out of range, [`Error::Unsupported`] for the TIFF predictor with other than 8 bits per component.
    pub(crate) fn from_params(params: &Dict) -> Result<Option<Self>, Error> {
        let kind = parameter(params, b"Predictor", 1)?;
        if kind == 1 {
            return Ok(None);
        }
        if kind != 2 && !(10..=15).contains(&kind) {
            return Err(Error::malformed("a stream has an unknown predictor"));
        }
        let colors = parameter(params, b"Colors", 1)?;
        let bits = parameter(params, b"BitsPerComponent", 8)?;
        let columns = parameter(params, b"Columns", 1)?;
        if !(1..=32).contains(&colors) || !matches!(bits, 1 | 2 | 4 | 8 | 16) || columns < 1 {
            return Err(Error::malformed(
                "a predictor's parameters are out of range",
            ));
        }
        if kind == 2 && bits != 8 {
            return Err(Error::unsupported(
                "the TIFF predictor with other than 8 bits per component",
            ));
        }
        // `Columns` can be any integer: every step that uses it is checked.
        let too_long = || Error::malformed("a predictor's row is too long");
        let bits_per_pixel = colors * bits;
        let row_bits = bits_per_pixel.checked_mul(columns).ok_or_else(too_long)?;
        let row = row_bits
            .checked_add(7)
            .map(|bits| bits / 8)
            .and_then(|bytes| usize::try_from(bytes).ok())
            // A PNG row has a tag byte before its bytes, so its length plus one must fit too.
            .filter(|row| row.checked_add(1).is_some())
            .ok_or_else(too_long)?;
        let pixel = usize::try_from((bits_per_pixel + 7) / 8)
            .map_err(|_| Error::malformed("a predictor's pixel is too large"))?;
        Ok(Some(Self { kind, row, pixel }))
    }
}

/// Undoes a predictor after FlateDecode.
fn predict(data: Vec<u8>, predictor: &Predictor, budget: &Budget) -> Result<Vec<u8>, Error> {
    if predictor.kind == 2 {
        return Ok(tiff(data, predictor));
    }
    // No rows: nothing to undo, and no row to allocate, however long the parameters say it is.
    if data.is_empty() {
        return Ok(data);
    }
    let stride = predictor
        .row
        .checked_add(1)
        .ok_or_else(|| Error::malformed("a predictor's row is too long"))?;
    // A whole number of rows, at least one, so that a row is never longer than the data.
    if predictor.row == 0 || !data.len().is_multiple_of(stride) {
        return Err(Error::malformed(
            "a PNG-predicted stream is not a whole number of rows",
        ));
    }
    let rows = data.len() / stride;
    budget.check(rows * predictor.row)?;
    let mut out = vec![0_u8; rows * predictor.row];
    let mut previous = vec![0_u8; predictor.row];
    for (row_index, row) in data.chunks_exact(stride).enumerate() {
        let (tag, bytes) = row
            .split_first()
            .ok_or_else(|| Error::malformed("an empty row"))?;
        let start = row_index * predictor.row;
        let current = out
            .get_mut(start..start + predictor.row)
            .ok_or_else(|| Error::malformed("a PNG row is out of range"))?;
        for (index, &byte) in bytes.iter().enumerate() {
            let left = if index >= predictor.pixel {
                current[index - predictor.pixel]
            } else {
                0
            };
            let up = previous[index];
            let upper_left = if index >= predictor.pixel {
                previous[index - predictor.pixel]
            } else {
                0
            };
            current[index] = match tag {
                0 => byte,
                1 => byte.wrapping_add(left),
                2 => byte.wrapping_add(up),
                3 => byte.wrapping_add(average(left, up)),
                4 => byte.wrapping_add(paeth(left, up, upper_left)),
                _ => return Err(Error::malformed("a PNG row has an unknown filter type")),
            };
        }
        previous.copy_from_slice(current);
    }
    Ok(out)
}

/// The PNG Average filter's predictor: the floor of the mean.
fn average(left: u8, up: u8) -> u8 {
    let mean = (u16::from(left) + u16::from(up)) / 2;
    mean.to_le_bytes()[0]
}

/// The PNG Paeth predictor (RFC 2083 §6.6).
fn paeth(left: u8, up: u8, upper_left: u8) -> u8 {
    let (a, b, c) = (i16::from(left), i16::from(up), i16::from(upper_left));
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        left
    } else if pb <= pc {
        up
    } else {
        upper_left
    }
}

/// The TIFF predictor 2 for 8-bit components: each byte adds the same component of the pixel before it in its row.
fn tiff(mut data: Vec<u8>, predictor: &Predictor) -> Vec<u8> {
    for row in data.chunks_mut(predictor.row) {
        for index in predictor.pixel..row.len() {
            row[index] = row[index].wrapping_add(row[index - predictor.pixel]);
        }
    }
    data
}

/// ASCIIHexDecode: hexadecimal digits and white space up to `>`.
fn ascii_hex(input: &[u8], budget: &Budget) -> Result<Vec<u8>, Error> {
    let (allowed, limit) = budget.allowance();
    let mut out = Vec::new();
    let mut high: Option<u8> = None;
    for &byte in input {
        if byte == b'>' {
            if let Some(first) = high {
                out.push(first << 4);
            }
            return Ok(out);
        }
        if super::lexer::is_whitespace(byte) {
            continue;
        }
        let value = super::lexer::hex_value(byte).ok_or_else(|| {
            Error::malformed(
                "an ASCIIHexDecode stream holds a byte that is not a hexadecimal digit",
            )
        })?;
        match high.take() {
            Some(first) => {
                if out.len() >= allowed {
                    return Err(Error::Limit(limit));
                }
                out.push((first << 4) | value);
            }
            None => high = Some(value),
        }
    }
    Err(Error::malformed(
        "an ASCIIHexDecode stream has no end marker",
    ))
}

/// ASCII85Decode: groups of five characters for four bytes, `z` for four zeros, up to `~>`.
fn ascii_85(input: &[u8], budget: &Budget) -> Result<Vec<u8>, Error> {
    let (allowed, limit) = budget.allowance();
    let mut out = Vec::new();
    let push = |out: &mut Vec<u8>, bytes: &[u8]| {
        if out.len() + bytes.len() > allowed {
            return Err(Error::Limit(limit));
        }
        out.extend_from_slice(bytes);
        Ok(())
    };
    let mut group = [0_u8; 5];
    let mut count = 0_usize;
    let mut bytes = input.iter();
    while let Some(&byte) = bytes.next() {
        match byte {
            b'~' => {
                if bytes.next() != Some(&b'>') {
                    return Err(Error::malformed(
                        "an ASCII85Decode stream has a `~` without `>`",
                    ));
                }
                if count == 1 {
                    return Err(Error::malformed(
                        "an ASCII85Decode stream ends in a single character",
                    ));
                }
                if count > 1 {
                    for slot in group.iter_mut().skip(count) {
                        *slot = b'u' - b'!';
                    }
                    let word = ascii_85_word(&group)?.to_be_bytes();
                    push(&mut out, &word[..count - 1])?;
                }
                return Ok(out);
            }
            b'z' if count == 0 => push(&mut out, &[0; 4])?,
            b'!'..=b'u' => {
                group[count] = byte - b'!';
                count += 1;
                if count == 5 {
                    push(&mut out, &ascii_85_word(&group)?.to_be_bytes())?;
                    count = 0;
                }
            }
            _ if super::lexer::is_whitespace(byte) => {}
            _ => {
                return Err(Error::malformed(
                    "an ASCII85Decode stream holds a byte outside its alphabet",
                ));
            }
        }
    }
    Err(Error::malformed(
        "an ASCII85Decode stream has no end marker",
    ))
}

/// The four bytes of one group of five base-85 digits.
fn ascii_85_word(group: &[u8; 5]) -> Result<u32, Error> {
    let value = group
        .iter()
        .fold(0_u64, |value, &digit| value * 85 + u64::from(digit));
    u32::try_from(value).map_err(|_| Error::malformed("an ASCII85Decode group is out of range"))
}

/// RunLengthDecode (§7.4.5), up to its end-of-data byte 128.
fn run_length(input: &[u8], budget: &Budget) -> Result<Vec<u8>, Error> {
    let (allowed, limit) = budget.allowance();
    let mut out = Vec::new();
    let mut bytes = input.iter();
    while let Some(&length) = bytes.next() {
        match length {
            128 => return Ok(out),
            0..=127 => {
                let count = usize::from(length) + 1;
                if out.len() + count > allowed {
                    return Err(Error::Limit(limit));
                }
                for _ in 0..count {
                    let &byte = bytes
                        .next()
                        .ok_or_else(|| Error::malformed("a RunLengthDecode run is cut short"))?;
                    out.push(byte);
                }
            }
            _ => {
                let count = 257 - usize::from(length);
                if out.len() + count > allowed {
                    return Err(Error::Limit(limit));
                }
                let &byte = bytes
                    .next()
                    .ok_or_else(|| Error::malformed("a RunLengthDecode run is cut short"))?;
                out.resize(out.len() + count, byte);
            }
        }
    }
    Err(Error::malformed(
        "a RunLengthDecode stream has no end marker",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> Budget {
        Budget {
            max_stream: 1 << 20,
            remaining: 1 << 24,
        }
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
    }

    #[test]
    fn decodes_each_filter() {
        let mut budget = budget();
        assert_eq!(
            decode(
                &zlib(b"BT /F1 12 Tf ET"),
                &[(Filter::Flate, None)],
                &mut budget
            )
            .unwrap(),
            b"BT /F1 12 Tf ET"
        );
        assert_eq!(
            decode(b"48 65 6C6C6F>", &[(Filter::AsciiHex, None)], &mut budget).unwrap(),
            b"Hello"
        );
        assert_eq!(
            decode(b"414>", &[(Filter::AsciiHex, None)], &mut budget).unwrap(),
            b"A@"
        );
        assert_eq!(
            decode(
                b"87cURD]i,\"Ebo80~>",
                &[(Filter::Ascii85, None)],
                &mut budget
            )
            .unwrap(),
            b"Hello World!"
        );
        assert_eq!(
            decode(
                b"87cURD]i,\"Ebo7~>",
                &[(Filter::Ascii85, None)],
                &mut budget
            )
            .unwrap(),
            b"Hello World"
        );
        assert_eq!(
            decode(b"z!!~>", &[(Filter::Ascii85, None)], &mut budget).unwrap(),
            [0, 0, 0, 0, 0]
        );
        assert_eq!(
            decode(
                &[2, b'a', b'b', b'c', 254, b'x', 128],
                &[(Filter::RunLength, None)],
                &mut budget
            )
            .unwrap(),
            b"abcxxx"
        );
        // Filters apply in order.
        let hex: Vec<u8> = zlib(b"twice")
            .iter()
            .flat_map(|byte| format!("{byte:02x}").into_bytes())
            .chain(*b">")
            .collect();
        assert_eq!(
            decode(
                &hex,
                &[(Filter::AsciiHex, None), (Filter::Flate, None)],
                &mut budget
            )
            .unwrap(),
            b"twice"
        );
    }

    /// The predictor of a `DecodeParms` dictionary with these integer entries.
    fn predictor(entries: &[(&[u8], i64)]) -> Result<Option<Predictor>, Error> {
        let entries = entries
            .iter()
            .map(|&(key, value)| (key.to_vec(), Object::Integer(value)))
            .collect();
        Predictor::from_params(&Dict::from_entries(entries).unwrap())
    }

    #[test]
    fn undoes_png_predictors() {
        // Two rows of three bytes: Sub, then Up.
        let rows = [1, 10, 5, 5, 2, 1, 1, 1];
        let png = predictor(&[(b"Predictor", 12), (b"Columns", 3)]).unwrap();
        let mut budget = budget();
        let decoded = decode(&zlib(&rows), &[(Filter::Flate, png)], &mut budget).unwrap();
        assert_eq!(decoded, [10, 15, 20, 11, 16, 21]);
        // A stream that is not a whole number of rows is refused.
        assert!(decode(&zlib(&rows[..7]), &[(Filter::Flate, png)], &mut budget).is_err());
        // No rows at all is an empty stream.
        assert!(
            decode(&zlib(&[]), &[(Filter::Flate, png)], &mut budget)
                .unwrap()
                .is_empty()
        );
        assert_eq!(predictor(&[(b"Predictor", 1)]), Ok(None));
    }

    #[test]
    fn predictor_rows_of_any_length_are_checked_arithmetic() {
        let too_long = Err(Error::malformed("a predictor's row is too long"));
        // A row of one bit per pixel and i64::MAX pixels: rounding its bits up to bytes overflows.
        let one_bit = |columns| {
            predictor(&[
                (b"Predictor", 12),
                (b"BitsPerComponent", 1),
                (b"Columns", columns),
            ])
        };
        assert_eq!(one_bit(i64::MAX), too_long);
        // The bits of a row overflow.
        assert_eq!(
            predictor(&[
                (b"Predictor", 12),
                (b"Colors", 4),
                (b"BitsPerComponent", 16),
                (b"Columns", i64::MAX / 64 + 1)
            ]),
            too_long
        );
        // A row of exactly usize::MAX bytes leaves no room for its tag byte (on 32-bit targets; on 64-bit ones the row's bits overflow an i64 first).
        let max_row = i64::try_from(usize::MAX).unwrap_or(i64::MAX);
        assert_eq!(
            predictor(&[(b"Predictor", 12), (b"Columns", max_row)]),
            too_long
        );
        // A row of 2^31 - 1 bytes fits everywhere: the data refuses it, without an overflow and without allocating the row.
        let long = predictor(&[(b"Predictor", 12), (b"Columns", i64::from(i32::MAX))]).unwrap();
        let mut budget = budget();
        assert_eq!(
            decode(&zlib(&[0, 0]), &[(Filter::Flate, long)], &mut budget).unwrap_err(),
            Error::malformed("a PNG-predicted stream is not a whole number of rows")
        );
        assert!(
            decode(&zlib(&[]), &[(Filter::Flate, long)], &mut budget)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn refuses_damaged_data() {
        let mut budget = budget();
        let mut damaged = zlib(b"some content");
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        for (data, filter) in [
            (&damaged[..], Filter::Flate),
            (&b"not zlib at all"[..], Filter::Flate),
            (b"41", Filter::AsciiHex),
            (b"4G>", Filter::AsciiHex),
            (b"87cUR", Filter::Ascii85),
            (b"8~>", Filter::Ascii85),
            (b"s8W-\"~>", Filter::Ascii85),
            (b"uuuuu~>", Filter::Ascii85),
            (&[2, b'a'][..], Filter::RunLength),
            (&[1, b'a', b'b'][..], Filter::RunLength),
        ] {
            assert!(
                decode(data, &[(filter, None)], &mut budget).is_err(),
                "{data:?}"
            );
        }
        assert_eq!(
            Filter::from_name(b"LZWDecode").unwrap_err(),
            Error::unsupported("LZWDecode compression")
        );
        assert!(matches!(
            Filter::from_name(b"DCTDecode"),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            Filter::from_name(b"Crypt"),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            Filter::from_name(b"Nonsense"),
            Err(Error::Malformed(_))
        ));
    }

    #[test]
    fn limits_each_stream_and_the_total() {
        let content = vec![b'x'; 10_000];
        let compressed = zlib(&content);
        let mut budget = Budget {
            max_stream: 9_999,
            remaining: 1 << 24,
        };
        assert_eq!(
            decode(&compressed, &[(Filter::Flate, None)], &mut budget).unwrap_err(),
            Error::Limit(Limit::StreamSize)
        );
        let mut budget = Budget {
            max_stream: 10_000,
            remaining: 15_000,
        };
        assert_eq!(
            decode(&compressed, &[(Filter::Flate, None)], &mut budget)
                .unwrap()
                .len(),
            10_000
        );
        assert_eq!(
            decode(&compressed, &[(Filter::Flate, None)], &mut budget).unwrap_err(),
            Error::Limit(Limit::ReadTotal)
        );
        let mut budget = Budget {
            max_stream: 100,
            remaining: 1 << 24,
        };
        assert_eq!(
            decode(&[129, b'x', 128], &[(Filter::RunLength, None)], &mut budget).unwrap_err(),
            Error::Limit(Limit::StreamSize)
        );
        assert_eq!(
            decode(&b"41".repeat(101), &[(Filter::AsciiHex, None)], &mut budget).unwrap_err(),
            Error::Limit(Limit::StreamSize)
        );
        assert_eq!(
            decode(&b"z".repeat(26), &[(Filter::Ascii85, None)], &mut budget).unwrap_err(),
            Error::Limit(Limit::StreamSize)
        );
    }
}
