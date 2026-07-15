// Embedding生成層。
//
//  MockEmbedder (既定) : モデル不要・決定論的。UTF-8バイト列の文字n-gram(2,3)を
//                        FNV-1aハッシュで固定次元へ射影しL2正規化。
//                        意味理解はしないが「表記が近い質問は近いベクトルになる」。
//  OnnxEmbedder (任意) : ONNX Runtime C++ APIでMiniLM等を読む拡張点。
//                        CMakeオプション POC_USE_ONNX 有効時のみコンパイル。
#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <memory>
#include <string>
#include <vector>

namespace poc {

class IEmbedder {
public:
    virtual ~IEmbedder() = default;
    virtual std::string name() const = 0;
    virtual size_t dim() const = 0;
    // 正規化済み(L2ノルム1)のベクトルを返すこと
    virtual std::vector<float> encode(const std::string& text) const = 0;
};

class MockEmbedder final : public IEmbedder {
public:
    explicit MockEmbedder(size_t dim = 512) : dim_(dim) {}

    std::string name() const override { return "mock-ngram-hash"; }
    size_t dim() const override { return dim_; }

    std::vector<float> encode(const std::string& text) const override {
        std::vector<float> v(dim_, 0.0f);
        std::string t = text;
        std::transform(t.begin(), t.end(), t.begin(), [](unsigned char c) {
            return static_cast<char>(std::tolower(c));
        });
        for (size_t n : {size_t(2), size_t(3)}) {
            if (t.size() < n) continue;
            for (size_t i = 0; i + n <= t.size(); ++i) {
                v[fnv1a(t.data() + i, n) % dim_] += 1.0f;
            }
        }
        normalize(v);
        return v;
    }

    static void normalize(std::vector<float>& v) {
        double norm = 0.0;
        for (float x : v) norm += double(x) * x;
        norm = std::sqrt(norm);
        if (norm > 0) for (float& x : v) x = float(x / norm);
    }

private:
    static uint64_t fnv1a(const char* p, size_t n) {
        uint64_t h = 1469598103934665603ull;
        for (size_t i = 0; i < n; ++i) {
            h ^= static_cast<unsigned char>(p[i]);
            h *= 1099511628211ull;
        }
        return h;
    }

    size_t dim_;
};

std::unique_ptr<IEmbedder> create_embedder();

}  // namespace poc

#ifdef POC_USE_ONNX
#include "onnx_embedder.hpp"
#endif

namespace poc {

inline std::unique_ptr<IEmbedder> create_embedder() {
#ifdef POC_USE_ONNX
    try {
        return std::make_unique<OnnxEmbedder>();
    } catch (const std::exception& e) {
        // モデル未配置などの場合はMockへフォールバック
        std::fprintf(stderr, "[embedder] OnnxEmbedder init failed: %s -> Mockで続行\n", e.what());
    }
#endif
    return std::make_unique<MockEmbedder>();
}

}  // namespace poc
