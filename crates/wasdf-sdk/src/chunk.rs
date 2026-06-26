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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_a_file_in_offset_chunks() {
        let mut path = std::env::temp_dir();
        path.push(format!("wasdf-chunk-{}.bin", std::process::id()));
        std::fs::write(&path, b"0123456789").unwrap();

        // First chunk: 4 bytes from the start, more remains.
        let (b, eof) = read_chunk_at(&path, 0, 4).unwrap();
        assert_eq!((b.as_slice(), eof), (b"0123".as_slice(), false));
        // Middle chunk from an offset.
        let (b, eof) = read_chunk_at(&path, 4, 4).unwrap();
        assert_eq!((b.as_slice(), eof), (b"4567".as_slice(), false));
        // Final chunk reaches EOF.
        let (b, eof) = read_chunk_at(&path, 8, 4).unwrap();
        assert_eq!((b.as_slice(), eof), (b"89".as_slice(), true));
        // Past the end: empty and EOF.
        let (b, eof) = read_chunk_at(&path, 99, 4).unwrap();
        assert!(b.is_empty() && eof);

        let _ = std::fs::remove_file(&path);
    }
}
