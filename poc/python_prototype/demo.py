"""Winny型 Semantic Cache PoC — 最小ループの通しデモ。

質問 → Embedding → 意味検索
  ├ ヒット(類似度 >= しきい値) → キャッシュ回答
  └ ミス → Agent推論 → 揮発性タグ付与 → 共有可否判定 → 署名付き登録 → 回答

実行:
    python demo.py            # キャッシュを毎回初期化してデモ
    python demo.py --keep     # 既存キャッシュを保持したまま実行
    python demo.py --offline  # 実Embedding/実LLMを使わず強制フォールバック
"""

from __future__ import annotations

import shutil
import sys
import time
from pathlib import Path

from agents import create_agent
from embedder import create_embedder
from semantic_cache import LOCAL_THRESHOLD, NodeKey, SemanticCache
from volatility import classify_volatility, share_gate

HERE = Path(__file__).parent
STORE = HERE / "cache_store"
KEY = HERE / "keys" / "node_ed25519.key"

QUESTIONS = [
    "Winnyとは何ですか?",
    "Winnyって何?",                      # 上と同義 → キャッシュヒット期待
    "P2Pの仕組みを教えてください",
    "最新のClaudeのモデルは何ですか?",     # volatile → 共有不可(ローカルのみ)
    "おすすめのエディタはどれですか?",      # 主観 → 共有不可
    "P2Pネットワークの仕組みについて教えて",  # 3問目と同義 → ヒット期待
]


def main() -> None:
    args = set(sys.argv[1:])
    if "--keep" not in args and STORE.exists():
        shutil.rmtree(STORE)

    offline = "--offline" in args
    embedder = create_embedder(prefer_real=not offline)
    agent = create_agent(prefer_real=not offline)
    key = NodeKey(KEY)
    cache = SemanticCache(STORE, embedder, key, threshold=LOCAL_THRESHOLD)

    print("=" * 72)
    print(f"embedder : {embedder.name} (dim={embedder.dim})")
    print(f"agent    : {agent.name}")
    print(f"threshold: {cache.threshold} (共有想定なら0.90+)")
    print(f"node pub : {key.pub_hex[:16]}...")
    print(f"既存キャッシュ: {len(cache)} 件")
    print("=" * 72)

    hits = misses = 0
    for q in QUESTIONS:
        print(f"\nQ: {q}")
        t0 = time.perf_counter()
        entry, sim = cache.lookup(q)
        if entry is not None:
            hits += 1
            ms = (time.perf_counter() - t0) * 1000
            print(f"  -> HIT  (sim={sim:.3f}, {ms:.1f}ms) 元の質問: {entry.question!r}")
            print(f"     A: {entry.answer[:80]}...")
            continue

        misses += 1
        print(f"  -> MISS (best sim={sim:.3f}) → Agent({agent.name}) へ推論委譲")
        answer = agent.ask(q)
        vol = classify_volatility(q)
        dec = share_gate(q, vol)
        e = cache.register(q, answer, vol, dec.shareable, dec.reason, agent.name)
        print(f"     A: {answer[:80]}...")
        print(f"     volatility={vol} / 共有={'可' if dec.shareable else '不可'} ({dec.reason})")
        print(f"     登録 id={e.content_hash[:16]}... sig={e.author_sig[:16]}...")

    print("\n" + "=" * 72)
    print(f"結果: {hits} hits / {misses} misses / キャッシュ {len(cache)} 件")

    # 改ざん検知デモ: 保存済みエントリの回答を書き換えて再読込
    print("\n--- 改ざん検知デモ ---")
    import json
    victim = next(STORE.glob("*.json"))
    data = json.loads(victim.read_text(encoding="utf-8"))
    data["answer"] = "【毒入り】" + data["answer"]  # 悪意ある書き換えを模擬
    victim.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
    reloaded = SemanticCache(STORE, embedder, key)
    print(f"1件を書き換え → 再読込後の有効エントリ: {len(reloaded)} 件 (改ざん分は除外)")


if __name__ == "__main__":
    main()
