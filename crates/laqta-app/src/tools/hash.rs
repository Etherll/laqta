//! The hash checker: digests of a file or a text, and a comparison against an expected value (M5 hash checker).

use std::path::Path;

use laqta_ports::AppError;
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};
use xxhash_rust::xxh3::Xxh3;
use xxhash_rust::xxh64::Xxh64;

use super::common::io_error;
use crate::Ports;
use crate::files;
use crate::task::compute;

/// Bytes read per step. Each step reads on the blocking pool and hashes on the compute pool.
const CHUNK: usize = 1 << 20;

/// A digest algorithm the checker knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HashKind {
    Crc32,
    Md5,
    Sha1,
    Sha256,
    Sha384,
    Sha512,
    Blake3,
    Xxh64,
    Xxh3,
}

impl HashKind {
    /// Every kind, in the order the checker lists them.
    pub const ALL: [HashKind; 9] = [
        HashKind::Crc32,
        HashKind::Md5,
        HashKind::Sha1,
        HashKind::Sha256,
        HashKind::Sha384,
        HashKind::Sha512,
        HashKind::Blake3,
        HashKind::Xxh64,
        HashKind::Xxh3,
    ];
}

/// A digest in progress. The large states are boxed so the enum stays small.
enum Digester {
    Crc32(crc32fast::Hasher),
    Md5(Md5),
    Sha1(Sha1),
    Sha256(Sha256),
    Sha384(Box<Sha384>),
    Sha512(Box<Sha512>),
    Blake3(Box<blake3::Hasher>),
    Xxh64(Xxh64),
    Xxh3(Box<Xxh3>),
}

impl Digester {
    fn new(kind: HashKind) -> Digester {
        match kind {
            HashKind::Crc32 => Digester::Crc32(crc32fast::Hasher::new()),
            HashKind::Md5 => Digester::Md5(Md5::new()),
            HashKind::Sha1 => Digester::Sha1(Sha1::new()),
            HashKind::Sha256 => Digester::Sha256(Sha256::new()),
            HashKind::Sha384 => Digester::Sha384(Box::new(Sha384::new())),
            HashKind::Sha512 => Digester::Sha512(Box::new(Sha512::new())),
            HashKind::Blake3 => Digester::Blake3(Box::new(blake3::Hasher::new())),
            HashKind::Xxh64 => Digester::Xxh64(Xxh64::new(0)),
            HashKind::Xxh3 => Digester::Xxh3(Box::new(Xxh3::new())),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Digester::Crc32(hasher) => hasher.update(bytes),
            Digester::Md5(hasher) => Digest::update(hasher, bytes),
            Digester::Sha1(hasher) => Digest::update(hasher, bytes),
            Digester::Sha256(hasher) => Digest::update(hasher, bytes),
            Digester::Sha384(hasher) => Digest::update(&mut **hasher, bytes),
            Digester::Sha512(hasher) => Digest::update(&mut **hasher, bytes),
            Digester::Blake3(hasher) => {
                hasher.update(bytes);
            }
            Digester::Xxh64(hasher) => hasher.update(bytes),
            Digester::Xxh3(hasher) => hasher.update(bytes),
        }
    }

    /// The digest as lower-case hex. Integer digests are zero-padded to their full width.
    fn finish(self) -> String {
        match self {
            Digester::Crc32(hasher) => format!("{:08x}", hasher.finalize()),
            Digester::Md5(hasher) => hex(&hasher.finalize()),
            Digester::Sha1(hasher) => hex(&hasher.finalize()),
            Digester::Sha256(hasher) => hex(&hasher.finalize()),
            Digester::Sha384(hasher) => hex(&hasher.finalize()),
            Digester::Sha512(hasher) => hex(&hasher.finalize()),
            Digester::Blake3(hasher) => hasher.finalize().to_hex().to_string(),
            Digester::Xxh64(hasher) => format!("{:016x}", hasher.digest()),
            Digester::Xxh3(hasher) => format!("{:016x}", hasher.digest()),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The digest of `text` (its UTF-8 bytes) as lower-case hex.
pub fn hash_text(text: &str, kind: HashKind) -> String {
    let mut digest = Digester::new(kind);
    digest.update(text.as_bytes());
    digest.finish()
}

/// Whether `actual` is the digest `expected`. Case and surrounding spaces do not matter.
pub fn compare_hash(expected: &str, actual: &str) -> bool {
    expected.trim().eq_ignore_ascii_case(actual.trim())
}

/// The digests of the file at `path`, one per kind, in the order of `kinds`. The file is read in 1 MiB chunks, so its
/// size does not limit the check. Empty `kinds` gives an empty list without reading the file.
pub async fn hash_file(ports: &Ports, path: &Path, kinds: &[HashKind]) -> Result<Vec<(HashKind, String)>, AppError> {
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    let spawner = ports.spawner.as_ref();
    let mut reader = files::open_chunks(spawner, path.to_path_buf(), CHUNK).await.map_err(io_error)?;
    let mut digests: Vec<(HashKind, Digester)> = kinds.iter().map(|&kind| (kind, Digester::new(kind))).collect();
    while let Some(bytes) = reader.next_chunk(spawner).await.map_err(io_error)? {
        digests = compute(spawner, move || {
            for (_, digest) in &mut digests {
                digest.update(&bytes);
            }
            Ok(digests)
        })
        .await?;
    }
    Ok(digests.into_iter().map(|(kind, digest)| (kind, digest.finish())).collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn text_digests_match_the_standard_vectors() {
        assert_eq!(hash_text("abc", HashKind::Md5), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hash_text("abc", HashKind::Sha1), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hash_text("abc", HashKind::Sha256), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hash_text("abc", HashKind::Crc32), "352441c2");
        assert_eq!(hash_text("", HashKind::Crc32), "00000000");
    }

    #[test]
    fn every_kind_gives_lower_hex_of_its_width() {
        let widths = [
            (HashKind::Crc32, 8),
            (HashKind::Md5, 32),
            (HashKind::Sha1, 40),
            (HashKind::Sha256, 64),
            (HashKind::Sha384, 96),
            (HashKind::Sha512, 128),
            (HashKind::Blake3, 64),
            (HashKind::Xxh64, 16),
            (HashKind::Xxh3, 16),
        ];
        for (kind, width) in widths {
            let digest = hash_text("abc", kind);
            assert_eq!(digest.len(), width, "{kind:?}");
            assert!(digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{kind:?}");
        }
    }

    #[test]
    fn compare_ignores_case_and_spaces() {
        assert!(compare_hash(" ABC123 ", "abc123"));
        assert!(!compare_hash("abc123", "abc124"));
    }
}
