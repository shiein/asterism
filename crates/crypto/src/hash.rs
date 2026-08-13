use std::io::Read;

use asterism_core::BlobId;

use crate::error::Result;

pub fn blake3_bytes(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

pub fn blake3_reader(mut reader: impl Read) -> Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(*hasher.finalize().as_bytes())
}

pub fn blob_id_of(data: &[u8]) -> BlobId {
    BlobId::from_blake3(&blake3_bytes(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reader_matches_bytes() {
        let data = b"asterism";
        let a = blake3_bytes(data);
        let b = blake3_reader(Cursor::new(data)).unwrap();
        assert_eq!(a, b);
    }
}
