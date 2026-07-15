// Winny型 Semantic Cache PoC — 最小ループの通しデモ。
//
//   質問 → Embedding → 意味検索
//     ├ ヒット(類似度 >= しきい値) → キャッシュ回答
//     └ ミス → Agent推論 → 揮発性タグ → 共有可否ゲート → 署名付き登録 → 回答
//
// 使い方:
//   semantic_cache_poc            … キャッシュを初期化してデモ実行
//   semantic_cache_poc --keep     … 既存キャッシュを保持したまま実行
#include <chrono>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <string>
#include <vector>

#include "agent.hpp"
#include "cache.hpp"
#include "embedder.hpp"
#include "signer.hpp"
#include "volatility.hpp"

#ifdef _WIN32
#include <windows.h>
#endif

namespace fs = std::filesystem;
using namespace poc;

// UTF-8境界を壊さないよう max_bytes 以内で切り詰める
static std::string clip_utf8(const std::string& s, size_t max_bytes) {
    if (s.size() <= max_bytes) return s;
    size_t n = max_bytes;
    while (n > 0 && (static_cast<unsigned char>(s[n]) & 0xC0) == 0x80) --n;  // 継続バイトを避ける
    return s.substr(0, n) + "...";
}

int main(int argc, char** argv) {
#ifdef _WIN32
    SetConsoleOutputCP(CP_UTF8);  // 日本語UTF-8出力の文字化け防止
#endif
    bool keep = false;
    for (int i = 1; i < argc; ++i)
        if (std::strcmp(argv[i], "--keep") == 0) keep = true;

    const fs::path store = "cache_store";
    const fs::path keyfile = fs::path("keys") / "node.key";
    if (!keep && fs::exists(store)) fs::remove_all(store);

    auto embedder = create_embedder();
    auto signer = create_signer(keyfile);
    auto agent = create_agent();
    SemanticCache cache(store, *embedder, *signer, kLocalThreshold);

    std::printf("========================================================\n");
    std::printf("embedder : %s (dim=%zu)\n", embedder->name().c_str(), embedder->dim());
    std::printf("signer   : %s\n", signer->name().c_str());
    std::printf("agent    : %s\n", agent->name().c_str());
    std::printf("threshold: %.2f (共有想定なら %.2f+)\n", cache.threshold(), kSharedThreshold);
    std::printf("node pub : %.16s...\n", signer->public_key_hex().c_str());
    std::printf("既存キャッシュ: %zu 件\n", cache.size());
    std::printf("========================================================\n");

    const std::vector<std::string> questions = {
        "Winnyとは何ですか?",
        "Winnyとは何ですか?",              // 完全一致 → ヒット(sim=1.0)
        "Winnyって何?",                    // 言い換え → Mock埋め込みでは類似度を表示
        "P2Pの仕組みを教えてください",
        "最新のClaudeのモデルは何ですか?",  // volatile → 共有不可
        "おすすめのエディタはどれですか?",   // 主観 → 共有不可
    };

    int hits = 0, misses = 0;
    for (const auto& q : questions) {
        std::printf("\nQ: %s\n", q.c_str());
        auto t0 = std::chrono::steady_clock::now();
        LookupResult r = cache.lookup(q);
        auto us = std::chrono::duration_cast<std::chrono::microseconds>(
                      std::chrono::steady_clock::now() - t0).count();
        if (r.entry) {
            ++hits;
            std::printf("  -> HIT  (sim=%.3f, %lld us) 元の質問: \"%s\"\n",
                        r.similarity, static_cast<long long>(us), r.entry->question.c_str());
            std::printf("     A: %s\n", clip_utf8(r.entry->answer, 80).c_str());
            continue;
        }
        ++misses;
        std::printf("  -> MISS (best sim=%.3f) → Agent(%s) へ推論委譲\n",
                    r.similarity, agent->name().c_str());
        std::string answer = agent->ask(q);
        std::string vol = classify_volatility(q);
        ShareDecision dec = share_gate(q, vol);
        const CacheEntry& e =
            cache.register_entry(q, answer, vol, dec.shareable, dec.reason, agent->name());
        std::printf("     A: %s\n", clip_utf8(answer, 80).c_str());
        std::printf("     volatility=%s / 共有=%s (%s)\n", vol.c_str(),
                    dec.shareable ? "可" : "不可", dec.reason.c_str());
        std::printf("     登録 id=%.16s... sig=%.16s...\n", e.entry_id.c_str(),
                    e.author_sig.c_str());
    }

    std::printf("\n========================================================\n");
    std::printf("結果: %d hits / %d misses / キャッシュ %zu 件\n", hits, misses, cache.size());

    // 改ざん検知デモ: 保存済みエントリのanswerを書き換えて再読込
    std::printf("\n--- 改ざん検知デモ ---\n");
    for (const auto& p : fs::directory_iterator(store)) {
        if (p.path().extension() != ".json") continue;
        std::ifstream in(p.path(), std::ios::binary);
        nlohmann::json j = nlohmann::json::parse(in);
        in.close();
        j["answer"] = "【毒入り】" + j["answer"].get<std::string>();  // 悪意ある書き換えを模擬
        std::ofstream out(p.path(), std::ios::binary);
        out << j.dump(1);
        break;  // 1件だけ改ざん
    }
    SemanticCache reloaded(store, *embedder, *signer, kLocalThreshold);
    std::printf("1件を書き換え → 再読込後の有効エントリ: %zu 件 (改ざん分は除外)\n",
                reloaded.size());
    return 0;
}
