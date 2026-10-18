use sha2::{Digest, Sha256};
use thiserror::Error;

// Deliberately no Debug or Serialize: exposing a credential is always explicit.
pub(crate) struct SecretToken([u8; 32]);

#[derive(Debug, Error)]
pub(crate) enum TokenError {
    #[error("could not obtain secure random bytes")]
    Entropy,
    #[error("invalid token format")]
    Invalid,
}

impl SecretToken {
    pub(crate) fn generate() -> Result<Self, TokenError> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|_| TokenError::Entropy)?;
        Ok(Self(bytes))
    }

    pub(crate) fn parse(value: &str) -> Result<Self, TokenError> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(TokenError::Invalid);
        }
        let mut bytes = [0; 32];
        hex::decode_to_slice(value, &mut bytes).map_err(|_| TokenError::Invalid)?;
        Ok(Self(bytes))
    }

    pub(crate) fn expose(&self) -> String {
        hex::encode(self.0)
    }

    pub(crate) fn hash(&self) -> [u8; 32] {
        Sha256::digest(self.0).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_round_trip_without_hashing_the_printable_representation() {
        let token = SecretToken::generate().unwrap();
        let encoded = token.expose();
        assert_eq!(encoded.len(), 64);
        assert_eq!(SecretToken::parse(&encoded).unwrap().hash(), token.hash());
        assert_ne!(
            token.hash(),
            <[u8; 32]>::from(Sha256::digest(encoded.as_bytes()))
        );
        for value in ["bad", &"A".repeat(64), &"g".repeat(64)] {
            assert!(SecretToken::parse(value).is_err());
        }
    }
}
