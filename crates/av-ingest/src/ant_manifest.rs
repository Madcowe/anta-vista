//! Autonomi file manifests (`.ant`), ant-client ADR-0006 — the display-only
//! half.
//!
//! A manifest is a self-describing list of what a published site or bundle
//! contains: magic bytes, a version, and a named msgpack map whose `entries`
//! array names each child (`path`, `size`, and whether its content travels
//! inside the manifest as a `DataMap` or sits at a public `address`). The
//! manifest itself is never stored on the network — the bytes travel *in the
//! link* — and neither are the embedded children, so nothing here can (or
//! should) be used to reconstruct content.
//!
//! What this module does is the cheap part the ingest pipeline needs: walk a
//! well-formed manifest and report what it lists, so the description a search
//! shows and the `metadata_json` a row stores can name the entries instead of
//! saying "an autonomi file manifest". It is bounded everywhere — size, entry
//! count, nesting — refuses anything it cannot fully account for, and never
//! interprets a path beyond the rules that make it safe to *print*.
//!
//! This is deliberately not the upstream decoder. It merges ADR-0006's
//! structural pass and typed decode into one walk, keyed on the same limits
//! and the same portable path rules (ported from ant-client's `path.rs`), so
//! a manifest this accepts is one the reference client accepts and vice versa
//! — with two scope carve-outs, both because we never extract:
//!
//! - The effective-name collision rules (folded duplicates, file-versus-
//!   directory prefixes) are not checked: they exist to make extraction
//!   unambiguous, they compare unnamed entries by content address we would
//!   have to derive, and listing two paths is harmless where writing them is
//!   not.
//! - An unknown `ContentRef` variant *is* a hard error here, matching ADR-0006's
//!   forward-compatibility rule, even though listing its path would be safe:
//!   a client must never quietly present a file it could not resolve as though
//!   the manifest were fully read.
//!
//! The golden fixture in the tests is upstream's committed `GOLDEN_V1_HEX`
//! byte for byte, so a change to the encoding fails here too.

/// The `.ant` header: `0xC1`, `ANT`, then a version byte (currently `0x01`).
///
/// Sniffing keys on these first four bytes only — a positive marker that
/// survives truncation, like a JPEG's three — while [`summarize`] demands the
/// full five and the version it knows. That split is deliberate: a manifest
/// written by a future version is still a manifest (so ingest should label it
/// as one), but this decoder has no business pretending to read it.
pub const MAGIC: &[u8] = b"\xC1ANT";

/// ADR-0006 `MAX_MANIFEST_BYTES`: rejected before anything is parsed, which
/// is what bounds every later allocation.
const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;
/// ADR-0006 `MAX_MANIFEST_ENTRIES`, checked from the array header alone
/// before a single entry is materialised.
const MAX_MANIFEST_ENTRIES: usize = 100_000;
/// The format version this decoder reads. A future version is a hard error.
pub(crate) const FORMAT_VERSION: u8 = 1;
/// Nesting bound, matching upstream's `MAX_DECODE_DEPTH`.
const MAX_DEPTH: usize = 16;
/// Longest whole path in bytes (ADR-0006).
const MAX_PATH_BYTES: usize = 1024;
/// Longest single component in bytes (ADR-0006).
const MAX_PATH_COMPONENT_BYTES: usize = 255;
/// One chunk record of an embedded DataMap: post-encryption hash,
/// pre-encryption hash, plaintext size (ADR-0006).
const CHUNK_RECORD_BYTES: usize = 32 + 32 + 4;
/// Length of a public DataMap address.
const ADDRESS_BYTES: usize = 32;

const NIL: u8 = 0xc0;

/// What a manifest summary says. Entries are kept whole (the input bound is
/// what caps them), so `entry_count` is always `entries.len()` — the true
/// total, not a stored prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestSummary {
    /// The suggested root directory name, when the manifest carries one.
    pub name: Option<String>,
    /// Every entry, in the order the manifest lists them.
    pub entries: Vec<EntrySummary>,
}

impl ManifestSummary {
    /// The true number of entries in the manifest.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

/// One listed file: what it is called, what the creator claims it weighs,
/// and which kind of reference holds its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntrySummary {
    /// The relative path, or `None` for an entry that extracts under its
    /// content address.
    pub path: Option<String>,
    /// The creator's claimed plaintext size — a hint for display only.
    pub size: Option<u64>,
    /// `true` for an `Embedded` DataMap, `false` for a public address.
    pub embedded: bool,
}

/// Walk `.ant` bytes and report what they list, or `None` for anything that
/// is not a manifest this version understands and fully accounts for.
///
/// `None` is the only failure mode: there is nothing a caller could do with
/// a partial or differently-worded reason, and refusing is the safe direction
/// to be wrong in.
pub fn summarize(bytes: &[u8]) -> Option<ManifestSummary> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    if bytes.len() < MAGIC.len() + 1 || !bytes.starts_with(MAGIC) {
        return None;
    }
    if bytes[MAGIC.len()] != FORMAT_VERSION {
        return None;
    }
    let mut reader = Reader {
        bytes: &bytes[MAGIC.len() + 1..],
        pos: 0,
    };
    let summary = reader.manifest(1)?;
    // The body is exactly one msgpack value; anything after it is not this
    // encoding.
    if reader.pos != reader.bytes.len() {
        return None;
    }
    Some(summary)
}

// ---------------------------------------------------------------------------
// Portable path rules (ADR-0006, ported from ant-client's path.rs)
// ---------------------------------------------------------------------------

/// Characters no component may contain, over and above control characters.
/// `/` is here too: it is the separator in a path and simply not allowed
/// inside a component (or in the manifest `name`, which is one component).
const FORBIDDEN_CHARS: &[char] = &['<', '>', ':', '"', '|', '?', '*', '\\', '/'];
/// Windows reserved device names, compared without regard to case, with
/// any extension and the spaces before it removed. Windows also treats the
/// superscript digits ¹ ² ³ as COM and LPT port numbers.
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "CONIN$",
    "CONOUT$",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];
/// Lowest non-control character.
const FIRST_PRINTABLE: char = '\u{20}';
/// The DEL control character.
const DELETE: char = '\u{7F}';

/// One path component, also the rule set for the manifest `name`. Any
/// violation rejects the whole manifest (ADR-0006).
fn valid_component(component: &str) -> bool {
    if component.is_empty() || component.len() > MAX_PATH_COMPONENT_BYTES {
        return false;
    }
    if component == "." || component == ".." {
        return false;
    }
    if component
        .chars()
        .any(|c| c < FIRST_PRINTABLE || c == DELETE || FORBIDDEN_CHARS.contains(&c))
    {
        return false;
    }
    if component.ends_with(' ') || component.ends_with('.') {
        return false;
    }
    // Windows ignores spaces between a device name and its extension, so
    // `CON .txt` is the console too.
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches(' ');
    !RESERVED_DEVICE_NAMES
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

/// A full relative entry path: bounded, no leading separator, every
/// component valid. (Trailing and doubled separators fail as empty
/// components, exactly as upstream's split does.)
fn valid_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return false;
    }
    if path.starts_with('/') {
        return false;
    }
    path.split('/').all(valid_component)
}

// ---------------------------------------------------------------------------
// One bounded msgpack walk
// ---------------------------------------------------------------------------

/// The kind and length of one msgpack value, its header consumed.
enum Header {
    /// A value whose bytes are fully consumed.
    Scalar,
    Nil,
    /// A string of this many bytes, not yet consumed.
    Str(usize),
    Array(usize),
    Map(usize),
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let slice = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.pos)?;
        self.pos += 1;
        Some(byte)
    }

    fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        let b = self.take(4)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Option<u64> {
        let b = self.take(8)?;
        Some(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// Consume a leading `nil` if that is what sits here.
    fn take_nil(&mut self) -> bool {
        if self.bytes.get(self.pos) == Some(&NIL) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// An unsigned integer in any of msgpack's uint encodings. Negative
    /// ints, floats and everything else are type errors, as in a typed decode.
    fn uint(&mut self) -> Option<u64> {
        let marker = self.u8()?;
        match marker {
            0x00..=0x7f => Some(u64::from(marker)),
            0xcc => Some(u64::from(self.u8()?)),
            0xcd => Some(u64::from(self.u16()?)),
            0xce => Some(u64::from(self.u32()?)),
            0xcf => self.u64(),
            _ => None,
        }
    }

    /// A `bin` value's bytes.
    fn bin(&mut self) -> Option<&'a [u8]> {
        let marker = self.u8()?;
        let len = match marker {
            0xc4 => usize::from(self.u8()?),
            0xc5 => usize::from(self.u16()?),
            0xc6 => self.u32()? as usize,
            _ => return None,
        };
        self.take(len)
    }

    /// The kind and length of one value, its marker (and length prefix)
    /// consumed. Mirrors upstream's `header`, including rejecting the never-
    /// used `0xc1` — which is the manifest magic's first byte, so a body
    /// starting with it is a DataMap file, not a manifest.
    fn header(&mut self) -> Option<Header> {
        let marker = self.u8()?;
        match marker {
            0x00..=0x7f | 0xe0..=0xff => Some(Header::Scalar),
            0x80..=0x8f => Some(Header::Map(usize::from(marker & 0x0f))),
            0x90..=0x9f => Some(Header::Array(usize::from(marker & 0x0f))),
            0xa0..=0xbf => Some(Header::Str(usize::from(marker & 0x1f))),
            NIL => Some(Header::Nil),
            0xc1 => None,
            0xc2 | 0xc3 => Some(Header::Scalar),
            0xc4 => Some(self.skip_len(1)?),
            0xc5 => Some(self.skip_len(2)?),
            0xc6 => Some(self.skip_len(4)?),
            // ext: length, then a one-byte type, then the payload.
            0xc7 => Some(self.skip_ext(1)?),
            0xc8 => Some(self.skip_ext(2)?),
            0xc9 => Some(self.skip_ext(4)?),
            0xca | 0xce | 0xd2 => Some(self.skip_n(4)?),
            0xcb | 0xcf | 0xd3 => Some(self.skip_n(8)?),
            0xcc | 0xd0 => Some(self.skip_n(1)?),
            0xcd | 0xd1 => Some(self.skip_n(2)?),
            0xd4 => Some(self.skip_n(2)?), // type + 1 byte
            0xd5 => Some(self.skip_n(3)?),
            0xd6 => Some(self.skip_n(5)?),
            0xd7 => Some(self.skip_n(9)?),
            0xd8 => Some(self.skip_n(17)?),
            0xd9 => Some(Header::Str(self.read_len(1)?)),
            0xda => Some(Header::Str(self.read_len(2)?)),
            0xdb => Some(Header::Str(self.read_len(4)?)),
            0xdc => Some(Header::Array(self.read_len(2)?)),
            0xdd => Some(Header::Array(self.read_len(4)?)),
            0xde => Some(Header::Map(self.read_len(2)?)),
            0xdf => Some(Header::Map(self.read_len(4)?)),
        }
    }

    /// Read a length prefix of `width` bytes and consume `len` payload bytes.
    fn skip_len(&mut self, width: usize) -> Option<Header> {
        let len = self.read_len(width)?;
        self.take(len)?;
        Some(Header::Scalar)
    }

    /// Read an ext length prefix and consume `1 + len` (type + payload).
    fn skip_ext(&mut self, width: usize) -> Option<Header> {
        let len = self.read_len(width)?;
        self.take(1)?;
        self.take(len)?;
        Some(Header::Scalar)
    }

    /// Consume `n` bytes and report a scalar header.
    fn skip_n(&mut self, n: usize) -> Option<Header> {
        self.take(n)?;
        Some(Header::Scalar)
    }

    /// Read a big-endian length prefix of `width` bytes.
    fn read_len(&mut self, width: usize) -> Option<usize> {
        let len = match width {
            1 => usize::from(self.u8()?),
            2 => usize::from(self.u16()?),
            4 => self.u32()? as usize,
            _ => return None,
        };
        Some(len)
    }

    /// Skip one whole value of whatever sits here, guarding the depth. This
    /// is upstream's structural pass over anything we do not interpret.
    fn skip_value(&mut self, depth: usize) -> Option<()> {
        if depth > MAX_DEPTH {
            return None;
        }
        let header = self.header()?;
        self.skip_body(header, depth)
    }

    fn skip_body(&mut self, header: Header, depth: usize) -> Option<()> {
        match header {
            Header::Scalar | Header::Nil => Some(()),
            Header::Str(len) => {
                self.take(len)?;
                Some(())
            }
            Header::Array(len) => {
                for _ in 0..len {
                    self.skip_value(depth + 1)?;
                }
                Some(())
            }
            Header::Map(len) => {
                for _ in 0..len {
                    self.skip_value(depth + 1)?;
                    self.skip_value(depth + 1)?;
                }
                Some(())
            }
        }
    }

    /// A map key: its text when it is a string, `None` (key already
    /// consumed, its value not) otherwise. A non-string key is a field no
    /// name can match — upstream skips it the same way.
    fn key(&mut self, depth: usize) -> Option<Option<&'a str>> {
        let header = self.header()?;
        match header {
            Header::Str(len) => {
                let bytes = self.take(len)?;
                Some(Some(std::str::from_utf8(bytes).ok()?))
            }
            other => {
                self.skip_body(other, depth)?;
                Some(None)
            }
        }
    }

    /// An optional struct whose contents we do not read (`torrent`): a map
    /// or `nil`, never an array — the positional encoding ADR-0006 forbids.
    fn skip_struct(&mut self, depth: usize) -> Option<()> {
        match self.header()? {
            Header::Nil => Some(()),
            Header::Map(len) => {
                for _ in 0..len {
                    self.key(depth + 1)?;
                    self.skip_value(depth + 1)?;
                }
                Some(())
            }
            _ => None,
        }
    }

    /// The manifest body itself: a map keyed by field names.
    fn manifest(&mut self, depth: usize) -> Option<ManifestSummary> {
        if depth > MAX_DEPTH {
            return None;
        }
        let Header::Map(nkeys) = self.header()? else {
            return None;
        };
        let mut name: Option<Option<String>> = None;
        let mut torrent_seen = false;
        let mut entries: Option<Vec<EntrySummary>> = None;
        for _ in 0..nkeys {
            match self.key(depth + 1)? {
                Some("name") => {
                    if name.is_some() {
                        return None;
                    }
                    name = Some(self.name_value()?);
                }
                Some("torrent") => {
                    if torrent_seen {
                        return None;
                    }
                    self.skip_struct(depth + 1)?;
                    torrent_seen = true;
                }
                Some("entries") => {
                    if entries.is_some() {
                        return None;
                    }
                    entries = Some(self.entries(depth + 1)?);
                }
                Some(_) | None => self.skip_value(depth + 1)?,
            }
        }
        Some(ManifestSummary {
            name: name.flatten(),
            entries: entries?,
        })
    }

    /// `name`: a string validated as one portable component, or `nil`.
    fn name_value(&mut self) -> Option<Option<String>> {
        match self.header()? {
            Header::Nil => Some(None),
            Header::Str(len) => {
                let component = std::str::from_utf8(self.take(len)?).ok()?;
                if !valid_component(component) {
                    return None;
                }
                Some(Some(component.to_owned()))
            }
            _ => None,
        }
    }

    /// The `entries` array: count read from its header and bounded before
    /// any entry exists, then every entry parsed.
    fn entries(&mut self, depth: usize) -> Option<Vec<EntrySummary>> {
        let Header::Array(count) = self.header()? else {
            return None;
        };
        if count > MAX_MANIFEST_ENTRIES {
            return None;
        }
        let mut entries = Vec::new();
        for _ in 0..count {
            entries.push(self.entry(depth)?);
        }
        Some(entries)
    }

    /// One `ManifestEntry` map.
    fn entry(&mut self, depth: usize) -> Option<EntrySummary> {
        if depth > MAX_DEPTH {
            return None;
        }
        let Header::Map(nkeys) = self.header()? else {
            return None;
        };
        let mut path: Option<Option<String>> = None;
        let mut size: Option<Option<u64>> = None;
        let mut embedded: Option<bool> = None;
        for _ in 0..nkeys {
            match self.key(depth + 1)? {
                Some("path") => {
                    if path.is_some() {
                        return None;
                    }
                    path = Some(match self.header()? {
                        Header::Nil => None,
                        Header::Str(len) => {
                            let path = std::str::from_utf8(self.take(len)?).ok()?;
                            if !valid_path(path) {
                                return None;
                            }
                            Some(path.to_owned())
                        }
                        _ => return None,
                    });
                }
                Some("size") => {
                    if size.is_some() {
                        return None;
                    }
                    size = Some(if self.take_nil() {
                        None
                    } else {
                        Some(self.uint()?)
                    });
                }
                Some("source") => {
                    if embedded.is_some() {
                        return None;
                    }
                    embedded = Some(self.content_ref(depth + 1)?);
                }
                Some(_) | None => self.skip_value(depth + 1)?,
            }
        }
        Some(EntrySummary {
            path: path.flatten(),
            size: size.flatten(),
            embedded: embedded?,
        })
    }

    /// An externally tagged `ContentRef`: a one-entry map from variant name
    /// to body. An unknown variant is a hard error (ADR-0006), and the one
    /// field each known body requires must be there.
    fn content_ref(&mut self, depth: usize) -> Option<bool> {
        if depth > MAX_DEPTH {
            return None;
        }
        let Header::Map(nkeys) = self.header()? else {
            return None;
        };
        if nkeys != 1 {
            return None;
        }
        match self.key(depth + 1)? {
            Some("Embedded") => {
                let Header::Map(nfields) = self.header()? else {
                    return None;
                };
                let mut seen = false;
                for _ in 0..nfields {
                    match self.key(depth + 1)? {
                        Some("data_map") => {
                            if seen {
                                return None;
                            }
                            self.data_map(depth + 1)?;
                            seen = true;
                        }
                        Some(_) | None => self.skip_value(depth + 1)?,
                    }
                }
                if !seen {
                    return None;
                }
                Some(true)
            }
            Some("Public") => {
                let Header::Map(nfields) = self.header()? else {
                    return None;
                };
                let mut seen = false;
                for _ in 0..nfields {
                    match self.key(depth + 1)? {
                        Some("address") => {
                            if seen {
                                return None;
                            }
                            if self.bin()?.len() != ADDRESS_BYTES {
                                return None;
                            }
                            seen = true;
                        }
                        Some(_) | None => self.skip_value(depth + 1)?,
                    }
                }
                if !seen {
                    return None;
                }
                Some(false)
            }
            // Unknown variant, or a non-string variant name: hard error.
            Some(_) | None => None,
        }
    }

    /// The embedded DataMap's shape: a map whose `chunks` bin is a non-empty
    /// whole number of 68-byte records. The records themselves are not read —
    /// nothing in a listing depends on them — but a manifest whose DataMap is
    /// malformed is one the reference decoder rejects, and we refuse it too.
    fn data_map(&mut self, depth: usize) -> Option<()> {
        let Header::Map(nfields) = self.header()? else {
            return None;
        };
        let mut seen = false;
        for _ in 0..nfields {
            match self.key(depth + 1)? {
                Some("chunks") => {
                    if seen {
                        return None;
                    }
                    let chunks = self.bin()?;
                    if chunks.is_empty() || chunks.len() % CHUNK_RECORD_BYTES != 0 {
                        return None;
                    }
                    seen = true;
                }
                Some(_) | None => self.skip_value(depth + 1)?,
            }
        }
        if seen { Some(()) } else { None }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Upstream's committed `GOLDEN_V1_HEX`, byte for byte: header;
    /// `{"name": "r", "entries": [`; the unnamed embedded entry, sorted
    /// first by its hex effective name, whose DataMap is one 68-byte chunk
    /// record (dst hash, src hash, big-endian u32 size) in a `bin`; then the
    /// `Public` entry `a` with its address as a 32-byte `bin`.
    const GOLDEN_V1_HEX: &str = concat!(
        "c1414e5401",
        "82a46e616d65a172a7656e747269657392",
        "81a6736f7572636581a8456d62656464656481a8646174615f6d617081a66368756e6b73c444",
        "2222222222222222222222222222222222222222222222222222222222222222",
        "3333333333333333333333333333333333333333333333333333333333333333",
        "00000102",
        "83a470617468a161a473697a6505a6736f7572636581a65075626c696381a761646472657373c420",
        "1111111111111111111111111111111111111111111111111111111111111111",
    );

    fn golden() -> Vec<u8> {
        hex::decode(GOLDEN_V1_HEX).unwrap()
    }

    #[test]
    fn the_committed_upstream_fixture_parses() {
        let summary = summarize(&golden()).unwrap();
        assert_eq!(summary.name.as_deref(), Some("r"));
        assert_eq!(summary.entry_count(), 2);
        assert_eq!(
            summary.entries,
            vec![
                EntrySummary {
                    path: None,
                    size: None,
                    embedded: true,
                },
                EntrySummary {
                    path: Some("a".to_owned()),
                    size: Some(5),
                    embedded: false,
                },
            ]
        );
    }

    #[test]
    fn a_prefix_of_the_magic_sniffs_but_never_parses() {
        // The four-byte magic alone is not the full five-byte header: enough
        // to call bytes a manifest, never enough to read one.
        assert!(summarize(&golden()[..4]).is_none());
        assert!(summarize(&golden()[..5]).is_none(), "version byte alone");
        assert!(summarize(b"").is_none());
        assert!(summarize(b"not a manifest at all").is_none());
    }

    #[test]
    fn an_unknown_format_version_is_a_hard_error() {
        let mut bytes = golden();
        bytes[4] = FORMAT_VERSION + 1;
        assert!(summarize(&bytes).is_none());
    }

    #[test]
    fn trailing_bytes_after_the_body_are_rejected() {
        let mut bytes = golden();
        bytes.push(0xc0);
        assert!(summarize(&bytes).is_none());
    }

    #[test]
    fn a_positional_array_is_never_a_struct() {
        // The body itself as an array, an entry as an array, and a variant
        // body as an array: three places ADR-0006 forbids the positional
        // encoding by name.
        let body: &[&[u8]] = &[
            &[0x92, 0xc0, 0x90],                                    // [nil, []]
            b"\x81\xa7entries\x91\x93\xc0\xc0\x80",                 // {"entries": [[nil, nil, {}]]}
            b"\x81\xa7entries\x91\x81\xa6source\x81\xa6Public\x90", // {"entries":[{"source":{"Public":[]}}]}
        ];
        for b in body {
            let mut bytes = MAGIC.to_vec();
            bytes.push(FORMAT_VERSION);
            bytes.extend_from_slice(b);
            assert!(summarize(&bytes).is_none(), "accepted {:?}", b);
        }
    }

    #[test]
    fn the_entry_count_is_refused_from_the_header_alone() {
        // One map key, one array header claiming u32::MAX entries, no
        // entries: rejected before anything is materialised.
        let mut bytes = MAGIC.to_vec();
        bytes.push(FORMAT_VERSION);
        bytes.extend_from_slice(b"\x81\xa7entries\xdd");
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(summarize(&bytes).is_none());
    }

    #[test]
    fn a_manifest_may_hold_no_entries() {
        let mut bytes = MAGIC.to_vec();
        bytes.push(FORMAT_VERSION);
        bytes.extend_from_slice(b"\x81\xa7entries\x90");
        let summary = summarize(&bytes).unwrap();
        assert_eq!(summary.name, None);
        assert_eq!(summary.entry_count(), 0);
    }

    #[test]
    fn an_unknown_content_ref_variant_is_refused() {
        // A variant added within version 1 is one this reader cannot claim
        // to have read: the ADR makes it a hard decode error, and a listing
        // must not present its path as resolved.
        let mut bytes = MAGIC.to_vec();
        bytes.push(FORMAT_VERSION);
        bytes
            .extend_from_slice(b"\x81\xa7entries\x91\x82\xa4path\xa1x\xa6source\x81\xa6Future\x80");
        assert!(summarize(&bytes).is_none());
    }

    #[test]
    fn unknown_fields_are_ignored_at_every_level() {
        // The forward-compatibility rule, exercised through skip: a future
        // top-level field of any shape, a future entry field, a future key
        // inside the variant body, and a non-string key (which no field name
        // can match) all decode around.
        let mut bytes = MAGIC.to_vec();
        bytes.push(FORMAT_VERSION);
        bytes.extend_from_slice(
            b"\x82\xa6future\x92\x01\x92\x02\x81\xa1x\xce\xff\xff\xff\xff\
              \xa7entries\x91\x83\xa4path\xa1x\xa5extra\x81\xa1a\x91\x01\
              \xa6source\x81\xa6Public\x82\xa7address\xc4\x20",
        );
        bytes.extend_from_slice(&[0x11; 32]);
        bytes.push(0xcc); // non-string key: uint8…
        bytes.push(0x07); // …whose value follows
        bytes.push(0x07);
        let summary = summarize(&bytes).unwrap();
        assert_eq!(summary.entries.len(), 1);
        assert!(!summary.entries[0].embedded);
    }

    #[test]
    fn oversized_input_is_refused_before_parsing() {
        let oversized = vec![0u8; MAX_MANIFEST_BYTES + 1];
        assert!(summarize(&oversized).is_none());
    }

    #[test]
    fn paths_are_judged_against_the_portable_rules() {
        assert!(valid_path("index.html"));
        assert!(valid_path("docs/a b/c.txt"));
        assert!(
            !valid_path("CON/x"),
            "a reserved component is rejected in place"
        );
        assert!(!valid_path("CON"));
        assert!(!valid_path("con.txt"));
        assert!(!valid_path("CON .txt"), "spaces before the extension");
        assert!(!valid_path("COM1.log"));
        assert!(!valid_path("LPT\u{b9}"));
        assert!(!valid_path("/abs"));
        assert!(!valid_path("a//b"));
        assert!(!valid_path("a/"));
        assert!(!valid_path("../escape"));
        assert!(!valid_path("a/./b"));
        assert!(!valid_path("trailing "));
        assert!(!valid_path("trailing."));
        assert!(!valid_path("bad<name>"));
        assert!(!valid_path("back\\slash"));
        assert!(!valid_path(&"x".repeat(256)), "component too long");
        assert!(!valid_path(&"a/".repeat(600)), "path too long");
        // Control characters: below 0x20 and DEL, and no further — a C1
        // control is printable by the ADR's rule.
        assert!(!valid_path("bell\u{7}"));
        assert!(!valid_path("del\u{7f}"));
        assert!(valid_path("c1\u{80}contested"));
        assert!(!valid_path("nul\u{0}byte"));
    }

    #[test]
    fn a_bad_path_or_name_rejects_the_whole_manifest() {
        // One hostile path means the manifest is refused, not summarised
        // without it: a decoder that silently drops files is worse than one
        // that stops.
        let mut bytes = MAGIC.to_vec();
        bytes.push(FORMAT_VERSION);
        bytes.extend_from_slice(b"\x81\xa7entries\x91\x82\xa4path\xa6../esc");
        bytes.extend_from_slice(b"\xa6source\x81\xa6Public\x81\xa7address\xc4\x20");
        bytes.extend_from_slice(&[0x11; 32]);
        assert!(summarize(&bytes).is_none());

        let mut bytes = MAGIC.to_vec();
        bytes.push(FORMAT_VERSION);
        bytes.extend_from_slice(b"\x82\xa4name\xa6/tmp/x\xa7entries\x90");
        assert!(summarize(&bytes).is_none());
    }

    #[test]
    fn a_missing_required_source_is_refused() {
        // `source` is the one required field of an entry; an entry that is
        // only a name and a size resolves nothing.
        let mut bytes = MAGIC.to_vec();
        bytes.push(FORMAT_VERSION);
        bytes.extend_from_slice(b"\x81\xa7entries\x91\x82\xa4path\xa1x\xa4size\x01");
        assert!(summarize(&bytes).is_none());
    }

    #[test]
    fn a_nesting_bomb_stops_at_the_depth_bound() {
        // A chain of fixarray headers, each wrapping the next, deeper than
        // the decode depth — as an unknown top-level field so every level
        // goes through the guarded skip.
        let mut bytes = MAGIC.to_vec();
        bytes.push(FORMAT_VERSION);
        bytes.extend_from_slice(b"\x81\xa1x");
        bytes.extend(std::iter::repeat_n(0x91, MAX_DEPTH * 4));
        bytes.push(0xc0);
        assert!(summarize(&bytes).is_none());
    }
}
