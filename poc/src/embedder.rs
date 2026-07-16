// Embedding生成層。
//
//  MockEmbedder (既定) : モデル不要・決定論的。UTF-8バイト列の文字n-gram(2,3)を
//                        FNV-1aハッシュで固定次元へ射影しL2正規化。
//                        意味理解はしないが「表記が近い質問は近いベクトルになる」。
//  OnnxEmbedder (任意, feature="onnx") : 実Embedding経路の拡張点。骨格のみ(未配線)。

pub trait Embedder {
    fn name(&self) -> &str;
    fn dim(&self) -> usize;
    /// 正規化済み(L2ノルム1)のベクトルを返すこと
    fn encode(&self, text: &str) -> Vec<f32>;
}

pub struct MockEmbedder {
    dim: usize,
}

impl MockEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }

    fn fnv1a(bytes: &[u8]) -> u64 {
        let mut h: u64 = 1469598103934665603;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        h
    }

    fn normalize(v: &mut [f32]) {
        let norm: f64 = v.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>().sqrt();
        if norm > 0.0 {
            for x in v.iter_mut() {
                *x = (*x as f64 / norm) as f32;
            }
        }
    }
}

impl Default for MockEmbedder {
    fn default() -> Self {
        Self::new(512)
    }
}

impl Embedder for MockEmbedder {
    fn name(&self) -> &str {
        "mock-ngram-hash"
    }
    fn dim(&self) -> usize {
        self.dim
    }

    fn encode(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; self.dim];
        let t = text.to_lowercase();
        let bytes = t.as_bytes();
        for n in [2usize, 3usize] {
            if bytes.len() < n {
                continue;
            }
            for i in 0..=(bytes.len() - n) {
                let h = Self::fnv1a(&bytes[i..i + n]);
                v[(h as usize) % self.dim] += 1.0;
            }
        }
        Self::normalize(&mut v);
        v
    }
}

#[cfg(feature = "onnx")]
mod onnx_embedder;
#[cfg(feature = "onnx")]
pub use onnx_embedder::OnnxEmbedder;

pub fn create_embedder() -> Box<dyn Embedder> {
    #[cfg(feature = "onnx")]
    {
        match OnnxEmbedder::new("models/model.onnx") {
            Ok(e) => return Box::new(e),
            Err(e) => eprintln!("[embedder] OnnxEmbedder init failed: {e} -> Mockで続行"),
        }
    }
    Box::new(MockEmbedder::default())
}
