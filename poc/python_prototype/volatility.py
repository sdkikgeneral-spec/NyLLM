"""揮発性タグ付与と共有可否ゲート(設計メモ §3, §5 のL0ルール縮小版)。

- 揮発性: 時間指示語を含む → volatile / それ以外 → slow
  (permanent 昇格は案4=知識グラフ分解が必要なため PoC では行わない。
   「疑わしきは slow/volatile 側へ」の非対称原則に従う)
- 共有可否: 文脈自立(軸1) AND 事実型かつ非volatile(軸2)。
  デフォルトは「共有しない」。L0語彙ルールのみで判定(L2 Agent自己申告は省略)。
"""

from __future__ import annotations

from dataclasses import dataclass

# L0: 時間指示語 → volatile
VOLATILE_TERMS = [
    "最新", "現在", "今日", "今の", "いま", "今年", "今月", "価格", "株価", "天気",
    "latest", "current", "today", "now", "price", "weather",
    "2025年", "2026年", "2027年",
]

# L0: 文脈依存語(指示語・代名詞・対象省略の命令)→ 共有不可
CONTEXT_DEPENDENT_TERMS = [
    "それ", "その", "これ", "この前", "あれ", "さっき", "上記", "前述", "彼", "彼女",
    "続けて", "もっと", "次は", "変えて", "直して",
    " it ", " that ", " above ", " previous ",
]

# L0: 主観・意見語 → 共有不可
SUBJECTIVE_TERMS = [
    "おすすめ", "べき", "どう思う", "好き", "良いと思う",
    "best", "should", "recommend", "opinion",
]

# L0: 個人参照 → 共有不可(ローカルのみ)
PERSONAL_TERMS = [
    "私の", "自分の", "俺の", "僕の", "うちの", "このファイル", "このコード",
    "my ", "our ",
]


def classify_volatility(question: str) -> str:
    """L0ルール: 時間指示語 → volatile / それ以外 → slow。"""
    q = question.lower()
    if any(t.lower() in q for t in VOLATILE_TERMS):
        return "volatile"
    return "slow"


@dataclass
class ShareDecision:
    shareable: bool
    reason: str


def share_gate(question: str, volatility: str) -> ShareDecision:
    """共有可否のANDゲート。全チェック通過時のみ共有可(保守的デフォルト)。"""
    q = question.lower()

    for terms, label in (
        (CONTEXT_DEPENDENT_TERMS, "文脈依存語を含む"),
        (SUBJECTIVE_TERMS, "主観・意見語を含む"),
        (PERSONAL_TERMS, "個人参照を含む"),
    ):
        hit = next((t for t in terms if t.lower() in q), None)
        if hit:
            return ShareDecision(False, f"{label} ({hit.strip()!r})")

    if volatility == "volatile":
        return ShareDecision(False, "volatile(時事)のためローカル短期TTLのみ")

    return ShareDecision(True, "文脈自立 かつ 非volatile: 共有可")
