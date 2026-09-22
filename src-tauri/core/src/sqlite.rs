//! A strictly read-only SQLite reader.
//!
//! Cursor keeps `state.vscdb` open in WAL mode while it runs. Pointing a normal
//! SQLite client at that file is risky even in `mode=ro`: the library still
//! wants to create the `-shm` file and may attempt journal recovery, which
//! produces the "database is locked" / "unable to open database file" failures
//! that plague tools in this space.
//!
//! So we parse the file format directly instead. This reader:
//!
//! * opens the database and its `-wal` with a plain read handle and never
//!   writes, locks, or creates a single byte;
//! * overlays committed WAL frames on top of the main file, so numbers are as
//!   fresh as the last commit Cursor made rather than the last checkpoint;
//! * validates WAL salts and the running frame checksums, stopping at the last
//!   good commit exactly as SQLite's own recovery does.
//!
//! It implements the read path we need (table b-tree scans with overflow) and
//! deliberately not the rest of SQLite: no indexes, no writing, no SQL.
//!
//! Format reference: <https://www.sqlite.org/fileformat2.html>

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

/// Refuse pathological pages rather than allocating whatever the header claims.
const MIN_PAGE_SIZE: usize = 512;
const MAX_PAGE_SIZE: usize = 65536;
/// Depth guard for a corrupt b-tree that points at itself.
const MAX_BTREE_DEPTH: usize = 64;

/// A value decoded from a record.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    /// Text of a TEXT value, or a BLOB that happens to hold UTF-8.
    ///
    /// VS Code-family databases declare `ItemTable.value` as BLOB but store
    /// JSON text in it, so the two cases have to be treated alike.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            Value::Blob(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Real(f) => Some(*f as i64),
            _ => None,
        }
    }
}

/// How the database encodes TEXT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

impl Encoding {
    fn decode(self, bytes: &[u8]) -> String {
        match self {
            Encoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
            Encoding::Utf16Le | Encoding::Utf16Be => {
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|c| {
                        if self == Encoding::Utf16Le {
                            u16::from_le_bytes([c[0], c[1]])
                        } else {
                            u16::from_be_bytes([c[0], c[1]])
                        }
                    })
                    .collect();
                String::from_utf16_lossy(&units)
            }
        }
    }
}

/// Reads pages from the main database file with committed WAL frames overlaid.
struct Pager {
    db: File,
    wal: Option<File>,
    /// Page number -> offset of that page's data within the `-wal` file.
    wal_pages: HashMap<u32, u64>,
    page_size: usize,
    /// Bytes at the end of each page reserved by extensions; excluded from payload.
    reserved: usize,
    page_count: u32,
    encoding: Encoding,
}

impl Pager {
    fn usable(&self) -> usize {
        self.page_size - self.reserved
    }

    /// Read one 1-based page, preferring the WAL copy when there is one.
    fn page(&mut self, number: u32) -> Result<Vec<u8>> {
        if number == 0 || number > self.page_count {
            bail!(
                "page {number} out of range (db has {} pages)",
                self.page_count
            );
        }
        let mut buf = vec![0u8; self.page_size];

        if let (Some(offset), Some(wal)) = (self.wal_pages.get(&number).copied(), self.wal.as_mut())
        {
            wal.seek(SeekFrom::Start(offset))?;
            wal.read_exact(&mut buf)
                .with_context(|| format!("reading page {number} from WAL"))?;
            return Ok(buf);
        }

        let offset = (number as u64 - 1) * self.page_size as u64;
        self.db.seek(SeekFrom::Start(offset))?;
        self.db
            .read_exact(&mut buf)
            .with_context(|| format!("reading page {number}"))?;
        Ok(buf)
    }
}

/// A read-only view of a SQLite database file.
pub struct ReadOnlyDb {
    pager: Pager,
}

impl ReadOnlyDb {
    /// Open `path` (and its `-wal`, if present) for reading.
    ///
    /// Never creates, locks, truncates or writes anything, so it is safe to
    /// point at the live database of a running application.
    pub fn open(path: &Path) -> Result<Self> {
        let mut db =
            File::open(path).with_context(|| format!("opening {} read-only", path.display()))?;

        let mut header = [0u8; 100];
        db.read_exact(&mut header)
            .with_context(|| format!("{} is too short to be a database", path.display()))?;
        if &header[..16] != b"SQLite format 3\0" {
            bail!("{} is not a SQLite database", path.display());
        }

        let page_size = match u16::from_be_bytes([header[16], header[17]]) {
            1 => MAX_PAGE_SIZE, // the header stores 65536 as 1
            n => n as usize,
        };
        if !(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&page_size) || !page_size.is_power_of_two() {
            bail!("implausible page size {page_size}");
        }

        let reserved = header[20] as usize;
        // SQLite itself rejects pages with fewer than 480 usable bytes. The
        // overflow maths further down (`usable - 35`, `% (usable - 4)`) would
        // underflow or divide by zero on anything smaller, and with
        // `panic = "abort"` a corrupt file would take the whole app down.
        if reserved >= page_size || page_size - reserved < 480 {
            bail!("reserved region {reserved} swallows the page");
        }

        let encoding = match u32::from_be_bytes([header[56], header[57], header[58], header[59]]) {
            2 => Encoding::Utf16Le,
            3 => Encoding::Utf16Be,
            _ => Encoding::Utf8,
        };

        // The in-header page count is only trustworthy when the change counter
        // matches the version-valid-for marker; otherwise fall back to the file
        // length, which is what older SQLite releases do.
        let change_counter = u32::from_be_bytes(header[24..28].try_into().unwrap());
        let valid_for = u32::from_be_bytes(header[92..96].try_into().unwrap());
        let header_pages = u32::from_be_bytes(header[28..32].try_into().unwrap());
        let file_len = db.metadata()?.len();
        let len_pages = (file_len / page_size as u64) as u32;
        let mut page_count = if change_counter == valid_for && header_pages > 0 {
            header_pages
        } else {
            len_pages
        };

        let (wal, wal_pages, wal_page_count) = load_wal(path, page_size)?;
        // WAL commits can extend the database past the size recorded in the
        // main file's header.
        if let Some(n) = wal_page_count {
            page_count = page_count.max(n);
        }

        Ok(Self {
            pager: Pager {
                db,
                wal,
                wal_pages,
                page_size,
                reserved,
                page_count,
                encoding,
            },
        })
    }

    /// Number of pages visible through this reader, WAL included.
    pub fn page_count(&self) -> u32 {
        self.pager.page_count
    }

    /// True when committed WAL frames are being overlaid on the main file.
    pub fn has_wal_overlay(&self) -> bool {
        !self.pager.wal_pages.is_empty()
    }

    /// Root page of a table, read from `sqlite_schema` on page 1.
    fn root_page_of(&mut self, table: &str) -> Result<u32> {
        let mut root = None;
        // sqlite_schema columns: type, name, tbl_name, rootpage, sql
        self.scan_page(1, 0, &mut |_rowid, cols: &[Value]| {
            let is_table = cols.first().and_then(Value::as_str) == Some("table");
            let matches = cols
                .get(1)
                .and_then(Value::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case(table));
            if is_table && matches {
                root = cols.get(3).and_then(Value::as_i64).map(|n| n as u32);
                return false; // found it, stop walking
            }
            true
        })?;
        root.ok_or_else(|| anyhow!("table `{table}` not found"))
    }

    /// Visit every row of `table`, stopping early if `visit` returns `false`.
    ///
    /// Rows arrive in rowid order. A corrupt or cyclic b-tree aborts with an
    /// error rather than looping forever.
    pub fn scan_table<F>(&mut self, table: &str, mut visit: F) -> Result<()>
    where
        F: FnMut(i64, &[Value]) -> bool,
    {
        let root = self.root_page_of(table)?;
        self.scan_page(root, 0, &mut visit)?;
        Ok(())
    }

    /// Read a `(key, value)` style table into a map, which is the shape every
    /// VS Code-family `state.vscdb` uses.
    ///
    /// `limit` caps how many rows are retained so a huge database can't balloon
    /// memory; scanning still stops as soon as the cap is hit.
    pub fn key_value_table(
        &mut self,
        table: &str,
        limit: usize,
    ) -> Result<HashMap<String, String>> {
        let mut out = HashMap::new();
        self.scan_table(table, |_rowid, cols| {
            if let (Some(k), Some(v)) = (
                cols.first().and_then(Value::as_str),
                cols.get(1).and_then(Value::as_str),
            ) {
                out.insert(k.to_string(), v.to_string());
            }
            out.len() < limit
        })?;
        Ok(out)
    }

    fn scan_page<F>(&mut self, page_no: u32, depth: usize, visit: &mut F) -> Result<bool>
    where
        F: FnMut(i64, &[Value]) -> bool,
    {
        if depth > MAX_BTREE_DEPTH {
            bail!("b-tree deeper than {MAX_BTREE_DEPTH} pages; refusing to recurse");
        }
        let page = self.pager.page(page_no)?;
        // Page 1 carries the 100-byte file header before its b-tree header.
        let base = if page_no == 1 { 100 } else { 0 };

        let page_type = *page
            .get(base)
            .ok_or_else(|| anyhow!("page {page_no} truncated"))?;
        let cell_count = u16::from_be_bytes([page[base + 3], page[base + 4]]) as usize;
        let header_len = match page_type {
            0x0D | 0x0A => 8,  // leaf
            0x05 | 0x02 => 12, // interior
            other => bail!("page {page_no} has unknown b-tree type 0x{other:02x}"),
        };
        let is_leaf = matches!(page_type, 0x0D | 0x0A);
        // Index pages hold keys, not rows; nothing here needs them.
        if matches!(page_type, 0x0A | 0x02) {
            return Ok(true);
        }

        let pointers_at = base + header_len;
        let mut cells = Vec::with_capacity(cell_count);
        for i in 0..cell_count {
            let at = pointers_at + i * 2;
            let ptr = *page
                .get(at)
                .zip(page.get(at + 1))
                .map(|(a, b)| u16::from_be_bytes([*a, *b]))
                .get_or_insert(0) as usize;
            if ptr >= self.pager.usable() {
                bail!("page {page_no} cell pointer {ptr} out of bounds");
            }
            cells.push(ptr);
        }

        if is_leaf {
            for offset in cells {
                let (rowid, payload) = self.read_leaf_cell(&page, offset)?;
                let cols = self.decode_record(&payload)?;
                if !visit(rowid, &cols) {
                    return Ok(false);
                }
            }
            return Ok(true);
        }

        // Interior page: descend into each child, then the rightmost pointer.
        for offset in cells {
            let child = u32::from_be_bytes(
                page.get(offset..offset + 4)
                    .ok_or_else(|| anyhow!("page {page_no} interior cell truncated"))?
                    .try_into()
                    .unwrap(),
            );
            if child == page_no {
                bail!("page {page_no} points at itself");
            }
            if !self.scan_page(child, depth + 1, visit)? {
                return Ok(false);
            }
        }
        let rightmost = u32::from_be_bytes(page[base + 8..base + 12].try_into().unwrap());
        if rightmost != 0 && rightmost != page_no {
            return self.scan_page(rightmost, depth + 1, visit);
        }
        Ok(true)
    }

    /// Read one table-leaf cell, following overflow pages when the payload
    /// doesn't fit on the page.
    fn read_leaf_cell(&mut self, page: &[u8], offset: usize) -> Result<(i64, Vec<u8>)> {
        let (payload_len, n1) = read_varint(page, offset)?;
        let (rowid, n2) = read_varint(page, offset + n1)?;
        let data_at = offset + n1 + n2;
        let payload_len = payload_len as usize;

        let usable = self.pager.usable();
        // Payload-spill thresholds, straight from the file format spec.
        let max_local = usable - 35;
        let min_local = ((usable - 12) * 32 / 255) - 23;

        let local_len = if payload_len <= max_local {
            payload_len
        } else {
            let k = min_local + (payload_len - min_local) % (usable - 4);
            if k <= max_local {
                k
            } else {
                min_local
            }
        };

        let mut payload = page
            .get(data_at..data_at + local_len)
            .ok_or_else(|| anyhow!("cell payload runs past the page"))?
            .to_vec();

        if local_len < payload_len {
            let mut next = u32::from_be_bytes(
                page.get(data_at + local_len..data_at + local_len + 4)
                    .ok_or_else(|| anyhow!("missing overflow pointer"))?
                    .try_into()
                    .unwrap(),
            );
            let mut guard = 0usize;
            while next != 0 && payload.len() < payload_len {
                guard += 1;
                if guard > self.pager.page_count as usize {
                    bail!("overflow chain loops");
                }
                let ov = self.pager.page(next)?;
                next = u32::from_be_bytes(ov[0..4].try_into().unwrap());
                let take = (payload_len - payload.len()).min(usable - 4);
                payload.extend_from_slice(&ov[4..4 + take]);
            }
            if payload.len() != payload_len {
                bail!("overflow chain ended early");
            }
        }

        Ok((rowid, payload))
    }

    /// Decode a record body into column values.
    fn decode_record(&self, payload: &[u8]) -> Result<Vec<Value>> {
        let (header_len, n) = read_varint(payload, 0)?;
        let header_len = header_len as usize;
        if header_len > payload.len() {
            bail!("record header longer than the record");
        }

        let mut serials = Vec::new();
        let mut at = n;
        while at < header_len {
            let (serial, used) = read_varint(payload, at)?;
            serials.push(serial);
            at += used;
        }

        let mut body = header_len;
        let mut out = Vec::with_capacity(serials.len());
        for serial in serials {
            let (value, size) = decode_value(payload, body, serial, self.pager.encoding)?;
            body += size;
            out.push(value);
        }
        Ok(out)
    }
}

/// Decode one column given its serial type. Returns the value and bytes consumed.
fn decode_value(buf: &[u8], at: usize, serial: i64, enc: Encoding) -> Result<(Value, usize)> {
    let need = |n: usize| -> Result<&[u8]> {
        buf.get(at..at + n)
            .ok_or_else(|| anyhow!("record body truncated"))
    };
    Ok(match serial {
        0 => (Value::Null, 0),
        1 => (Value::Int(need(1)?[0] as i8 as i64), 1),
        2 => (
            Value::Int(i16::from_be_bytes(need(2)?.try_into().unwrap()) as i64),
            2,
        ),
        3 => {
            let b = need(3)?;
            // Sign-extend a 24-bit big-endian integer.
            let raw = ((b[0] as i64) << 16) | ((b[1] as i64) << 8) | b[2] as i64;
            let signed = if raw & 0x80_0000 != 0 {
                raw - 0x100_0000
            } else {
                raw
            };
            (Value::Int(signed), 3)
        }
        4 => (
            Value::Int(i32::from_be_bytes(need(4)?.try_into().unwrap()) as i64),
            4,
        ),
        5 => {
            let b = need(6)?;
            let mut raw: i64 = 0;
            for byte in b {
                raw = (raw << 8) | *byte as i64;
            }
            let signed = if raw & 0x8000_0000_0000 != 0 {
                raw - 0x1_0000_0000_0000
            } else {
                raw
            };
            (Value::Int(signed), 6)
        }
        6 => (
            Value::Int(i64::from_be_bytes(need(8)?.try_into().unwrap())),
            8,
        ),
        7 => (
            Value::Real(f64::from_be_bytes(need(8)?.try_into().unwrap())),
            8,
        ),
        8 => (Value::Int(0), 0),
        9 => (Value::Int(1), 0),
        10 | 11 => bail!("serial type {serial} is reserved"),
        n if n % 2 == 0 => {
            let len = ((n - 12) / 2) as usize;
            (Value::Blob(need(len)?.to_vec()), len)
        }
        n => {
            let len = ((n - 13) / 2) as usize;
            (Value::Text(enc.decode(need(len)?)), len)
        }
    })
}

/// Read a SQLite variable-length integer. Returns the value and its byte width.
fn read_varint(buf: &[u8], at: usize) -> Result<(i64, usize)> {
    let mut result: u64 = 0;
    for i in 0..8 {
        let byte = *buf
            .get(at + i)
            .ok_or_else(|| anyhow!("varint runs past the buffer"))?;
        result = (result << 7) | (byte & 0x7F) as u64;
        if byte & 0x80 == 0 {
            return Ok((result as i64, i + 1));
        }
    }
    // A ninth byte contributes all eight of its bits.
    let last = *buf
        .get(at + 8)
        .ok_or_else(|| anyhow!("varint runs past the buffer"))?;
    result = (result << 8) | last as u64;
    Ok((result as i64, 9))
}

/// The committed WAL overlay: the open file, a page -> data-offset map, and the
/// database size recorded by the last commit frame.
type WalOverlay = (Option<File>, HashMap<u32, u64>, Option<u32>);

/// Parse `<path>-wal`, returning the committed page overlay.
///
/// A WAL that is missing, empty, or fails validation simply yields no overlay:
/// the caller then sees the last checkpointed state, which is stale but never
/// wrong.
fn load_wal(db_path: &Path, page_size: usize) -> Result<WalOverlay> {
    let mut wal_path = db_path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let wal_path = std::path::PathBuf::from(wal_path);

    let mut file = match File::open(&wal_path) {
        Ok(f) => f,
        Err(_) => return Ok((None, HashMap::new(), None)),
    };

    let mut header = [0u8; 32];
    if file.read_exact(&mut header).is_err() {
        return Ok((None, HashMap::new(), None)); // freshly created, no frames yet
    }

    let magic = u32::from_be_bytes(header[0..4].try_into().unwrap());
    // SQLite writes the checksum in the host's native byte order and records
    // which one in the magic's low bit: set means big-endian.
    let big_endian_checksums = match magic {
        0x377f_0682 => false,
        0x377f_0683 => true,
        _ => return Ok((None, HashMap::new(), None)),
    };

    let wal_page_size = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
    if wal_page_size != page_size {
        // A WAL written against a different page size belongs to another
        // database generation; ignoring it is the safe read.
        return Ok((None, HashMap::new(), None));
    }

    let salt = &header[16..24];
    let mut running = (
        u32::from_be_bytes(header[24..28].try_into().unwrap()),
        u32::from_be_bytes(header[28..32].try_into().unwrap()),
    );
    // The header's own checksum seeds the frame chain; if it doesn't verify the
    // WAL is from a different era and must not be trusted.
    if checksum(&header[0..24], (0, 0), big_endian_checksums) != running {
        return Ok((None, HashMap::new(), None));
    }

    let frame_size = 24 + page_size as u64;
    let len = file.metadata()?.len();
    let mut frames: Vec<(u32, u64, Option<u32>)> = Vec::new();
    let mut offset = 32u64;
    let mut frame_header = [0u8; 24];
    let mut page_buf = vec![0u8; page_size];

    while offset + frame_size <= len {
        file.seek(SeekFrom::Start(offset))?;
        if file.read_exact(&mut frame_header).is_err() {
            break;
        }
        // Frames from an older WAL generation are left behind after a reset.
        if &frame_header[8..16] != salt {
            break;
        }
        if file.read_exact(&mut page_buf).is_err() {
            break;
        }

        let expected = (
            u32::from_be_bytes(frame_header[16..20].try_into().unwrap()),
            u32::from_be_bytes(frame_header[20..24].try_into().unwrap()),
        );
        let mut sum = checksum(&frame_header[0..8], running, big_endian_checksums);
        sum = checksum(&page_buf, sum, big_endian_checksums);
        if sum != expected {
            break; // torn or partially written frame: stop, like SQLite recovery
        }
        running = sum;

        let page_no = u32::from_be_bytes(frame_header[0..4].try_into().unwrap());
        let db_size = u32::from_be_bytes(frame_header[4..8].try_into().unwrap());
        frames.push((
            page_no,
            offset + 24,
            if db_size > 0 { Some(db_size) } else { None },
        ));
        offset += frame_size;
    }

    // Only frames up to and including the last commit are visible.
    let last_commit = frames.iter().rposition(|(_, _, commit)| commit.is_some());
    let Some(last_commit) = last_commit else {
        return Ok((Some(file), HashMap::new(), None));
    };

    let mut pages = HashMap::new();
    for (page_no, data_at, _) in &frames[..=last_commit] {
        // A later frame for the same page wins.
        pages.insert(*page_no, *data_at);
    }
    let db_size = frames[last_commit].2;

    Ok((Some(file), pages, db_size))
}

/// SQLite's WAL checksum: a pair of accumulators over big-endian u32 pairs.
fn checksum(data: &[u8], seed: (u32, u32), big_endian: bool) -> (u32, u32) {
    let (mut s0, mut s1) = seed;
    for chunk in data.chunks_exact(8) {
        let (a, b) = if big_endian {
            (
                u32::from_be_bytes(chunk[0..4].try_into().unwrap()),
                u32::from_be_bytes(chunk[4..8].try_into().unwrap()),
            )
        } else {
            (
                u32::from_le_bytes(chunk[0..4].try_into().unwrap()),
                u32::from_le_bytes(chunk[4..8].try_into().unwrap()),
            )
        };
        s0 = s0.wrapping_add(a).wrapping_add(s1);
        s1 = s1.wrapping_add(b).wrapping_add(s0);
    }
    (s0, s1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_cover_the_single_and_multi_byte_forms() {
        assert_eq!(read_varint(&[0x00], 0).unwrap(), (0, 1));
        assert_eq!(read_varint(&[0x7F], 0).unwrap(), (127, 1));
        assert_eq!(read_varint(&[0x81, 0x00], 0).unwrap(), (128, 2));
        assert_eq!(read_varint(&[0x82, 0x21], 0).unwrap(), (289, 2));
        // Nine-byte form: the last byte contributes all eight bits.
        assert_eq!(
            read_varint(&[0xFF; 9], 0).unwrap(),
            (-1, 9),
            "all-ones varint is -1 when reinterpreted as i64"
        );
        assert!(
            read_varint(&[0x81], 0).is_err(),
            "truncated varint must error"
        );
    }

    #[test]
    fn serial_types_decode_to_the_right_values() {
        let enc = Encoding::Utf8;
        assert_eq!(decode_value(&[], 0, 0, enc).unwrap().0, Value::Null);
        assert_eq!(decode_value(&[], 0, 8, enc).unwrap().0, Value::Int(0));
        assert_eq!(decode_value(&[], 0, 9, enc).unwrap().0, Value::Int(1));
        assert_eq!(decode_value(&[0xFF], 0, 1, enc).unwrap().0, Value::Int(-1));
        // 24-bit and 48-bit integers must sign-extend.
        assert_eq!(
            decode_value(&[0xFF, 0xFF, 0xFF], 0, 3, enc).unwrap().0,
            Value::Int(-1)
        );
        assert_eq!(
            decode_value(&[0xFF; 6], 0, 5, enc).unwrap().0,
            Value::Int(-1)
        );
        assert_eq!(
            decode_value(b"hi", 0, 13 + 2 * 2, enc).unwrap().0,
            Value::Text("hi".into())
        );
        assert_eq!(
            decode_value(&[1, 2], 0, 12 + 2 * 2, enc).unwrap().0,
            Value::Blob(vec![1, 2])
        );
        assert!(
            decode_value(&[], 0, 10, enc).is_err(),
            "reserved type errors"
        );
    }

    #[test]
    fn blob_columns_holding_json_read_back_as_text() {
        // This is exactly how VS Code stores ItemTable.value.
        let v = Value::Blob(br#"{"a":1}"#.to_vec());
        assert_eq!(v.as_str(), Some(r#"{"a":1}"#));
        assert_eq!(Value::Blob(vec![0xFF, 0xFE]).as_str(), None);
    }

    #[test]
    fn checksum_matches_a_known_seed_chain() {
        // Two u32 words: s0 = 1 + 0, s1 = 2 + s0.
        let data = [0, 0, 0, 1, 0, 0, 0, 2];
        assert_eq!(checksum(&data, (0, 0), true), (1, 3));
        // Little-endian selection changes the word decode.
        assert_eq!(
            checksum(&data, (0, 0), false),
            (1 << 24, (2 << 24) + (1 << 24))
        );
        // Trailing bytes that don't fill a word pair are ignored, as in SQLite.
        assert_eq!(checksum(&[0, 0, 0, 1], (5, 6), true), (5, 6));
    }

    #[test]
    fn a_non_sqlite_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let open_err = |name: &str, bytes: &[u8]| -> String {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            match ReadOnlyDb::open(&path) {
                Ok(_) => panic!("{name} must not parse as a database"),
                Err(e) => e.to_string(),
            }
        };

        // Long enough to reach the magic check.
        let err = open_err("not.db", &[b'x'; 200]);
        assert!(err.contains("not a SQLite database"), "got: {err}");

        // Too short even for a header.
        let err = open_err("short.db", b"SQLite format 3\0");
        assert!(err.contains("too short"), "got: {err}");
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        assert!(ReadOnlyDb::open(Path::new("/nonexistent/state.vscdb")).is_err());
        // (`is_err` avoids needing Debug on the success type.)
    }
}
