// OnnxEmbedder — ONNX Runtime C++ API による実Embedding経路(拡張点)。
//
// CMakeオプション -DPOC_USE_ONNX=ON かつ ONNX Runtime のヘッダ/ライブラリが
// ある環境でのみコンパイルされる。既定ビルドでは本ファイルは使用されない。
//
// 注意(PoCの割り切り):
//  - トークナイザは未実装。sentence-transformers系モデルはWordPiece等の
//    トークナイズが必要で、実運用では tokenizers-cpp 等を併用する。
//    ここでは「ONNXモデルをロードし推論を呼ぶ」骨格のみ示す。
//  - モデルファイルは poc/models/model.onnx に配置する想定。
#pragma once
#ifdef POC_USE_ONNX

#include <onnxruntime_cxx_api.h>

#include <stdexcept>
#include <string>
#include <vector>

#include "embedder.hpp"

namespace poc {

class OnnxEmbedder final : public IEmbedder {
public:
    explicit OnnxEmbedder(const std::string& model_path = "models/model.onnx")
        : env_(ORT_LOGGING_LEVEL_WARNING, "poc"),
          session_(nullptr) {
        Ort::SessionOptions opts;
        opts.SetIntraOpNumThreads(1);
#ifdef _WIN32
        std::wstring wpath(model_path.begin(), model_path.end());
        session_ = Ort::Session(env_, wpath.c_str(), opts);
#else
        session_ = Ort::Session(env_, model_path.c_str(), opts);
#endif
    }

    std::string name() const override { return "onnx-runtime"; }
    size_t dim() const override { return 384; }  // MiniLM想定

    std::vector<float> encode(const std::string& /*text*/) const override {
        // TODO(拡張点): トークナイズ -> input_ids/attention_mask を作り
        // session_.Run() で last_hidden_state を得て mean pooling + L2正規化。
        throw std::runtime_error(
            "OnnxEmbedder::encode は未実装です(トークナイザ統合が必要)。"
            "PoCでは MockEmbedder を使用してください。");
    }

private:
    Ort::Env env_;
    mutable Ort::Session session_;
};

}  // namespace poc

#endif  // POC_USE_ONNX
