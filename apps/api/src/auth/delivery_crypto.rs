use std::env;

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use super::DeliveryError;

pub(super) struct DeliveryKey {
    cipher: XChaCha20Poly1305,
    id: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Invitation {
    pub(super) email: String,
    pub(super) origin: String,
    pub(super) code: String,
}

impl Drop for Invitation {
    fn drop(&mut self) {
        self.email.zeroize();
        self.origin.zeroize();
        self.code.zeroize();
    }
}

pub(super) struct EncryptedInvitation {
    pub(super) nonce: [u8; 24],
    pub(super) ciphertext: Vec<u8>,
}

impl DeliveryKey {
    pub(super) fn from_env() -> Result<Self, DeliveryError> {
        let encoded =
            Zeroizing::new(env::var("TURNESTA_DELIVERY_KEY").map_err(|_| DeliveryError::Key)?);
        Self::from_hex(&encoded)
    }

    fn from_hex(encoded: &str) -> Result<Self, DeliveryError> {
        if encoded.len() != 64 {
            return Err(DeliveryError::Key);
        }
        let mut bytes = Zeroizing::new([0_u8; 32]);
        hex::decode_to_slice(encoded, &mut *bytes).map_err(|_| DeliveryError::Key)?;
        let mut digest = Sha256::new();
        digest.update(bytes.as_slice());
        let id = digest.finalize().into();
        let cipher = XChaCha20Poly1305::new_from_slice(&*bytes).map_err(|_| DeliveryError::Key)?;
        Ok(Self { cipher, id })
    }

    pub(super) fn id(&self) -> &[u8; 32] {
        &self.id
    }

    pub(super) fn encrypt(
        &self,
        job_id: Uuid,
        code_hash: &[u8; 32],
        invitation: &Invitation,
    ) -> Result<EncryptedInvitation, DeliveryError> {
        let mut nonce = [0_u8; 24];
        getrandom::fill(&mut nonce).map_err(|_| DeliveryError::Entropy)?;
        let plaintext =
            Zeroizing::new(serde_json::to_vec(invitation).map_err(|_| DeliveryError::Payload)?);
        let aad = self.aad(job_id, code_hash);
        let ciphertext = self
            .cipher
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| DeliveryError::Encryption)?;
        Ok(EncryptedInvitation { nonce, ciphertext })
    }

    pub(super) fn decrypt(
        &self,
        job_id: Uuid,
        code_hash: &[u8; 32],
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<Invitation, DeliveryError> {
        let nonce: [u8; 24] = nonce.try_into().map_err(|_| DeliveryError::Payload)?;
        let aad = self.aad(job_id, code_hash);
        let plaintext = Zeroizing::new(
            self.cipher
                .decrypt(
                    &XNonce::from(nonce),
                    Payload {
                        msg: ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| DeliveryError::Payload)?,
        );
        serde_json::from_slice(&plaintext).map_err(|_| DeliveryError::Payload)
    }

    fn aad(&self, job_id: Uuid, code_hash: &[u8; 32]) -> Vec<u8> {
        let mut aad = b"turnesta.invitation-delivery.v1\0".to_vec();
        aad.extend_from_slice(job_id.as_bytes());
        aad.extend_from_slice(code_hash);
        aad.extend_from_slice(&self.id);
        aad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_payload_authenticates_the_job_code_key_and_ciphertext() {
        let key = DeliveryKey::from_hex(&"17".repeat(32)).unwrap();
        let other_key = DeliveryKey::from_hex(&"29".repeat(32)).unwrap();
        let job = Uuid::new_v4();
        let hash = [3_u8; 32];
        let invitation = Invitation {
            email: "recipient@example.test".to_owned(),
            origin: "http://127.0.0.1:8080".to_owned(),
            code: "credential fixture".to_owned(),
        };
        let mut encrypted = key.encrypt(job, &hash, &invitation).unwrap();
        let decrypted = key
            .decrypt(job, &hash, &encrypted.nonce, &encrypted.ciphertext)
            .unwrap();
        assert!(decrypted.email == invitation.email && decrypted.code == invitation.code);
        assert!(
            !encrypted
                .ciphertext
                .windows(invitation.code.len())
                .any(|bytes| bytes == invitation.code.as_bytes())
        );
        assert!(
            key.decrypt(
                Uuid::new_v4(),
                &hash,
                &encrypted.nonce,
                &encrypted.ciphertext
            )
            .is_err()
        );
        assert!(
            key.decrypt(job, &[4_u8; 32], &encrypted.nonce, &encrypted.ciphertext)
                .is_err()
        );
        assert!(
            other_key
                .decrypt(job, &hash, &encrypted.nonce, &encrypted.ciphertext)
                .is_err()
        );
        encrypted.ciphertext[0] ^= 1;
        assert!(
            key.decrypt(job, &hash, &encrypted.nonce, &encrypted.ciphertext)
                .is_err()
        );
    }
}
