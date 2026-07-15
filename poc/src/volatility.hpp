// 揮発性タグ付与と共有可否ゲート(設計メモ §3, §5 のL0ルール縮小版)。
//
//  - 揮発性: 時間指示語を含む → volatile / それ以外 → slow
//    (permanent昇格は案4=知識グラフ分解が必要なためPoCでは行わない。
//     「疑わしきはslow/volatile側へ」の非対称原則)
//  - 共有可否: 文脈自立(軸1) AND 事実型かつ非volatile(軸2)。
//    デフォルトは「共有しない」(保守的デフォルト)。
#pragma once

#include <string>
#include <vector>

namespace poc {

struct ShareDecision {
    bool shareable = false;
    std::string reason;
};

namespace detail {

// 部分一致(バイト列)。/utf-8 でコンパイルするためUTF-8リテラルのまま照合可能。
inline const char* find_term(const std::string& q, const std::vector<std::string>& terms) {
    for (const auto& t : terms)
        if (q.find(t) != std::string::npos) return t.c_str();
    return nullptr;
}

inline const std::vector<std::string>& volatile_terms() {
    static const std::vector<std::string> v = {
        "最新", "現在", "今日", "今の", "いま", "今年", "今月", "価格", "株価", "天気",
        "latest", "current", "today", "now", "price", "weather",
        "2025年", "2026年", "2027年"};
    return v;
}
inline const std::vector<std::string>& context_terms() {
    static const std::vector<std::string> v = {
        "それ", "その", "これ", "あれ", "さっき", "上記", "前述", "彼",
        "続けて", "もっと", "次は", "変えて", "直して",
        " it ", " that ", " above "};
    return v;
}
inline const std::vector<std::string>& subjective_terms() {
    static const std::vector<std::string> v = {
        "おすすめ", "べき", "どう思う", "好き",
        "best", "should", "recommend", "opinion"};
    return v;
}
inline const std::vector<std::string>& personal_terms() {
    static const std::vector<std::string> v = {
        "私の", "自分の", "俺の", "僕の", "うちの", "このファイル", "このコード",
        "my ", "our "};
    return v;
}

}  // namespace detail

// L0ルール: 時間指示語 → volatile / それ以外 → slow
inline std::string classify_volatility(const std::string& question) {
    return detail::find_term(question, detail::volatile_terms()) ? "volatile" : "slow";
}

// 共有可否ANDゲート。全チェック通過時のみ共有可。
inline ShareDecision share_gate(const std::string& question, const std::string& volatility) {
    using namespace detail;
    if (const char* t = find_term(question, context_terms()))
        return {false, std::string("文脈依存語を含む: '") + t + "'"};
    if (const char* t = find_term(question, subjective_terms()))
        return {false, std::string("主観・意見語を含む: '") + t + "'"};
    if (const char* t = find_term(question, personal_terms()))
        return {false, std::string("個人参照を含む: '") + t + "'"};
    if (volatility == "volatile")
        return {false, "volatile(時事)のためローカル短期TTLのみ"};
    return {true, "文脈自立 かつ 非volatile: 共有可"};
}

}  // namespace poc
