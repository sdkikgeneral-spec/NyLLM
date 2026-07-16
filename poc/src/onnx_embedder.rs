// OnnxEmbedder — 実Embedding経路の拡張点(骨格のみ)。
//
// feature="onnx" 時のみコンパイルされる。既定ビルドでは使用されない。
//
// 注意(PoCの割り切り):
//  - トークナイザは未実装。sentence-transformers系モデルはWordPiece等の
//    トークナイズが必要で、実運用では tokenizers クレート等を併用する。
//  - 実際のONNX Runtime呼び出しは ort クレート等の導入が必要(未配線)。
//  - モデルファイルは poc/models/model.onnx に配置する想定。
use super::Embedder;

pub struct OnnxEmbedder;

impl OnnxEmbedder {
    pub fn new(_model_path: &str) -> Result<Self, String> {
        Err("OnnxEmbedder は未実装です(ortクレート連携 + トークナイザ統合が必要)。\
             PoCでは MockEmbedder を使用してください。"
            .to_string())
    }
}

impl Embedder for OnnxEmbedder {
    fn name(&self) -> &str {
        "onnx-runtime"
    }
    fn dim(&self) -> usize {
        384 // MiniLM想定
    }

    fn encode(&self, _text: &str) -> Vec<f32> {
        panic!(
            "OnnxEmbedder::encode は未実装です(ortクレート連携 + トークナイザ統合が必要)。\
             PoCでは MockEmbedder を使用してください。"
        );
    }
}
