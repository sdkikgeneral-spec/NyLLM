// 署名層(設計メモ §4: ハッシュ=改ざん検知、署名=詐称防止 の区別を実装)。
//
// S2.5 変更(docs/S2.5_エントリ形式設計.md §5, §10-5):
//   - 署名対象は immutable_core の正準バイト列(core_bytes)そのものになった。
//     core_bytes はバイナリであり &str に載らないため、trait を
//     sign_hex(&str) から sign_bytes(&[u8]) / verify(.., &[u8]) へ必須変更。
//   - DummySigner は旧 sha256(secret || payload) が長さ拡張攻撃を許すため、
//     HMAC-SHA256(RFC 2104)へ変更(§5 付随修正。依存追加なし・sha2 のみで手実装)。
//
//  DummySigner (既定): 依存ゼロのプレースホルダ。HMAC-SHA256 の鍵付きMAC。
//                      検証に秘密鍵が要るため公開検証不可 = 本物の署名ではない。
//                      単一ノードPoCで「署名付き登録・検証」のフローと
//                      インターフェースを成立させるための代替。
//  Ed25519Signer (任意, feature="ed25519"): ed25519-dalekによる実Ed25519署名。
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub trait Signer
{
    fn name(&self) -> &str;
    fn public_key_hex(&self) -> &str;
    // 正準バイト列(core_bytes)へ直接署名する(S2.5 §5)。戻り値は hex 文字列。
    fn sign_bytes(&self, data: &[u8]) -> String;
    // 同じバイト列に対する署名検証。実装が変わっても
    // 「ハッシュ=改ざん検知/署名=偽造防止」の分離(設計メモ §4)を維持する。
    fn verify(&self, pub_hex: &str, sig_hex: &str, data: &[u8]) -> bool;
}

fn sha256_hex(data: &[u8]) -> String
{
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

// HMAC-SHA256(RFC 2104 手実装。ブロック長64バイト、ipad=0x36 / opad=0x5C)。
// hmac クレートを追加せず sha2 のみで構成する(S2.5 §5 付随修正:
// 旧 sha256(secret || payload) は Merkle–Damgård の長さ拡張攻撃で
// 「payload の末尾に追記した偽MAC」が作れてしまうため HMAC 化)。
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32]
{
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK
    {
        // 鍵がブロック長を超える場合はハッシュして詰める(RFC 2104)
        let d = Sha256::digest(key);
        k[..d.len()].copy_from_slice(&d);
    }
    else
    {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0u8; BLOCK];
    let mut opad = [0u8; BLOCK];
    for i in 0..BLOCK
    {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5C;
    }
    let inner = Sha256::new().chain_update(ipad).chain_update(data).finalize();
    let outer = Sha256::new().chain_update(opad).chain_update(inner).finalize();
    outer.into()
}

pub struct DummySigner
{
    secret: Vec<u8>,
    pub_hex: String,
}

impl DummySigner
{
    pub fn new(key_path: &Path) -> std::io::Result<Self>
    {
        if let Some(parent) = key_path.parent()
        {
            fs::create_dir_all(parent)?;
        }
        let secret = if key_path.exists()
        {
            fs::read(key_path)?
        }
        else
        {
            use rand::RngCore;
            let mut s = vec![0u8; 32];
            rand::thread_rng().fill_bytes(&mut s);
            fs::write(key_path, &s)?;
            s
        };
        // 公開鍵導出は現行方式を維持(秘密鍵からの一方向導出。識別子用途のみ)
        let mut pub_input = b"pub:".to_vec();
        pub_input.extend_from_slice(&secret);
        let pub_hex = sha256_hex(&pub_input);
        Ok(Self { secret, pub_hex })
    }
}

impl Signer for DummySigner
{
    fn name(&self) -> &str
    {
        "dummy-mac(hmac-sha256)"
    }
    fn public_key_hex(&self) -> &str
    {
        &self.pub_hex
    }

    fn sign_bytes(&self, data: &[u8]) -> String
    {
        hex::encode(hmac_sha256(&self.secret, data))
    }

    fn verify(&self, pub_hex: &str, sig_hex: &str, data: &[u8]) -> bool
    {
        // 自ノードの鍵でのみ検証可能(MACの限界)。他ノード鍵のエントリは検証不能。
        if pub_hex != self.pub_hex
        {
            return false;
        }
        sig_hex == self.sign_bytes(data)
    }
}

#[cfg(feature = "ed25519")]
mod ed25519_signer;
#[cfg(feature = "ed25519")]
pub use ed25519_signer::Ed25519Signer;

pub fn create_signer(key_path: &Path) -> std::io::Result<Box<dyn Signer>>
{
    #[cfg(feature = "ed25519")]
    {
        return Ok(Box::new(Ed25519Signer::new(key_path)?));
    }
    #[cfg(not(feature = "ed25519"))]
    {
        Ok(Box::new(DummySigner::new(key_path)?))
    }
}
