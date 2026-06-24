//! File signature ("magic number") detection from a leading byte slice.

/// A coarse classification of a file by its leading bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signature {
    Png,
    Jpeg,
    Gif,
    Pdf,
    Gzip,
    Zip,
    Bzip2,
    Xz,
    Zstd,
    Tar,
    Elf,
    Unknown,
}

impl Signature {
    /// True when this signature denotes an archive/compressed container.
    pub fn is_archive(self) -> bool {
        matches!(
            self,
            Signature::Gzip
                | Signature::Zip
                | Signature::Bzip2
                | Signature::Xz
                | Signature::Zstd
                | Signature::Tar
        )
    }

    /// True when this signature denotes a raster image.
    pub fn is_image(self) -> bool {
        matches!(self, Signature::Png | Signature::Jpeg | Signature::Gif)
    }
}

/// Detect a signature from the head of a file's contents.
pub fn detect_signature(head: &[u8]) -> Signature {
    let starts = |sig: &[u8]| head.len() >= sig.len() && &head[..sig.len()] == sig;
    if starts(&[0x89, b'P', b'N', b'G']) {
        Signature::Png
    } else if starts(&[0xFF, 0xD8, 0xFF]) {
        Signature::Jpeg
    } else if starts(b"GIF8") {
        Signature::Gif
    } else if starts(b"%PDF") {
        Signature::Pdf
    } else if starts(&[0x1F, 0x8B]) {
        Signature::Gzip
    } else if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
        Signature::Zip
    } else if starts(b"BZh") {
        Signature::Bzip2
    } else if starts(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]) {
        Signature::Xz
    } else if starts(&[0x28, 0xB5, 0x2F, 0xFD]) {
        Signature::Zstd
    } else if starts(&[0x7F, b'E', b'L', b'F']) {
        Signature::Elf
    } else if head.len() >= 262 && &head[257..262] == b"ustar" {
        Signature::Tar
    } else {
        Signature::Unknown
    }
}
