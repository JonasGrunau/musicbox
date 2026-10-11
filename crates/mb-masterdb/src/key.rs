//! master.db passphrase derivation. Spec: `docs/re/masterdb.md#key`.
//!
//! rekordbox stores the SQLCipher passphrase in `options.json` (`dp`) as base64 of a Blowfish-ECB
//! (big-endian) encryption. The Blowfish key is a constant string in the rekordbox binary.

use crate::{Error, Result};
use base64::Engine;
use blowfish::Blowfish;
use blowfish::cipher::{Array, BlockCipherDecrypt, KeyInit};

/// cstring in the arm64 slice of rekordbox 7.2.19 at VA 0x104FAE8DD.
const DP_BLOWFISH_KEY: &[u8] = b"ZOwUlUZYqe9Rdm6j";

pub fn derive_passphrase(dp: &str) -> Result<String> {
    let mut data = base64::engine::general_purpose::STANDARD
        .decode(dp.trim())
        .map_err(|e| Error::Passphrase(format!("not base64: {e}")))?;
    if data.is_empty() || data.len() % 8 != 0 {
        return Err(Error::Passphrase(format!(
            "length {} is not a multiple of the 8-byte block",
            data.len()
        )));
    }
    let cipher = <Blowfish>::new_from_slice(DP_BLOWFISH_KEY)
        .map_err(|e| Error::Passphrase(e.to_string()))?;
    for chunk in data.chunks_exact_mut(8) {
        let block: &mut Array<u8, _> = chunk.try_into().expect("8-byte chunk");
        cipher.decrypt_block(block);
    }
    // No padding scheme: the plaintext is NUL/whitespace-filled to the block size.
    let end = data
        .iter()
        .rposition(|b| !matches!(b, 0 | b' ' | b'\t' | b'\r' | b'\n'))
        .map_or(0, |i| i + 1);
    data.truncate(end);
    String::from_utf8(data).map_err(|_| Error::Passphrase("plaintext is not UTF-8".into()))
}
