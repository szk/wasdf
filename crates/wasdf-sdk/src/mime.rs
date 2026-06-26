//! Coarse MIME classification used to pick a content renderer.

use crate::magic::{Signature, detect_signature};

/// The renderer-relevant class of a file's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MimeClass {
    Text,
    Image,
    Binary,
}

/// Classify content from its leading bytes. Heuristic: a known image signature
/// is an image; otherwise a NUL byte or a high ratio of control bytes marks it
/// binary; everything else is treated as text.
pub fn detect_mime(head: &[u8]) -> MimeClass {
    if detect_signature(head).is_image() {
        return MimeClass::Image;
    }
    if head.is_empty() {
        return MimeClass::Text;
    }
    if head.contains(&0) {
        return MimeClass::Binary;
    }
    let control = head
        .iter()
        .filter(|&&b| b < 0x09 || (0x0E..0x20).contains(&b))
        .count();
    if control * 100 / head.len() > 30 {
        MimeClass::Binary
    } else {
        MimeClass::Text
    }
}

/// Whether the leading bytes look like an archive container.
pub fn is_archive(head: &[u8]) -> bool {
    detect_signature(head).is_archive()
}

/// Re-exported signature helper for convenience.
pub fn signature_of(head: &[u8]) -> Signature {
    detect_signature(head)
}
