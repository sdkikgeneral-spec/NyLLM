// 署名層(設計メモ §4: ハッシュ=改ざん検知、署名=詐称防止 の区別を実装)。
//
//  SodiumSigner (任意) : libsodium の Ed25519。CMakeオプション POC_USE_SODIUM。
//  DummySigner  (既定) : 依存ゼロのプレースホルダ。sha256(secret || payload) の
//                        鍵付きMAC。検証に秘密鍵が要るため公開検証不可 = 本物の
//                        署名ではない。単一ノードPoCで「署名付き登録・検証」の
//                        フローとインターフェースを成立させるための代替。
#pragma once

#include <filesystem>
#include <fstream>
#include <memory>
#include <random>
#include <string>

#include "../vendor/sha256.hpp"

namespace poc {

class ISigner {
public:
    virtual ~ISigner() = default;
    virtual std::string name() const = 0;
    virtual std::string public_key_hex() const = 0;
    virtual std::string sign_hex(const std::string& payload) const = 0;
    virtual bool verify(const std::string& pub_hex, const std::string& sig_hex,
                        const std::string& payload) const = 0;
};

class DummySigner final : public ISigner {
public:
    explicit DummySigner(const std::filesystem::path& key_path) {
        namespace fs = std::filesystem;
        fs::create_directories(key_path.parent_path());
        if (fs::exists(key_path)) {
            std::ifstream in(key_path, std::ios::binary);
            secret_.assign(std::istreambuf_iterator<char>(in), {});
        } else {
            std::random_device rd;
            secret_.resize(32);
            for (auto& c : secret_) c = char(rd() & 0xff);
            std::ofstream out(key_path, std::ios::binary);
            out.write(secret_.data(), std::streamsize(secret_.size()));
        }
        pub_ = Sha256::hex("pub:" + secret_);
    }

    std::string name() const override { return "dummy-mac(sha256)"; }
    std::string public_key_hex() const override { return pub_; }

    std::string sign_hex(const std::string& payload) const override {
        return Sha256::hex(secret_ + payload);
    }

    bool verify(const std::string& pub_hex, const std::string& sig_hex,
                const std::string& payload) const override {
        // 自ノードの鍵でのみ検証可能(MACの限界)。他ノード鍵のエントリは検証不能。
        if (pub_hex != pub_) return false;
        return sig_hex == sign_hex(payload);
    }

private:
    std::string secret_;
    std::string pub_;
};

}  // namespace poc

#ifdef POC_USE_SODIUM
#include <sodium.h>

namespace poc {

class SodiumSigner final : public ISigner {
public:
    explicit SodiumSigner(const std::filesystem::path& key_path) {
        if (sodium_init() < 0) throw std::runtime_error("sodium_init failed");
        namespace fs = std::filesystem;
        fs::create_directories(key_path.parent_path());
        unsigned char pk[crypto_sign_PUBLICKEYBYTES];
        unsigned char sk[crypto_sign_SECRETKEYBYTES];
        if (fs::exists(key_path)) {
            std::ifstream in(key_path, std::ios::binary);
            in.read(reinterpret_cast<char*>(sk), sizeof(sk));
            crypto_sign_ed25519_sk_to_pk(pk, sk);
        } else {
            crypto_sign_keypair(pk, sk);
            std::ofstream out(key_path, std::ios::binary);
            out.write(reinterpret_cast<const char*>(sk), sizeof(sk));
        }
        sk_.assign(sk, sk + sizeof(sk));
        pub_ = bin2hex(pk, sizeof(pk));
    }

    std::string name() const override { return "ed25519(libsodium)"; }
    std::string public_key_hex() const override { return pub_; }

    std::string sign_hex(const std::string& payload) const override {
        unsigned char sig[crypto_sign_BYTES];
        crypto_sign_detached(sig, nullptr,
                             reinterpret_cast<const unsigned char*>(payload.data()),
                             payload.size(), sk_.data());
        return bin2hex(sig, sizeof(sig));
    }

    bool verify(const std::string& pub_hex, const std::string& sig_hex,
                const std::string& payload) const override {
        auto pk = hex2bin(pub_hex);
        auto sig = hex2bin(sig_hex);
        if (pk.size() != crypto_sign_PUBLICKEYBYTES || sig.size() != crypto_sign_BYTES)
            return false;
        return crypto_sign_verify_detached(
                   sig.data(),
                   reinterpret_cast<const unsigned char*>(payload.data()),
                   payload.size(), pk.data()) == 0;
    }

private:
    static std::string bin2hex(const unsigned char* p, size_t n) {
        static const char* k = "0123456789abcdef";
        std::string s;
        s.reserve(n * 2);
        for (size_t i = 0; i < n; ++i) {
            s.push_back(k[p[i] >> 4]);
            s.push_back(k[p[i] & 0xf]);
        }
        return s;
    }
    static std::vector<unsigned char> hex2bin(const std::string& h) {
        std::vector<unsigned char> out;
        if (h.size() % 2) return out;
        out.reserve(h.size() / 2);
        auto nib = [](char c) -> int {
            if (c >= '0' && c <= '9') return c - '0';
            if (c >= 'a' && c <= 'f') return c - 'a' + 10;
            if (c >= 'A' && c <= 'F') return c - 'A' + 10;
            return -1;
        };
        for (size_t i = 0; i < h.size(); i += 2) {
            int hi = nib(h[i]), lo = nib(h[i + 1]);
            if (hi < 0 || lo < 0) return {};
            out.push_back(static_cast<unsigned char>((hi << 4) | lo));
        }
        return out;
    }

    std::vector<unsigned char> sk_;
    std::string pub_;
};

}  // namespace poc
#endif  // POC_USE_SODIUM

namespace poc {

inline std::unique_ptr<ISigner> create_signer(const std::filesystem::path& key_path) {
#ifdef POC_USE_SODIUM
    return std::make_unique<SodiumSigner>(key_path);
#else
    return std::make_unique<DummySigner>(key_path);
#endif
}

}  // namespace poc
