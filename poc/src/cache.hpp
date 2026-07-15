// Winny型 Semantic Cache PoC — キャッシュ本体。
//
// エントリのデータモデル(設計メモ §4 スキーマの縮小版):
//   entry_id (=ファイル名) : sha256(署名対象ペイロード) … 改ざん"検知"用
//   author_sig             : 署名(既定はダミーMAC、-DPOC_USE_SODIUM でEd25519)
//   witness_sigs           : 単一ノードPoCのため省略
//
// 検索: 全エントリの正規化済みEmbeddingとの内積(=コサイン類似度)を
//       総当たり計算。PoC規模ではO(n)で十分(将来faiss等に差し替え可能)。
#pragma once

#include <algorithm>
#include <cstdio>
#include <ctime>
#include <filesystem>
#include <fstream>
#include <optional>
#include <string>
#include <vector>

#include "../vendor/nlohmann/json.hpp"
#include "../vendor/sha256.hpp"
#include "agent.hpp"
#include "embedder.hpp"
#include "signer.hpp"

namespace poc {

// しきい値(設計メモ §1, §2): ローカル利用は0.8前後、共有想定は精度優先で0.9+
inline constexpr float kLocalThreshold = 0.80f;
inline constexpr float kSharedThreshold = 0.90f;

struct CacheEntry {
    std::string question;
    std::string answer;               // answer_or_triples(PoCでは平文回答のみ)
    std::vector<float> embedding;     // 正規化済み
    std::string created;              // ISO8601(claimed_date 相当・平文)
    std::string volatility;           // permanent | slow | volatile
    bool shareable = false;
    std::string share_reason;
    std::string agent;
    std::string author_pub;
    std::string author_sig;
    std::string entry_id;             // sha256(signed_payload) = content hash

    // 署名対象 = 質問 + 回答 + 日付 + 揮発性(キー順ソートで正規化)
    std::string signed_payload() const {
        nlohmann::json j;  // nlohmann::json のオブジェクトはキー順ソートされる
        j["question"] = question;
        j["answer"] = answer;
        j["created"] = created;
        j["volatility"] = volatility;
        return j.dump();
    }
};

struct LookupResult {
    const CacheEntry* entry = nullptr;  // しきい値未満なら nullptr
    float similarity = 0.0f;
};

class SemanticCache {
public:
    SemanticCache(std::filesystem::path store_dir, const IEmbedder& embedder,
                  const ISigner& signer, float threshold = kLocalThreshold)
        : store_dir_(std::move(store_dir)),
          embedder_(embedder),
          signer_(signer),
          threshold_(threshold) {
        std::filesystem::create_directories(store_dir_);
        load();
    }

    size_t size() const { return entries_.size(); }
    float threshold() const { return threshold_; }

    LookupResult lookup(const std::string& question) const {
        LookupResult r;
        if (entries_.empty()) return r;
        const std::vector<float> q = embedder_.encode(question);
        for (const auto& e : entries_) {
            float sim = dot(e.embedding, q);
            if (sim > r.similarity) {
                r.similarity = sim;
                if (sim >= threshold_) r.entry = &e;
            }
        }
        if (r.similarity < threshold_) r.entry = nullptr;
        return r;
    }

    const CacheEntry& register_entry(const std::string& question, const std::string& answer,
                                     const std::string& volatility, bool shareable,
                                     const std::string& share_reason,
                                     const std::string& agent_name) {
        CacheEntry e;
        e.question = question;
        e.answer = answer;
        e.embedding = embedder_.encode(question);
        e.created = now_iso8601();
        e.volatility = volatility;
        e.shareable = shareable;
        e.share_reason = share_reason;
        e.agent = agent_name;
        e.author_pub = signer_.public_key_hex();
        const std::string payload = e.signed_payload();
        e.author_sig = signer_.sign_hex(payload);
        e.entry_id = Sha256::hex(payload);

        save(e);
        entries_.push_back(std::move(e));
        return entries_.back();
    }

private:
    static float dot(const std::vector<float>& a, const std::vector<float>& b) {
        if (a.size() != b.size()) return 0.0f;
        double s = 0.0;
        for (size_t i = 0; i < a.size(); ++i) s += double(a[i]) * b[i];
        return float(s);
    }

    static std::string now_iso8601() {
        std::time_t t = std::time(nullptr);
        std::tm tm{};
#ifdef _WIN32
        localtime_s(&tm, &t);
#else
        localtime_r(&t, &tm);
#endif
        char buf[32];
        std::strftime(buf, sizeof(buf), "%Y-%m-%dT%H:%M:%S", &tm);
        return buf;
    }

    void save(const CacheEntry& e) const {
        nlohmann::json j;
        j["question"] = e.question;
        j["answer"] = e.answer;
        j["embedding"] = e.embedding;
        j["created"] = e.created;
        j["volatility"] = e.volatility;
        j["shareable"] = e.shareable;
        j["share_reason"] = e.share_reason;
        j["agent"] = e.agent;
        j["author_pub"] = e.author_pub;
        j["author_sig"] = e.author_sig;
        j["entry_id"] = e.entry_id;
        std::ofstream out(store_dir_ / (e.entry_id + ".json"), std::ios::binary);
        out << j.dump(1);
    }

    void load() {
        namespace fs = std::filesystem;
        std::vector<fs::path> files;
        for (const auto& p : fs::directory_iterator(store_dir_))
            if (p.path().extension() == ".json") files.push_back(p.path());
        std::sort(files.begin(), files.end());

        for (const auto& f : files) {
            CacheEntry e;
            try {
                std::ifstream in(f, std::ios::binary);
                nlohmann::json j = nlohmann::json::parse(in);
                e.question = j.at("question").get<std::string>();
                e.answer = j.at("answer").get<std::string>();
                e.embedding = j.at("embedding").get<std::vector<float>>();
                e.created = j.at("created").get<std::string>();
                e.volatility = j.at("volatility").get<std::string>();
                e.shareable = j.at("shareable").get<bool>();
                e.share_reason = j.at("share_reason").get<std::string>();
                e.agent = j.at("agent").get<std::string>();
                e.author_pub = j.at("author_pub").get<std::string>();
                e.author_sig = j.at("author_sig").get<std::string>();
                e.entry_id = j.at("entry_id").get<std::string>();
            } catch (const std::exception& ex) {
                std::printf("[cache] 破損エントリをスキップ: %s (%s)\n",
                             f.filename().string().c_str(), ex.what());
                continue;
            }
            if (!verify(e, f.stem().string())) {
                std::printf("[cache] 検証失敗エントリをスキップ: %s\n",
                             f.filename().string().c_str());
                continue;
            }
            entries_.push_back(std::move(e));
        }
    }

    // 改ざん検知(content hash) + 署名検証(author_sig)
    bool verify(const CacheEntry& e, const std::string& expected_id) const {
        const std::string payload = e.signed_payload();
        const std::string h = Sha256::hex(payload);
        if (e.entry_id != h || expected_id != h) return false;
        return signer_.verify(e.author_pub, e.author_sig, payload);
    }

    std::filesystem::path store_dir_;
    const IEmbedder& embedder_;
    const ISigner& signer_;
    float threshold_;
    std::vector<CacheEntry> entries_;
};

}  // namespace poc
