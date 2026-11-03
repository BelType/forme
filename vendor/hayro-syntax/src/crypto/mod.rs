// forme's copy of hayro-syntax 0.7.2: no crypto. Upstream's decryption (RC4, AES-128 and AES-256 with MD5 and the SHA-2 family, ported from pdf.js) is removed with its six files, and a file whose trailer names an /Encrypt dictionary is refused as encrypted, whatever its password; Caster refuses an encrypted facsimile before the list (NOTICE says what changed and why).

//! What is left of the cryptography: the refusal of an encrypted file.

use crate::object::{Dict, ObjectIdentifier};
use alloc::vec::Vec;

/// An error that occurred during decryption.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DecryptionError {
    /// The ID entry is missing in the PDF.
    MissingIDEntry,
    /// The PDF is password-protected and no password (or a wrong one) was
    /// provided.
    PasswordProtected,
    /// The PDF has invalid encryption.
    InvalidEncryption,
    /// The PDF uses an unsupported encryption algorithm.
    UnsupportedAlgorithm,
}

#[derive(Debug, Clone)]
pub(crate) enum Decryptor {
    None,
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum DecryptionTarget {
    String,
    Stream,
}

impl Decryptor {
    pub(crate) fn decrypt(
        &self,
        _id: ObjectIdentifier,
        data: &[u8],
        _target: DecryptionTarget,
    ) -> Option<Vec<u8>> {
        Some(data.to_vec())
    }
}

/// forme: every encryption dictionary is refused, since no decryptor is linked.
pub(crate) fn get(
    _dict: &Dict<'_>,
    _id: &[u8],
    _password: &[u8],
) -> Result<Decryptor, DecryptionError> {
    Err(DecryptionError::UnsupportedAlgorithm)
}
