// Agent層。キャッシュミス時の推論委譲先を抽象化する。
//
//  MockAgent (既定): 固定知識ベース + 汎用フォールバック。ネット不要。
//  実LLM経路: IAgent を実装したクラス(例: Anthropic Claude のMessages APIを
//  HTTPクライアントで叩く ClaudeAgent)を差し込む拡張点。PoCではHTTP依存を
//  必須にしないため未実装(python_prototype/agents.py に実装例あり)。
#pragma once

#include <memory>
#include <string>
#include <utility>
#include <vector>

namespace poc {

class IAgent {
public:
    virtual ~IAgent() = default;
    virtual std::string name() const = 0;
    virtual std::string ask(const std::string& question) = 0;
};

class MockAgent final : public IAgent {
public:
    std::string name() const override { return "mock"; }

    std::string ask(const std::string& question) override {
        static const std::vector<std::pair<std::string, std::string>> kb = {
            {"Winny",
             "Winnyは金子勇氏が2002年に開発したP2Pファイル共有ソフトウェアです。"
             "中央サーバーを持たない純粋P2P型で、キャッシュの中継により匿名性を高める設計でした。"},
            {"winny",
             "Winnyは金子勇氏が2002年に開発したP2Pファイル共有ソフトウェアです。"},
            {"P2P",
             "P2P(Peer-to-Peer)は、中央サーバーを介さずノード同士が対等に直接通信する"
             "ネットワーク方式です。各ノードがクライアントとサーバーの両方の役割を担います。"},
        };
        for (const auto& [k, v] : kb)
            if (question.find(k) != std::string::npos) return v;
        return "(モック回答) 「" + question + "」への回答をここでLLMが生成します。";
    }
};

inline std::unique_ptr<IAgent> create_agent() {
    return std::make_unique<MockAgent>();
}

}  // namespace poc
