// 署名層(設計メモ §4: ハッシュ=改ざん検知、署名=詐称防止 の区別を実装)。
//
//  DummySigner (既定): 依存ゼロのプレースホルダ。sha256(secret || payload) の
//                      鍵付きMAC。検証に秘密鍵が要るため公開検証不可 = 本物の
//                      署名ではない。単一ノードPoCで「署名付き登録・検証」の
//                      フローとインターフェースを成立させるための代替。
//  Ed25519Signer (任意, feature="ed25519"): ed25519-dalekによる実Ed25519署名。
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub trait Signer {
    fn name(&self) -> &str;
    fn public_key_hex(&self) -> &str;
    fn sign_hex(&self, payload: &str) -> String;
    fn verify(&self, pub_hex: &str, sig_hex: &str, payload: &str) -> bool;
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub struct DummySigner {
    secret: Vec<u8>,
    pub_hex: String,
}

impl DummySigner {
    pub fn new(key_path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = key_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let secret = if key_path.exists() {
            fs::read(key_path)?
        } else {
            use rand::RngCore;
            let mut s = vec![0u8; 32];
            rand::thread_rng().fill_bytes(&mut s);
            fs::write(key_path, &s)?;
            s
        };
        let mut pub_input = b"pub:".to_vec();
        pub_input.extend_from_slice(&secret);
        let pub_hex = sha256_hex(&pub_input);
        Ok(Self { secret, pub_hex })
    }
}

impl Signer for DummySigner {
    fn name(&self) -> &str {
        "dummy-mac(sha256)"
    }
    fn public_key_hex(&self) -> &str {
        &self.pub_hex
    }

    fn sign_hex(&self, payload: &str) -> String {
        let mut data = self.secret.clone();
        data.extend_from_slice(payload.as_bytes());
        sha256_hex(&data)
    }

    fn verify(&self, pub_hex: &str, sig_hex: &str, payload: &str) -> bool {
        // 自ノードの鍵でのみ検証可能(MACの限界)。他ノード鍵のエントリは検証不能。
        if pub_hex != self.pub_hex {
            return false;
        }
        sig_hex == self.sign_hex(payload)
    }
}

#[cfg(feature = "ed25519")]
mod ed25519_signer;
#[cfg(feature = "ed25519")]
pub use ed25519_signer::Ed25519Signer;

pub fn create_signer(key_path: &Path) -> std::io::Result<Box<dyn Signer>> {
    #[cfg(feature = "ed25519")]
    {
        return Ok(Box::new(Ed25519Signer::new(key_path)?));
    }
    #[cfg(not(feature = "ed25519"))]
    {
        Ok(Box::new(DummySigner::new(key_path)?))
    }
}
