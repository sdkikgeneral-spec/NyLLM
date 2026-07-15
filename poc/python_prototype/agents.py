"""Agent層。キャッシュミス時の推論委譲先を抽象化する。

- MockAgent  : ネット/APIキー無しで通しデモを動かすための固定回答
- ClaudeAgent: Anthropic Claude (claude-opus-4-8) による実推論。
               ANTHROPIC_API_KEY 等の資格情報がある環境でのみ使用可。
"""

from __future__ import annotations

import os
from typing import Protocol


class Agent(Protocol):
    name: str

    def ask(self, question: str) -> str: ...


class MockAgent:
    """固定知識ベース + 汎用フォールバックのモック。"""

    name = "mock"

    _KB = {
        "winny": "Winnyは金子勇氏が2002年に開発したP2Pファイル共有ソフトウェアです。"
                 "中央サーバーを持たない純粋P2P型で、キャッシュの中継により匿名性を高める設計でした。",
        "p2p": "P2P(Peer-to-Peer)は、中央サーバーを介さずノード同士が対等に直接通信する"
               "ネットワーク方式です。各ノードがクライアントとサーバーの両方の役割を担います。",
        "claude": "(モック回答)最新のClaudeについてはAnthropic公式の発表をご確認ください。",
    }

    def ask(self, question: str) -> str:
        q = question.lower()
        for k, v in self._KB.items():
            if k in q:
                return v
        return f"(モック回答) 「{question}」への回答をここでLLMが生成します。"


class ClaudeAgent:
    """Anthropic Claude API による実推論Agent。"""

    name = "claude-opus-4-8"

    def __init__(self, model: str = "claude-opus-4-8"):
        import anthropic

        self.name = model
        self._client = anthropic.Anthropic()
        self._model = model

    def ask(self, question: str) -> str:
        resp = self._client.messages.create(
            model=self._model,
            max_tokens=1024,
            thinking={"type": "adaptive"},
            system="簡潔かつ正確に日本語で回答してください。",
            messages=[{"role": "user", "content": question}],
        )
        return next(b.text for b in resp.content if b.type == "text")


def create_agent(prefer_real: bool = True) -> "Agent":
    """APIキーがあればClaude、無ければモックを返す。"""
    if prefer_real and os.environ.get("ANTHROPIC_API_KEY"):
        try:
            return ClaudeAgent()
        except Exception as e:
            print(f"[agent] ClaudeAgent を初期化できません ({e})。モックで続行します")
    return MockAgent()
