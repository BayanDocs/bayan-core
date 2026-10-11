#!/usr/bin/env python3
"""Inspects the content of every document in a public corpus store, for the curator's licensing and privacy review.

Usage: python3 -I inspect-content.py STORE MANIFEST [--contexts]

STORE is a local store folder (`objects/ab/cdef….docx`), MANIFEST the corpus manifest. The script reads every document the manifest lists and reports, per document, what a curator must judge before the document may be published: the text of every part (every story, the properties, the relationship targets, SVG pictures, HTML and plain-text parts, and the text strings of parts of any other kind) against lists of markers (copyright and confidentiality notices, licenses, news agencies, picture libraries, standards bodies and the wording of ECMA-376's own definitions, publishers, Office.com template identifiers), e-mail addresses, bank account and payment card numbers and user folders in paths, both as the part stores it and with each paragraph's text runs joined, because Word splits a word across runs wherever its formatting, revisions or proofing marks change; every embedded object, by its content (programs, e-mail messages, documents and packages, with embedded Office documents inspected in turn), listed for every document that has one; every picture's metadata (camera, author and copyright fields, GPS positions, picture-library credits); every embedded font (complete or subset, vendor and embedding permission); and every signature certificate. It also counts, without showing them, the people that the properties, comments and tracked changes name and the account IDs of the presence information in `word/people.xml`. Names, addresses and numbers that could identify a person are masked (`Xxxx`), so the report can be shared; `--contexts` adds 60 characters around each marker, for the curator's own terminal only.

Only for the public corpus: never run it on private (T2) documents. It uses only Python's standard library and treats every document as hostile: it limits the size it reads of every part, how deeply it opens embedded packages, and how long it follows the chains inside OLE files. It changes nothing and decides nothing; the curator records decisions in excluded.tsv.
"""

import base64
import html
import io
import json
import pathlib
import re
import struct
import sys
import zipfile

MAX_OBJECT = 64 * 1024 * 1024
MAX_PART = 64 * 1024 * 1024
MAX_TOTAL = 256 * 1024 * 1024
MAX_DEPTH = 3
CONTEXT = 60

# REUSE-IgnoreStart: REUSE would read the copyright patterns as this file's own copyright.
MARKERS = [
    # Copyright, rights and confidentiality.
    ("rights", r"©|\(c\)\s*(?:19|20)\d\d|\bCopyright\b|\bAll [Rr]ights [Rr]eserved\b|\bTous droits\b|\bAlle Rechte\b|版权所有"),
    ("confidential", r"\b[Cc]onfidential\b|\bCONFIDENTIAL\b|\bProprietary\b|\bInternal [Uu]se [Oo]nly\b|\b[Dd]o [Nn]ot [Dd]istribute\b|\bNot for [Dd]istribution\b|机密"),
    # Licenses that may not fit the source's own.
    ("license", r"Creative Commons|\bCC[- ]BY\b|\bNon-?[Cc]ommercial\b|\bGNU\b|\bGPL\b|\bShareAlike\b"),
    # News agencies, picture libraries and encyclopedias.
    ("news or pictures", r"\bReuters\b|\bAFP\b|_AFP_|Agence France|Associated Press|\bAP\)|\bBloomberg\b|\bXinhua\b|新华社|\bGetty\b|Shutterstock|iStock|\bCorbis\b|Alamy|Wikipedia|Wikimedia"),
    # Standards bodies, and the wording of ECMA-376's own definitions, which a copied excerpt keeps.
    ("standard", r"ECMA-376|Ecma International|\bISO/IEC\b|\bITU-T\b|\bIEEE\b|This (?:element|attribute) specifies|\[Example:|\bend example\]"),
    # Publishers and template makers.
    ("publisher or template", r"McGraw|Pearson|Elsevier|Springer|\bWiley\b|\bSCORE\b|Teacher [Rr]esume|[Rr]ésumé|[Rr]esume [Tt]emplate|Curriculum [Vv]itae"),
]
# REUSE-IgnoreEnd
MARKER_KINDS = {kind for kind, _ in MARKERS}
PERSONAL_MAIL = {
    "gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "hotmail.co.uk", "live.com", "msn.com", "yahoo.com",
    "yahoo.co.uk", "yahoo.fr", "aol.com", "icloud.com", "me.com", "mac.com", "gmx.de", "gmx.net", "web.de", "mail.ru",
    "yandex.ru", "qq.com", "163.com", "126.com", "sina.com", "protonmail.com", "proton.me", "free.fr", "orange.fr",
    "t-online.de", "libero.it", "naver.com", "hanmail.net",
}
PLACEHOLDER_LOCAL = {"someone", "somebody", "name", "user", "username", "example", "test", "foo", "bar", "nobody", "noreply", "no-reply", "firstname.lastname", "john.doe", "jane.doe", "johndoe", "janedoe"}
EXAMPLE_DOMAIN = re.compile(r"(?:^|\.)(?:example\.(?:com|org|net)|example|invalid|test|localhost)$", re.I)
EMAIL = re.compile(r"[A-Za-z0-9._%+\-]{1,64}@[A-Za-z0-9.\-]{1,253}\.[A-Za-z]{2,24}")
IBAN = re.compile(r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]{4}){3,7}(?: ?[A-Z0-9]{1,3})?\b")
CARD = re.compile(r"\b(?:\d{4}[ -]){3}\d{1,7}\b|\b\d{13,19}\b")
USER_FOLDER = re.compile(r"(?i)(?:[A-Z]:\\{1,2}(?:Users|Documents and Settings)\\{1,2}|/Users/|/home/)([^\\/\"'<>|]{1,64})")
TEMPLATE_ID = re.compile(r"<Template>\s*(T[CM]\d{6,})")
IMAGE_EXTENSIONS = (".png", ".jpg", ".jpeg", ".jpe", ".gif", ".bmp", ".tif", ".tiff", ".emf", ".wmf", ".svg", ".wdp", ".jxr", ".ico", ".pict")
EXECUTABLE_NAMES = re.compile(r"(?i)\.(?:exe|dll|com|scr|msi|bat|cmd|ps1|vbs|vbe|js|jse|wsf|wsh|hta|jar|lnk|reg|sh|app|pif|cpl)$")


def mask(text):
    """Letters become x or X and digits 0, so a value's shape stays visible but not its content."""
    text = re.sub(r"[A-Z]", "X", text)
    text = re.sub(r"[a-z]", "x", text)
    text = re.sub(r"\d", "0", text)
    return re.sub(r"[^\x00-\x7f]", "*", text)


def printable(text, limit=120):
    text = "".join(ch if ch.isprintable() and not 0x200B <= ord(ch) <= 0x206F else "?" for ch in text)
    return text[:limit]


class Report:
    def __init__(self):
        self.lines = []
        self.flags = set()

    def add(self, flag, text):
        self.flags.add(flag)
        self.lines.append(f"   [{flag}] {text}")


# ZIP packages.

def read_member(archive, info, budget):
    if info.file_size > MAX_PART or info.file_size > budget[0]:
        return None
    with archive.open(info) as member:
        data = member.read(MAX_PART + 1)
    if len(data) > MAX_PART:
        return None
    budget[0] -= len(data)
    return data


def inspect_package(data, report, where, depth, contexts):
    try:
        archive = zipfile.ZipFile(io.BytesIO(data))
        infos = archive.infolist()
    except (zipfile.BadZipFile, ValueError, OSError) as error:
        report.add("unreadable", f"{where}: not a readable ZIP package ({error})")
        return
    budget = [MAX_TOTAL]
    parts = {}
    for info in infos:
        if info.is_dir():
            continue
        try:
            content = read_member(archive, info, budget)
        except (zipfile.BadZipFile, ValueError, OSError, NotImplementedError, EOFError) as error:
            report.add("unreadable", f"{where}{info.filename}: cannot be read ({error})")
            continue
        if content is None:
            report.add("too large", f"{where}{info.filename}: larger than the inspection limit, not inspected")
            continue
        parts[info.filename] = content
    font_keys = font_table(parts)
    for name, content in sorted(parts.items()):
        lower = name.lower()
        label = where + name
        if lower.endswith((".xml", ".rels", ".vml")) or content[:5] == b"<?xml":
            inspect_xml(name, content, report, label, contexts)
        elif lower.startswith("_xmlsignatures/") and lower.endswith(".sigs"):
            continue
        elif name in font_keys:
            inspect_font(content, font_keys[name], report, label)
        elif "printersettings" in lower:
            report.add("printer settings", f"{label}: printer settings")
        elif lower.endswith(IMAGE_EXTENSIONS) or image_kind(content):
            inspect_image(content, report, label, contexts)
        else:
            inspect_binary(content, report, label, depth, contexts)
    for name, content in parts.items():
        if name.lower().startswith("_xmlsignatures/") and name.lower().endswith(".xml"):
            inspect_signature(content, report, where + name)
        elif name.lower() == "word/people.xml":
            for kind, count in sorted(presence(content).items()):
                report.add("presence", f"{where}{name}: {count} people with {kind} (not shown)")
    named = people(parts)
    if named:
        report.add("people", f"{where or 'the document'}: {len(named)} people named in the properties, comments or tracked changes (not shown)")


# Text.

def xml_text(content):
    text = content.decode("utf-8", "replace")
    if text.startswith("﻿"):
        text = text[1:]
    return html.unescape(text)


# The elements of a paragraph's text runs (text, deleted text and field instructions), in WordprocessingML and DrawingML alike.
RUN_TEXT = {"t", "delText", "instrText", "delInstrText"}


def paragraph_texts(raw):
    """Each paragraph's text with its runs joined; a paragraph inside another (in a text box) is a paragraph of its own.

    Each tag is read from its `<` to the next `>` and never again, so the time it takes grows with the part's size, however the part is crafted (an unterminated tag ends the reading)."""
    open_paragraphs, texts, run_start, position = [], [], None, 0
    while True:
        start = raw.find("<", position)
        end = raw.find(">", start + 1) if start >= 0 else -1
        if end < 0:
            break
        position = end + 1
        tag = raw[start + 1:end]
        closing = tag.startswith("/")
        name = tag[1:] if closing else tag
        name = name.split(None, 1)[0].rstrip("/").rpartition(":")[2] if name.strip() else ""
        self_closing = tag.endswith("/")
        if name == "p":
            if closing:
                if open_paragraphs:
                    texts.append("".join(open_paragraphs.pop()))
            elif not self_closing:
                open_paragraphs.append([])
        elif name in RUN_TEXT:
            if closing:
                if run_start is not None and open_paragraphs:
                    open_paragraphs[-1].append(raw[run_start:start])
                run_start = None
            elif not self_closing:
                run_start = position
    texts.extend("".join(pieces) for pieces in open_paragraphs)
    return [html.unescape(text) for text in texts if text]


def text_findings(text):
    """What a curator must judge in a text, as (flag, the matched text, what to report)."""
    for kind, pattern in MARKERS:
        for match in re.finditer(pattern, text):
            yield kind, match, f"{kind} marker {printable(match.group(0), 40)!r}"
    for match in EMAIL.finditer(text):
        address = match.group(0)
        domain = address.rsplit("@", 1)[1].lower().rstrip(".")
        if EXAMPLE_DOMAIN.search(domain) or address.split("@")[0].lower() in PLACEHOLDER_LOCAL:
            continue
        kind = "personal mail" if domain in PERSONAL_MAIL else "mail address"
        yield kind, match, mask(address.split("@")[0]) + "@" + (domain if kind == "mail address" and "." in domain else mask(domain))
    for match in IBAN.finditer(text):
        if iban_valid(match.group(0)):
            yield "bank account", match, f"an IBAN {mask(match.group(0))}"
    for match in CARD.finditer(text):
        digits = re.sub(r"\D", "", match.group(0))
        if 13 <= len(digits) <= 19 and luhn(digits) and len(set(digits)) > 3 and card_prefix(digits):
            yield "card number", match, f"a number that passes the card checksum, {mask(match.group(0))}"
    for match in USER_FOLDER.finditer(text):
        user = match.group(1)
        if user.lower() in {"public", "default", "all users", "shared", "administrator", "user", "username", "%username%"}:
            continue
        yield "user folder", match, f"a path with a user folder {mask(user)!r}"


def report_findings(findings, text, report, label, contexts):
    """Reports each kind of marker once, with its first sample and its count, then every other finding on its own."""
    markers = {}
    for kind, match, detail in findings:
        if kind in MARKER_KINDS:
            markers.setdefault(kind, []).append((match, detail))
    for kind, found in markers.items():
        sample, detail = found[0]
        line = f"{label}: {detail} ({len(found)}x)"
        if contexts:
            start, end = max(0, sample.start() - CONTEXT), sample.end() + CONTEXT
            line += " … " + printable(re.sub(r"<[^>]*>", " ", text[start:end]), 200)
        report.add(kind, line)
    for kind, _, detail in findings:
        if kind not in MARKER_KINDS:
            report.add(kind, f"{label}: {detail}")


def inspect_xml(name, content, report, label, contexts):
    text = xml_text(content)
    stored_findings = list(text_findings(text))
    report_findings(stored_findings, text, report, label, contexts)
    # Word splits a word across runs wherever its formatting, revision or proofing marks change ("Confidentia" and "l"), so no run need hold a marker whole. Each paragraph's text is searched with its runs joined too, and a finding that the joined text has more often than the part's own text is reported. Both texts' findings are counted once, so the comparison takes time in proportion to the part.
    joined = "\n".join(paragraph_texts(content.decode("utf-8", "replace")))
    if joined:
        stored = {}
        for _, match, _ in stored_findings:
            stored[match.group(0)] = stored.get(match.group(0), 0) + 1
        seen = {}
        hidden = []
        for kind, match, detail in text_findings(joined):
            value = match.group(0)
            seen[value] = seen.get(value, 0) + 1
            if seen[value] > stored.get(value, 0):
                hidden.append((kind, match, detail))
        report_findings(hidden, joined, report, f"{label} (runs joined)", contexts)
    if name.lower() == "docprops/app.xml":
        for match in TEMPLATE_ID.finditer(text):
            report.add("publisher or template", f"{label}: an Office.com template identifier {match.group(1)}")
    if name.lower() == "word/vbaproject.bin" or "vbaProject" in name:
        report.add("macros", f"{label}: a macro project")


def binary_strings(content):
    """The text that binary content carries: runs of at least six printable characters, in ASCII and in UTF-16."""
    ascii_runs = [run.decode("latin-1") for run in re.findall(rb"[\x20-\x7e]{6,}", content)]
    utf16_runs = [run.decode("utf-16-le") for run in re.findall(rb"(?:[\x20-\x7e\xa0-\xff]\x00){6,}", content)]
    return ("\n".join(ascii_runs + utf16_runs)).encode("utf-8", "replace")


def iban_valid(text):
    text = text.replace(" ", "")
    moved = text[4:] + text[:4]
    digits = "".join(str(int(ch, 36)) for ch in moved)
    return len(text) >= 15 and int(digits) % 97 == 1


def luhn(digits):
    total = 0
    for index, ch in enumerate(reversed(digits)):
        value = int(ch)
        if index % 2:
            value *= 2
            if value > 9:
                value -= 9
        total += value
    return total % 10 == 0


def card_prefix(digits):
    return digits[0] in "3456" and not digits.startswith(("30000", "40000", "50000"))


# Embedded objects.

def inspect_binary(content, report, label, depth, contexts):
    lower = label.lower()
    if lower.endswith("vbaproject.bin") or "/vbaproject" in lower:
        report.add("macros", f"{label}: a macro project")
    if "activex" in lower:
        report.add("active content", f"{label}: an ActiveX control")
    executables = find_executables(content)
    if executables:
        report.add("program", f"{label}: contains {', '.join(executables)}")
    if content[:8] == b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1":
        inspect_ole(content, report, label, depth, contexts)
    elif content[:4] == b"PK\x03\x04":
        if depth >= MAX_DEPTH:
            report.add("embedded", f"{label}: a ZIP package nested too deeply, not inspected")
        else:
            report.add("embedded", f"{label}: a ZIP package (inspected below)")
            inspect_package(content, report, label + "!", depth + 1, contexts)
    elif content[:5] == b"%PDF-":
        report.add("embedded", f"{label}: a PDF document ({len(content)} bytes), not inspected inside")
    elif content.lstrip()[:5] in (b"From:", b"Recei", b"Retur", b"MIME-") or label.lower().endswith((".eml", ".msg")):
        report.add("mail message", f"{label}: an e-mail message")
    elif not executables:
        report.add("embedded", f"{label}: {len(content)} bytes of an unrecognized kind, starting {content[:8].hex()}")
        # Its text is searched all the same: HTML, MHTML and plain text as they are, anything else by its strings.
        inspect_xml("", binary_strings(content) if b"\x00" in content[:512] else content, report, label, contexts)


def find_executables(content):
    found = []
    for match in re.finditer(rb"MZ", content):
        start = match.start()
        if start + 0x40 > len(content):
            continue
        offset = struct.unpack_from("<I", content, start + 0x3C)[0]
        if 0x40 <= offset < 0x10000 and content[start + offset:start + offset + 4] == b"PE\x00\x00":
            found.append("a Windows program (PE)")
            break
    if b"\x7fELF" in content:
        found.append("a Linux program (ELF)")
    for magic, name in [(b"\xcf\xfa\xed\xfe", "a macOS program (Mach-O)"), (b"\xce\xfa\xed\xfe", "a macOS program (Mach-O)")]:
        if magic in content:
            found.append(name)
    if b"This program cannot be run in DOS mode" in content and not any("PE" in item for item in found):
        found.append("a DOS stub of a Windows program")
    return found


class Cfb:
    """A minimal reader for OLE compound files (MS-CFB), enough to name streams and read them."""

    def __init__(self, data):
        self.data = data
        if len(data) < 512:
            raise ValueError("shorter than a header")
        shift = struct.unpack_from("<H", data, 0x1E)[0]
        mini_shift = struct.unpack_from("<H", data, 0x20)[0]
        if shift not in (9, 12) or mini_shift != 6:
            raise ValueError("unknown sector size")
        self.size = 1 << shift
        self.mini_size = 64
        self.cutoff = struct.unpack_from("<I", data, 0x38)[0]
        fat_sectors = []
        difat = list(struct.unpack_from("<109I", data, 0x4C))
        next_difat = struct.unpack_from("<I", data, 0x44)[0]
        seen = set()
        while next_difat < 0xFFFFFFFA and next_difat not in seen and len(seen) < 10000:
            seen.add(next_difat)
            block = self.sector(next_difat)
            entries = struct.unpack_from(f"<{self.size // 4}I", block)
            difat.extend(entries[:-1])
            next_difat = entries[-1]
        fat_sectors = [sector for sector in difat if sector < 0xFFFFFFFA]
        self.fat = []
        for sector in fat_sectors:
            self.fat.extend(struct.unpack_from(f"<{self.size // 4}I", self.sector(sector)))
        directory = self.chain(struct.unpack_from("<I", data, 0x30)[0], self.fat, self.sector)
        self.entries = []
        for offset in range(0, len(directory) - 127, 128):
            entry = directory[offset:offset + 128]
            length = struct.unpack_from("<H", entry, 64)[0]
            kind = entry[66]
            if kind not in (1, 2, 5) or length > 64:
                continue
            name = entry[:max(0, length - 2)].decode("utf-16-le", "replace")
            start = struct.unpack_from("<I", entry, 116)[0]
            size = struct.unpack_from("<Q", entry, 120)[0]
            if self.size == 512:
                size &= 0xFFFFFFFF
            self.entries.append((name, kind, start, size))
        root = next((entry for entry in self.entries if entry[1] == 5), None)
        self.mini_stream = self.chain(root[2], self.fat, self.sector)[: root[3]] if root else b""
        mini_fat_start = struct.unpack_from("<I", data, 0x3C)[0]
        mini_fat = self.chain(mini_fat_start, self.fat, self.sector)
        self.mini_fat = list(struct.unpack_from(f"<{len(mini_fat) // 4}I", mini_fat))

    def sector(self, number):
        start = (number + 1) * self.size
        if start + self.size > len(self.data):
            raise ValueError("a sector outside the file")
        return self.data[start:start + self.size]

    def mini_sector(self, number):
        start = number * self.mini_size
        if start + self.mini_size > len(self.mini_stream):
            raise ValueError("a mini sector outside the mini stream")
        return self.mini_stream[start:start + self.mini_size]

    @staticmethod
    def chain(start, table, read):
        parts, seen, sector = [], set(), start
        while sector < 0xFFFFFFFA:
            if sector in seen or sector >= len(table) or len(seen) > 1_000_000:
                raise ValueError("a broken or circular sector chain")
            seen.add(sector)
            parts.append(read(sector))
            sector = table[sector]
        return b"".join(parts)

    def streams(self):
        for name, kind, start, size in self.entries:
            if kind != 2:
                continue
            if size > MAX_PART:
                yield name, None
                continue
            if size < self.cutoff:
                content = self.chain(start, self.mini_fat, self.mini_sector)
            else:
                content = self.chain(start, self.fat, self.sector)
            yield name, content[:size]


def inspect_ole(content, report, label, depth, contexts):
    try:
        cfb = Cfb(content)
        streams = list(cfb.streams())
    except (ValueError, struct.error) as error:
        report.add("embedded", f"{label}: an OLE file that cannot be read ({error})")
        return
    names = [name for name, _ in streams]
    prog_id = ""
    for name, data in streams:
        if name == "\x01CompObj" and data:
            strings = re.findall(rb"[\x20-\x7e]{3,}", data)
            prog_id = ", ".join(s.decode() for s in strings[:3])
    kind = "an OLE object"
    if any(name.startswith("__substg1.0_") for name in names):
        report.add("mail message", f"{label}: an Outlook e-mail message")
        kind = "an Outlook message"
    elif "WordDocument" in names:
        kind = "a Word 97-2003 document"
    elif "Workbook" in names or "Book" in names:
        kind = "an Excel 97-2003 workbook"
    elif "PowerPoint Document" in names:
        kind = "a PowerPoint 97-2003 presentation"
    elif "EquationNative" in names:
        kind = "an Equation Editor 3 object"
    elif "\x01Ole10Native" in names:
        kind = "an OLE package"
    elif "EncryptedPackage" in names:
        kind = "an encrypted Office document"
    report.add("embedded", f"{label}: {kind} ({prog_id or 'no ProgID'})")
    for name, data in streams:
        if data is None:
            report.add("too large", f"{label}/{printable(name)}: a stream larger than the inspection limit")
            continue
        if name == "\x01Ole10Native":
            files = [f.decode("latin-1") for f in re.findall(rb"[\x20-\x7e]{1,260}\.[A-Za-z0-9]{1,5}(?=\x00)", data[:4096])]
            names_shown = sorted({pathlib.PureWindowsPath(f).suffix.lower() or "(none)" for f in files})
            if any(EXECUTABLE_NAMES.search(f) for f in files):
                report.add("program", f"{label}: an OLE package of a file of type {', '.join(names_shown)}")
            elif files:
                report.add("embedded", f"{label}: an OLE package of a file of type {', '.join(names_shown)}")
            packaged = package_data(data)
            if packaged is not None:
                inspect_packaged(packaged, report, f"{label}/package", depth, contexts)
        executables = find_executables(data)
        if executables:
            report.add("program", f"{label}/{printable(name)}: contains {', '.join(executables)}")
        if data[:5] == b"%PDF-":
            # Acrobat's OLE objects keep the PDF itself in a stream; its text is read with a PDF tool, by the curator.
            report.add("embedded", f"{label}/{printable(name)}: a PDF document ({len(data)} bytes), not inspected inside")
        elif data[:4] == b"PK\x03\x04":
            if depth >= MAX_DEPTH:
                report.add("embedded", f"{label}/{printable(name)}: a ZIP package nested too deeply, not inspected")
            else:
                inspect_package(data, report, f"{label}/{printable(name)}!", depth + 1, contexts)
        elif name.startswith("__substg1.0_") and name.endswith("001F"):
            inspect_xml("", data.decode("utf-16-le", "replace").encode("utf-8"), report, f"{label}/{printable(name)}", contexts)
        elif name in ("WordDocument", "1Table", "0Table", "Workbook", "Book", "PowerPoint Document", "CONTENTS", "Contents") or name.startswith("__substg1.0_"):
            inspect_xml("", binary_strings(data), report, f"{label}/{printable(name)}", contexts)


def package_data(data):
    """The file inside an OLE package's native data (MS-OLEDS 2.3.6 and the Packager format): label, path, temporary path, then the file."""
    try:
        position = 6
        for _ in range(2):
            position = data.index(b"\x00", position) + 1
        position += 4
        temporary = struct.unpack_from("<I", data, position)[0]
        position += 4 + temporary
        size = struct.unpack_from("<I", data, position)[0]
        position += 4
        if size > len(data) - position:
            return None
        return data[position:position + size]
    except (ValueError, struct.error):
        return None


def inspect_packaged(content, report, label, depth, contexts):
    if image_kind(content):
        inspect_image(content, report, label, contexts)
    elif content[:5] == b"%PDF-" or content[:4] == b"PK\x03\x04" or content[:8] == b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1":
        inspect_binary(content, report, label, depth + 1, contexts)
    else:
        inspect_xml("", binary_strings(content) if b"\x00" in content[:512] else content, report, label, contexts)


# Pictures.

def image_kind(content):
    if content[:3] == b"\xff\xd8\xff":
        return "JPEG"
    if content[:8] == b"\x89PNG\r\n\x1a\n":
        return "PNG"
    if content[:4] in (b"II*\x00", b"MM\x00*"):
        return "TIFF"
    if content[:6] in (b"GIF87a", b"GIF89a"):
        return "GIF"
    if content[:2] == b"BM":
        return "BMP"
    if content[:4] == b"\xd7\xcd\xc6\x9a" or content[:6] in (b"\x01\x00\x09\x00\x00\x03", b"\x02\x00\x09\x00\x00\x03"):
        return "WMF"
    if content[:4] == b"\x01\x00\x00\x00" and content[40:44] == b" EMF":
        return "EMF"
    return None


def inspect_image(content, report, label, contexts):
    kind = image_kind(content)
    if kind is None and (label.lower().endswith(".svg") or content.lstrip()[:4] == b"<svg"):
        # An SVG picture is XML: its text and metadata are searched like a part's. Office's icon library names its icons in the root element.
        inspect_xml("", content, report, label, contexts)
        icon = re.search(rb'<svg\b[^>]{0,2000}?\bid="Icons_([A-Za-z0-9_]{1,60})"', content[:4096])
        if icon:
            report.add("office icon", f"{label}: the icon {icon.group(1).decode()!r} of Office's icon library")
        return
    if "thumbnail" in label.lower():
        report.add("thumbnail", f"{label}: the document's thumbnail")
    if kind in ("WMF", "EMF"):
        # Vector pictures carry their text as strings; a thumbnail carries the first page's.
        inspect_xml("", binary_strings(content), report, label, contexts)
        return
    if "thumbnail" in label.lower():
        return
    if kind == "JPEG":
        inspect_jpeg(content, report, label)
    elif kind == "PNG":
        inspect_png(content, report, label)
    elif kind == "TIFF":
        inspect_tiff(content, 0, report, label, "TIFF")
    xmp = re.search(rb"<x:xmpmeta.{0,1048576}?</x:xmpmeta>", content, re.S)
    if xmp:
        packet = xmp.group(0).decode("utf-8", "replace")
        fields = sorted(set(re.findall(r"(dc:rights|dc:creator|photoshop:Credit|photoshop:Source|xmpRights:\w+|Iptc4xmpCore:\w+|plus:\w+)", packet)))
        libraries = sorted(set(re.findall(r"Getty|Shutterstock|iStock|Corbis|Alamy|Adobe Stock|Dreamstime|123RF|Depositphotos", packet)))
        if fields or libraries:
            report.add("picture metadata", f"{label}: XMP fields {', '.join(fields)}{'; picture library ' + ', '.join(libraries) if libraries else ''}")


def inspect_jpeg(content, report, label):
    index, width, height = 2, None, None
    while index + 4 <= len(content):
        if content[index] != 0xFF:
            break
        marker = content[index + 1]
        if marker in (0xD8, 0x01) or 0xD0 <= marker <= 0xD7:
            index += 2
            continue
        length = struct.unpack_from(">H", content, index + 2)[0]
        segment = content[index + 4:index + 2 + length]
        if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF) and len(segment) >= 5:
            height, width = struct.unpack_from(">HH", segment, 1)
        elif marker == 0xE1 and segment[:6] == b"Exif\x00\x00":
            inspect_tiff(segment[6:], 0, report, label, "JPEG")
        elif marker == 0xED and b"8BIM" in segment:
            if b"\x1c\x02\x74" in segment or b"\x1c\x02\x50" in segment or b"\x1c\x02\x6e" in segment:
                report.add("picture metadata", f"{label}: IPTC copyright, by-line or credit fields")
        if marker == 0xDA:
            break
        index += 2 + length
    if width and height:
        report.add("photo" if width * height >= 300 * 300 else "small picture", f"{label}: a JPEG picture of {width}x{height} pixels")


TIFF_TAGS = {0x010F: "camera make", 0x0110: "camera model", 0x013B: "artist", 0x8298: "copyright", 0x8825: "GPS position", 0x9C9D: "author (Windows)"}


def inspect_tiff(data, depth, report, label, container):
    if len(data) < 8 or data[:2] not in (b"II", b"MM"):
        return
    order = "<" if data[:2] == b"II" else ">"
    try:
        offset = struct.unpack_from(order + "I", data, 4)[0]
        count = struct.unpack_from(order + "H", data, offset)[0]
    except struct.error:
        return
    found = []
    for number in range(min(count, 512)):
        try:
            tag, kind, items, value = struct.unpack_from(order + "HHII", data, offset + 2 + 12 * number)
        except struct.error:
            break
        if tag not in TIFF_TAGS:
            continue
        name = TIFF_TAGS[tag]
        if tag == 0x8825:
            found.append(name)
            continue
        text = ""
        if kind == 2:
            start = offset + 2 + 12 * number + 8 if items <= 4 else value
            text = data[start:start + min(items, 200)].split(b"\x00")[0].decode("latin-1", "replace").strip()
        if not text:
            continue
        shown = text if tag in (0x010F, 0x0110) else mask(text)
        found.append(f"{name} {printable(shown, 60)!r}")
    if found:
        flag = "picture metadata"
        if any(item == "GPS position" for item in found):
            flag = "GPS position"
        report.add(flag, f"{label}: {container} EXIF {', '.join(found)}")


def inspect_png(content, report, label):
    index = 8
    width = height = None
    keys = []
    while index + 12 <= len(content):
        length = struct.unpack_from(">I", content, index)[0]
        kind = content[index + 4:index + 8]
        body = content[index + 8:index + 8 + length]
        if kind == b"IHDR" and len(body) >= 8:
            width, height = struct.unpack_from(">II", body, 0)
        elif kind in (b"tEXt", b"iTXt", b"zTXt"):
            keys.append(body.split(b"\x00")[0].decode("latin-1", "replace"))
        elif kind == b"eXIf":
            inspect_tiff(body, 0, report, label, "PNG")
        if kind == b"IEND" or length > len(content):
            break
        index += 12 + length
    interesting = sorted({key for key in keys if key.lower() in {"author", "copyright", "source", "artist", "credit", "disclaimer"}})
    if interesting:
        report.add("picture metadata", f"{label}: PNG text fields {', '.join(interesting)}")
    if width and height and width * height >= 600 * 600:
        report.add("large picture", f"{label}: a PNG picture of {width}x{height} pixels")


# Fonts.

def font_table(parts):
    """The embedded font parts, by part name: the font's name, its style, its key and whether Word marked it subsetted."""
    table = parts.get("word/fontTable.xml")
    rels = parts.get("word/_rels/fontTable.xml.rels")
    if not table or not rels:
        return {}
    targets = {}
    for match in re.finditer(r"<Relationship\b([^>]{0,8192})>", rels.decode("utf-8", "replace")):
        attributes = dict(re.findall(r'(\w+)="([^"]*)"', match.group(1)))
        if "Id" in attributes and "Target" in attributes:
            target = attributes["Target"].lstrip("/")
            targets[attributes["Id"]] = target if target.startswith("word/") else "word/" + target
    found = {}
    text = table.decode("utf-8", "replace")
    for font in re.finditer(r"<w:font\b[^>]{0,8192}?w:name=\"([^\"]{0,1024})\"(.{0,65536}?)</w:font>", text, re.S):
        for embed in re.finditer(r"<w:embed(Regular|Bold|Italic|BoldItalic)\b([^>]{0,8192})>", font.group(2)):
            attributes = dict(re.findall(r'([\w:]+)="([^"]*)"', embed.group(2)))
            target = targets.get(attributes.get("r:id", ""))
            if target:
                found[target] = (html.unescape(font.group(1)), embed.group(1), attributes.get("w:fontKey", ""), attributes.get("w:subsetted", ""))
    return found


def deobfuscate(content, key):
    hexits = re.sub(r"[^0-9A-Fa-f]", "", key)
    if len(hexits) != 32:
        return content
    reversed_key = bytes.fromhex(hexits)[::-1]
    head = bytes(byte ^ reversed_key[index % 16] for index, byte in enumerate(content[:32]))
    return head + content[32:]


def sfnt_tables(data):
    if len(data) < 12:
        return {}
    count = struct.unpack_from(">H", data, 4)[0]
    tables = {}
    for number in range(min(count, 64)):
        try:
            tag, _, offset, length = struct.unpack_from(">4sIII", data, 12 + 16 * number)
        except struct.error:
            break
        tables[tag.decode("latin-1")] = data[offset:offset + length]
    return tables


def font_names(name_table):
    names = {}
    if len(name_table) < 6:
        return names
    count, storage = struct.unpack_from(">HH", name_table, 2)
    for number in range(min(count, 512)):
        try:
            platform, encoding, language, name_id, length, offset = struct.unpack_from(">HHHHHH", name_table, 6 + 12 * number)
        except struct.error:
            break
        raw = name_table[storage + offset:storage + offset + length]
        if platform == 3:
            value = raw.decode("utf-16-be", "replace")
        elif platform == 1:
            value = raw.decode("latin-1", "replace")
        else:
            continue
        names.setdefault(name_id, value)
    return names


def inspect_font(content, info, report, label):
    font, style, key, subsetted = info
    data = deobfuscate(content, key)
    tables = sfnt_tables(data)
    glyphs = used = None
    if "maxp" in tables and "head" in tables and "loca" in tables and len(tables["maxp"]) >= 6:
        glyphs = struct.unpack_from(">H", tables["maxp"], 4)[0]
        long_offsets = struct.unpack_from(">h", tables["head"], 50)[0] == 1
        loca = tables["loca"]
        size, unit = (4, ">I") if long_offsets else (2, ">H")
        offsets = [struct.unpack_from(unit, loca, i * size)[0] for i in range(min(glyphs + 1, len(loca) // size))]
        used = sum(1 for a, b in zip(offsets, offsets[1:]) if b > a)
    names = font_names(tables.get("name", b""))
    vendor = tables.get("OS/2", b"")[58:62].decode("latin-1", "replace").strip() if len(tables.get("OS/2", b"")) >= 62 else "?"
    fs_type = struct.unpack_from(">H", tables["OS/2"], 8)[0] if len(tables.get("OS/2", b"")) >= 10 else None
    copyright_holder = printable(names.get(0, ""), 80)
    if glyphs is None:
        shape = "an embedded font that cannot be measured (CFF outlines or unreadable)"
        flag = "font"
    elif used is not None and glyphs and used / glyphs > 0.5:
        shape = f"a complete embedded font: {used} of {glyphs} glyphs have outlines"
        flag = "complete font"
    else:
        shape = f"a font subset: {used} of {glyphs} glyphs have outlines"
        flag = "font subset"
    report.add(flag, f"{label}: {shape}; {printable(font, 40)!r} {style}, vendor {vendor!r}, embedding permission {fs_type}, w:subsetted={subsetted or 'absent'}, copyright {copyright_holder!r}")


# Signatures.

OIDS = {
    "2.5.4.3": "CN", "2.5.4.4": "surname", "2.5.4.5": "serialNumber", "2.5.4.6": "C", "2.5.4.7": "L", "2.5.4.8": "ST",
    "2.5.4.10": "O", "2.5.4.11": "OU", "2.5.4.12": "title", "2.5.4.42": "givenName", "1.2.840.113549.1.9.1": "emailAddress",
    "0.9.2342.19200300.100.1.1": "UID",
}


def der(data, index):
    tag = data[index]
    length = data[index + 1]
    index += 2
    if length & 0x80:
        count = length & 0x7F
        if count == 0 or count > 4:
            raise ValueError("unsupported length")
        length = int.from_bytes(data[index:index + count], "big")
        index += count
    if index + length > len(data):
        raise ValueError("length beyond the data")
    return tag, index, index + length


def children(data, start, end):
    index = start
    while index < end:
        tag, body, after = der(data, index)
        yield tag, body, after
        index = after


def oid(raw):
    first = raw[0]
    parts = [first // 40, first % 40]
    value = 0
    for byte in raw[1:]:
        value = (value << 7) | (byte & 0x7F)
        if not byte & 0x80:
            parts.append(value)
            value = 0
    return ".".join(map(str, parts))


def name_fields(data, start, end):
    fields = []
    for _, set_start, set_end in children(data, start, end):
        for _, seq_start, seq_end in children(data, set_start, set_end):
            items = list(children(data, seq_start, seq_end))
            if len(items) != 2:
                continue
            identifier = oid(data[items[0][1]:items[0][2]])
            tag, value_start, value_end = items[1]
            raw = data[value_start:value_end]
            value = raw.decode("utf-16-be", "replace") if tag == 0x1E else raw.decode("utf-8", "replace")
            fields.append((OIDS.get(identifier, identifier), value))
    return fields


def shown_field(key, value):
    if key in ("C", "O", "OU") or re.search(r"\b(?:Test|CA|Root|Certificate Authority|Citizen)\b", value):
        return f"{key}={printable(value, 60)}"
    if key == "serialNumber":
        return f"serialNumber=({len(value)} characters)"
    return f"{key}={mask(printable(value, 60))}"


def inspect_signature(content, report, label):
    text = content.decode("utf-8", "replace")
    certificates = re.findall(r"<(?:\w+:)?X509Certificate>([^<]+)</", text)
    if not certificates:
        report.add("signature", f"{label}: a signature without a certificate")
    for encoded in certificates:
        try:
            data = base64.b64decode(re.sub(r"\s", "", encoded))
            _, cert_start, cert_end = der(data, 0)
            _, tbs_start, tbs_end = next(children(data, cert_start, cert_end))
            items = list(children(data, tbs_start, tbs_end))
            if items and items[0][0] == 0xA0:
                items = items[1:]
            issuer, validity, subject = items[2], items[3], items[4]
            subject_fields = name_fields(data, subject[1], subject[2])
            issuer_fields = name_fields(data, issuer[1], issuer[2])
            dates = [data[s:e].decode("ascii", "replace") for _, s, e in children(data, validity[1], validity[2])]
        except (ValueError, IndexError, StopIteration, struct.error) as error:
            report.add("signature", f"{label}: a certificate that cannot be read ({error})")
            continue
        person = any(key in ("givenName", "surname", "serialNumber", "emailAddress", "UID") for key, _ in subject_fields) or any(
            key == "CN" and re.match(r"^[^,]+, [^,]+", value) for key, value in subject_fields
        )
        report.add(
            "signature naming a person" if person else "signature",
            f"{label}: certificate for {', '.join(shown_field(k, v) for k, v in subject_fields)}; issued by {', '.join(shown_field(k, v) for k, v in issuer_fields)}; valid until {dates[-1] if dates else '?'}",
        )


# People in the metadata: counted, never shown.

PRESENCE_PROVIDERS = {"Windows Live": "a Microsoft account ID", "AD": "a Windows domain account ID", "None": "no account ID (provider None, the user ID is the author's name)"}


def people(parts):
    """The distinct names of the people that the properties, comments, tracked changes and people part name."""
    names = set()
    for name, content in parts.items():
        lower = name.lower()
        if lower == "docprops/core.xml" or (lower.startswith("word/") and lower.endswith(".xml")):
            text = content.decode("utf-8", "replace")
            names.update(re.findall(r"<(?:dc:creator|cp:lastModifiedBy)>([^<]+)<", text))
            names.update(re.findall(r'\b(?:\w+:)?author="([^"]+)"', text))
    return {html.unescape(name).strip() for name in names} - {""}


def presence(content):
    """How many people of word/people.xml carry presence information, by the kind of account it identifies ([MS-DOCX] w15:presenceInfo)."""
    kinds = {}
    for match in re.finditer(r"<(?:\w+:)?presenceInfo\b([^>]{0,8192})>", content.decode("utf-8", "replace")):
        found = re.search(r'\b(?:\w+:)?providerId="([^"]*)"', match.group(1))
        provider = html.unescape(found.group(1)) if found else ""
        kind = PRESENCE_PROVIDERS.get(provider, f"an ID from the provider {printable(provider, 40)!r}")
        kinds[kind] = kinds.get(kind, 0) + 1
    return kinds


def main():
    arguments = [argument for argument in sys.argv[1:] if not argument.startswith("--")]
    contexts = "--contexts" in sys.argv
    if len(arguments) != 2:
        sys.exit(__doc__)
    store, manifest_path = map(pathlib.Path, arguments)
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    totals = {}
    documents = 0
    for document in manifest["documents"]:
        if document.get("tier") == "T2":
            sys.exit("the manifest lists private (T2) documents; this script is for the public corpus only")
        sha256 = document["sha256"]
        path = store / "objects" / sha256[:2] / (sha256[2:] + ".docx")
        report = Report()
        try:
            data = path.read_bytes() if path.stat().st_size <= MAX_OBJECT else None
        except OSError as error:
            report.add("missing", f"cannot read the object ({error.strerror})")
            data = None
        if data is not None:
            inspect_package(data, report, "", 0, contexts)
        documents += 1
        for flag in report.flags:
            totals[flag] = totals.get(flag, 0) + 1
        # Findings the review accepted as a kind (see the README) do not print a document by themselves; they are counted below. Every embedded object is listed.
        quiet = {"small picture", "thumbnail", "printer settings", "people", "presence", "office icon"}
        if report.flags - quiet:
            places = ", ".join(f"{p['source']}:{p.get('path') or '(no path)'}" for p in document["provenance"][:2])
            print(f"== {sha256[:12]} {printable(places, 300)}")
            for line in report.lines:
                print(printable(line, 600))
    print(f"\n{documents} documents inspected. Documents with each finding:")
    for flag, count in sorted(totals.items(), key=lambda item: (-item[1], item[0])):
        print(f"  {count:5} {flag}")


if __name__ == "__main__":
    main()
