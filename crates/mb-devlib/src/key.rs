//! exportLibrary.db passphrase derivation. Spec: `docs/re/devlib.md#key`.
//!
//! rekordbox keeps the SQLCipher passphrase in the binary as an encoded string and decodes it at
//! start-up through its generic "Blowfish helper" (the same helper also decodes the master.db
//! passphrase from a second constant). The steps, all verified in the arm64 slice of 7.2.19:
//!
//! 1. Decode the string with JUCE's `MemoryBlock` base64 flavour: `<byte count>.<6-bit chars>`,
//!    alphabet `.A-Za-z0-9+`, bits packed least-significant first.
//! 2. Read the bytes as little-endian 32-bit words and Blowfish-decrypt each (L, R) word pair
//!    with the ASCII key `alphatheta corporation` (standard Blowfish key schedule).
//! 3. Write the words back little-endian, read the bytes as a UTF-8 string (up to the first NUL)
//!    and strip a trailing pad: if the last character's code is below the 8-byte block size,
//!    that many characters are removed.

use crate::{Error, Result};
use blowfish::BlowfishLE;
use blowfish::cipher::{Array, BlockCipherDecrypt, KeyInit};

/// Encoded passphrase: the cstring right after `exportLibrary.db` in rekordbox 7.2.19 (arm64
/// slice, VA 0x104F8E7D8). The static initialiser of `rekordboxDevSqliteDB.cpp` copies it into
/// `db::DevSqliteDBController::_dbPwd` (VA 0x1056C7508).
pub const ENCODED_PASSPHRASE: &str =
    "64.FMiVKYF6lAn2Y0OtK8F4SGJGclnMl.T4RW3pw80t7Y+euW+AIy8K1.8aJhpZMo+J+nun.OyDvTsCD6C6mTenVC";

/// Blowfish key: cstring at VA 0x104F8E94E, loaded by `DevSqliteDBController::startupDeviceDB`
/// (@0x1006F4B38) and handed to `CryptoHelpers::BlowFishHelper::BlowFishHelper`.
pub const BLOWFISH_KEY: &str = "alphatheta corporation";

/// `BlowFishHelper` stores 8 (the Blowfish block size) as its first field and uses it as the
/// pad threshold when converting the decrypted words back into a string.
const BLOCK_SIZE: u32 = 8;

/// JUCE `MemoryBlock::toBase64Encoding` alphabet: index = 6-bit value.
const JUCE_BASE64: &[u8; 64] = b".ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+";

/// Decodes JUCE's `MemoryBlock` base64 flavour (`juce::MemoryBlock::fromBase64Encoding`,
/// @0x1024AA2EC): a decimal byte count, a `.`, then one character per 6 bits, written into a
/// zeroed buffer of that size least-significant bit first. Bits past the end are dropped.
///
/// JUCE itself maps unknown characters between `+` and `z` to 0 and skips all others; we reject
/// them so a mistyped constant fails loudly.
pub fn juce_base64_decode(encoded: &str) -> Result<Vec<u8>> {
    let (count, chars) = encoded
        .split_once('.')
        .ok_or_else(|| Error::Passphrase("missing '.' after the byte count".into()))?;
    let count: usize = count
        .trim()
        .parse()
        .map_err(|_| Error::Passphrase(format!("bad byte count {count:?}")))?;
    let mut out = vec![0u8; count];
    let mut bit = 0usize;
    for c in chars.bytes() {
        let value = JUCE_BASE64
            .iter()
            .position(|&a| a == c)
            .ok_or_else(|| Error::Passphrase(format!("invalid character {:?}", c as char)))?
            as u32;
        for i in 0..6 {
            let pos = bit + i;
            if pos / 8 < out.len() && value & (1 << i) != 0 {
                out[pos / 8] |= 1 << (pos % 8);
            }
        }
        bit += 6;
    }
    Ok(out)
}

/// Derives the SQLCipher passphrase from an encoded constant such as [`ENCODED_PASSPHRASE`]
/// (`CryptoHelpers::BlowFishHelper::decryptedString` @0x10056F9AC with the key
/// [`BLOWFISH_KEY`]). The result is handed to `sqlite3_key` as-is (SQLCipher 4 defaults, PBKDF2).
pub fn derive_passphrase(encoded: &str) -> Result<String> {
    decode_with_key(encoded, BLOWFISH_KEY.as_bytes())
}

/// The passphrase of every exportLibrary.db written by rekordbox 7.2.19.
pub fn passphrase() -> String {
    derive_passphrase(ENCODED_PASSPHRASE).expect("the built-in constant decodes")
}

/// The generic helper: same steps as [`derive_passphrase`] with an arbitrary Blowfish key.
pub fn decode_with_key(encoded: &str, key: &[u8]) -> Result<String> {
    let mut data = juce_base64_decode(encoded)?;
    if data.is_empty() {
        return Err(Error::Passphrase("empty payload".into()));
    }
    // `conv_string_to_array` (@0x10057003C) reads ceil(len / 4) words and `decrypt_array`
    // (@0x10056FE18) pairs them, padding with zero: so pad to whole 8-byte blocks.
    data.resize(data.len().div_ceil(8) * 8, 0);
    let cipher = BlowfishLE::new_from_slice(key)
        .map_err(|_| Error::Passphrase("Blowfish key must be 4..=56 bytes".into()))?;
    for chunk in data.chunks_exact_mut(8) {
        let block: &mut Array<u8, _> = chunk.try_into().expect("8-byte chunk");
        cipher.decrypt_block(block);
    }
    // `conv_array_to_string` (@0x10056FB68): bytes -> String stops at the first NUL.
    if let Some(nul) = data.iter().position(|&b| b == 0) {
        data.truncate(nul);
    }
    let mut text =
        String::from_utf8(data).map_err(|_| Error::Passphrase("plaintext is not UTF-8".into()))?;
    if let Some(last) = text.chars().last()
        && (last as u32) < BLOCK_SIZE
    {
        let keep = text.chars().count().saturating_sub(last as usize);
        text = text.chars().take(keep).collect();
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn juce_base64_bit_order() {
        // '.' = 0, 'A' = 1, 'B' = 2: one byte whose lowest bit is set.
        assert_eq!(juce_base64_decode("1.A.").unwrap(), vec![1]);
        // 6 + 2 bits: '+' = 63 fills bits 0..6, 'B' = 2 sets bit 7.
        assert_eq!(juce_base64_decode("1.+B").unwrap(), vec![0xBF]);
        // Bits past the declared byte count are dropped.
        assert_eq!(juce_base64_decode("1.+++").unwrap(), vec![0xFF]);
        assert!(juce_base64_decode("nodot").is_err());
        assert!(juce_base64_decode("1.*").is_err());
    }

    #[test]
    fn builtin_passphrase_shape() {
        let p = passphrase();
        assert!(!p.is_empty());
        assert!(p.is_ascii(), "{p:?}");
    }
}
