//! Mozilla's `mozLz4` container: the magic `mozLz40\0`, the decompressed size as a
//! little-endian `u32`, then a plain LZ4 block. Firefox uses it for `addonStartup.json.lz4`,
//! `search.json.mozlz4`, session backups and more.

use crate::error::{ApiError, Result};

pub const MAGIC: &[u8; 8] = b"mozLz40\0";
/// Refuse to inflate anything larger than this (a corrupt size field must not exhaust memory).
const MAX_SIZE: usize = 256 * 1024 * 1024;

pub fn compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 16);
    out.extend_from_slice(MAGIC);
    // `compress_prepend_size` writes exactly the u32 LE length Firefox expects.
    out.extend_from_slice(&lz4_flex::block::compress_prepend_size(data));
    out
}

pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() < 12 || &bytes[..8] != MAGIC {
        return Err(ApiError::io("not a mozLz4 file"));
    }
    let size = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    if size > MAX_SIZE {
        return Err(ApiError::io("mozLz4 file claims an implausible size"));
    }
    lz4_flex::block::decompress_size_prepended(&bytes[8..])
        .map_err(|e| ApiError::io(format!("could not decompress the mozLz4 file: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_layout() {
        let data = br#"{"app-profile":{"addons":{"a@b":{"enabled":true}}}}"#.repeat(20);
        let c = compress(&data);
        assert_eq!(&c[..8], b"mozLz40\0");
        assert_eq!(
            u32::from_le_bytes([c[8], c[9], c[10], c[11]]) as usize,
            data.len()
        );
        assert_eq!(decompress(&c).unwrap(), data);
    }

    #[test]
    fn empty_and_garbage() {
        assert_eq!(decompress(&compress(b"")).unwrap(), b"");
        assert!(decompress(b"short").is_err());
        assert!(decompress(b"notmozlz4data!").is_err());
        let mut c = compress(b"hello hello hello hello");
        c[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decompress(&c).is_err());
    }
}
