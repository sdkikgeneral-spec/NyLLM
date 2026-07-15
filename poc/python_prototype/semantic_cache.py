"""Winny型 Semantic Cache PoC — キャッシュ本体。

エントリのデータモデル(設計メモ §4 のスキーマ縮小版):
    filename  : sha256(正規化した中身)  … 改ざん"検知"用(公開関数なので詐称は防げない)
    author_sig: Ed25519署名             … "誰が言ったか"の固定(詐称防止)
    witness_sigs は単一ノードPoCのため省略。

検索: 質問Embeddingと全エントリの正規化済みEmbeddingのコサイン類似度を
      numpy総当たりで計算(PoC規模ではO(n)で十分。MeanCacheもローカル線形検索)。
"""

from __future__ import annotations

import json
import time
from dataclasses import asdict, dataclass, field
from hashlib import sha256
from pathlib import Path

import numpy as np
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)
from cryptography.hazmat.primitives.serialization import (
    Encoding,
    NoEncryption,
    PrivateFormat,
    PublicFormat,
)

# しきい値(設計メモ §1, §2): ローカル利用は0.8前後、共有想定は精度優先で高め
LOCAL_THRESHOLD = 0.80
SHARED_THRESHOLD = 0.90


@dataclass
class CacheEntry:
    question: str
    answer: str
    embedding: list[float]          # 正規化済み
    created: str                    # ISO8601 (claimed_date に相当・平文)
    volatility: str                 # permanent | slow | volatile
    shareable: bool                 # 共有ゲート判定結果
    share_reason: str
    agent: str                      # 回答を生成したAgent名
    author_pub: str = ""            # 投稿ノード公開鍵(hex)
    author_sig: str = ""            # Ed25519署名(hex)
    content_hash: str = ""          # sha256(署名対象ペイロード) = ファイル名/ID

    def signed_payload(self) -> bytes:
        """署名対象 = 質問 + 回答 + 日付 + 揮発性(設計メモ §4)。"""
        obj = {
            "question": self.question,
            "answer": self.answer,
            "created": self.created,
            "volatility": self.volatility,
        }
        return json.dumps(obj, ensure_ascii=False, sort_keys=True).encode("utf-8")


class NodeKey:
    """ローカルノードのEd25519鍵ペア。無ければ生成して保存。"""

    def __init__(self, key_path: Path):
        key_path.parent.mkdir(parents=True, exist_ok=True)
        if key_path.exists():
            self._priv = Ed25519PrivateKey.from_private_bytes(key_path.read_bytes())
        else:
            self._priv = Ed25519PrivateKey.generate()
            key_path.write_bytes(
                self._priv.private_bytes(Encoding.Raw, PrivateFormat.Raw, NoEncryption())
            )
        self.pub_hex = self._priv.public_key().public_bytes(
            Encoding.Raw, PublicFormat.Raw
        ).hex()

    def sign(self, payload: bytes) -> str:
        return self._priv.sign(payload).hex()

    @staticmethod
    def verify(pub_hex: str, sig_hex: str, payload: bytes) -> bool:
        try:
            pub = Ed25519PublicKey.from_public_bytes(bytes.fromhex(pub_hex))
            pub.verify(bytes.fromhex(sig_hex), payload)
            return True
        except Exception:
            return False


class SemanticCache:
    def __init__(self, store_dir: Path, embedder, key: NodeKey,
                 threshold: float = LOCAL_THRESHOLD):
        self.store_dir = Path(store_dir)
        self.store_dir.mkdir(parents=True, exist_ok=True)
        self.embedder = embedder
        self.key = key
        self.threshold = threshold
        self._entries: list[CacheEntry] = []
        self._matrix: np.ndarray | None = None  # (n, dim) 正規化済み
        self._load()

    # ---------- 永続化 ----------

    def _load(self) -> None:
        for f in sorted(self.store_dir.glob("*.json")):
            try:
                data = json.loads(f.read_text(encoding="utf-8"))
                entry = CacheEntry(**data)
            except (json.JSONDecodeError, TypeError):
                print(f"[cache] 破損エントリをスキップ: {f.name}")
                continue
            if not self._verify(entry, expected_hash=f.stem):
                print(f"[cache] 検証失敗エントリをスキップ: {f.name}")
                continue
            self._entries.append(entry)
        self._rebuild_matrix()

    def _rebuild_matrix(self) -> None:
        if self._entries:
            self._matrix = np.array([e.embedding for e in self._entries], dtype=np.float32)
        else:
            self._matrix = None

    def _verify(self, entry: CacheEntry, expected_hash: str | None = None) -> bool:
        """改ざん検知(content hash) + 署名検証(author_sig)。"""
        payload = entry.signed_payload()
        h = sha256(payload).hexdigest()
        if entry.content_hash != h:
            return False
        if expected_hash is not None and expected_hash != h:
            return False
        return NodeKey.verify(entry.author_pub, entry.author_sig, payload)

    # ---------- 検索 ----------

    def lookup(self, question: str) -> tuple[CacheEntry | None, float]:
        """意味検索。しきい値以上の最良ヒットと類似度を返す。"""
        if self._matrix is None:
            return None, 0.0
        q = self.embedder.encode(question)
        sims = self._matrix @ q  # 正規化済み前提のコサイン類似度
        idx = int(np.argmax(sims))
        best = float(sims[idx])
        if best >= self.threshold:
            return self._entries[idx], best
        return None, best

    # ---------- 登録 ----------

    def register(self, question: str, answer: str, volatility: str,
                 shareable: bool, share_reason: str, agent: str) -> CacheEntry:
        emb = self.embedder.encode(question)
        entry = CacheEntry(
            question=question,
            answer=answer,
            embedding=[float(x) for x in emb],
            created=time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            volatility=volatility,
            shareable=shareable,
            share_reason=share_reason,
            agent=agent,
            author_pub=self.key.pub_hex,
        )
        payload = entry.signed_payload()
        entry.author_sig = self.key.sign(payload)
        entry.content_hash = sha256(payload).hexdigest()

        path = self.store_dir / f"{entry.content_hash}.json"
        path.write_text(
            json.dumps(asdict(entry), ensure_ascii=False, indent=1), encoding="utf-8"
        )
        self._entries.append(entry)
        self._rebuild_matrix()
        return entry

    def __len__(self) -> int:
        return len(self._entries)
