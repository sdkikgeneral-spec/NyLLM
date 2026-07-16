// Ed25519Signer — feature="ed25519" 時のみコンパイルされる実署名経路。
use super::Signer;
use ed25519_dalek::{Signature, SigningKey, Signer as DalekSigner, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use std::fs;
use std::io::{Error, ErrorKind};
use std::path::Path;

pub struct Ed25519Signer {
    signing_key: SigningKey,
    pub_hex: String,
}

impl Ed25519Signer {
    pub fn new(key_path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = key_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let signing_key = if key_path.exists() {
            let bytes = fs::read(key_path)?;
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| Error::new(ErrorKind::InvalidData, "node.key: 長さがEd25519鍵と不一致"))?;
            SigningKey::from_bytes(&arr)
        } else {
            let signing_key = SigningKey::generate(&mut OsRng);
            fs::write(key_path, signing_key.to_bytes())?;
            signing_key
        };
        let pub_hex = hex::encode(signing_key.verifying_key().to_bytes());
        Ok(Self { signing_key, pub_hex })
    }
}

impl Signer for Ed25519Signer {
    fn name(&self) -> &str {
        "ed25519(ed25519-dalek)"
    }
    fn public_key_hex(&self) -> &str {
        &self.pub_hex
    }

    fn sign_hex(&self, payload: &str) -> String {
        let sig: Signature = self.signing_key.sign(payload.as_bytes());
        hex::encode(sig.to_bytes())
    }

    fn verify(&self, pub_hex: &str, sig_hex: &str, payload: &str) -> bool {
        let Ok(pub_bytes) = hex::decode(pub_hex) else { return false };
        let Ok(sig_bytes) = hex::decode(sig_hex) else { return false };
        let Ok(pub_arr) = <[u8; 32]>::try_from(pub_bytes.as_slice()) else { return false };
        let Ok(sig_arr) = <[u8; 64]>::try_from(sig_bytes.as_slice()) else { return false };
        let Ok(vk) = VerifyingKey::from_bytes(&pub_arr) else { return false };
        let sig = Signature::from_bytes(&sig_arr);
        vk.verify(payload.as_bytes(), &sig).is_ok()
    }
}
