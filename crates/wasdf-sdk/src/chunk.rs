//! Bounded chunked reading: never pull a whole large file into memory for a
//! bounded content read. Reads at most `limit` bytes from a given offset, so a
//! consumer can page through a large file one chunk at a time.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// Read up to `limit` bytes from `path` starting at byte `offset`. Returns the
/// bytes read and whether this chunk reached the end of the file (no more bytes
/// beyond it).
pub fn read_chunk_at(path: &Path, offset: u64, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut file = File::open(path)?;
    if offset > 0 {
        file.seek(SeekFrom::Start(offset))?;
    }
    let mut buf = vec![0u8; limit];
    let mut filled = 0;
    while filled < limit {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    buf.truncate(filled);
    // One more byte tells us whether the file continues past this chunk.
    let mut probe = [0u8; 1];
    let more = filled == limit && matches!(file.read(&mut probe), Ok(n) if n > 0);
    Ok((buf, !more))
}

/// Read up to `limit` bytes from the head of `path` (a `read_chunk_at` at offset
/// 0). Returns the bytes and whether the file was truncated by the limit.
pub fn read_chunk(path: &Path, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let (bytes, eof) = read_chunk_at(path, 0, limit)?;
    Ok((bytes, !eof))
}
