"""Embedding生成層。

優先: sentence-transformers (多言語MiniLM。MeanCacheの知見に基づきMPNet級の
      文埋め込みモデルを使う。PoCでは軽量な多言語MiniLMを採用)
代替: HashEmbedder (文字n-gramハッシュ。ネット/torch無し環境でも
      「似た文字列は似たベクトルになる」性質を粗く再現し、通しデモを可能にする)
"""

from __future__ import annotations

import hashlib

import numpy as np

DEFAULT_ST_MODEL = "paraphrase-multilingual-MiniLM-L12-v2"


class HashEmbedder:
    """文字n-gram(2,3-gram)のfeature hashingによる簡易埋め込み。

    意味理解はしないが、表記の近い質問同士は高コサイン類似度になる。
    オフライン環境でのフォールバック専用。
    """

    name = "hash-ngram(fallback)"
    dim = 512

    def encode(self, text: str) -> np.ndarray:
        vec = np.zeros(self.dim, dtype=np.float32)
        t = text.strip().lower()
        for n in (2, 3):
            for i in range(max(1, len(t) - n + 1)):
                gram = t[i : i + n]
                h = int.from_bytes(
                    hashlib.blake2s(gram.encode("utf-8"), digest_size=4).digest(), "big"
                )
                vec[h % self.dim] += 1.0
        norm = np.linalg.norm(vec)
        return vec / norm if norm > 0 else vec


class STEmbedder:
    """sentence-transformers による実Embedding。"""

    def __init__(self, model_name: str = DEFAULT_ST_MODEL):
        from sentence_transformers import SentenceTransformer

        self.name = f"sentence-transformers/{model_name}"
        self._model = SentenceTransformer(model_name)
        self.dim = self._model.get_sentence_embedding_dimension()

    def encode(self, text: str) -> np.ndarray:
        v = self._model.encode(text, normalize_embeddings=True)
        return np.asarray(v, dtype=np.float32)


def create_embedder(prefer_real: bool = True):
    """利用可能なら実モデル、無理ならフォールバックを返す。"""
    if prefer_real:
        try:
            return STEmbedder()
        except Exception as e:  # ImportError / モデルDL失敗など
            print(f"[embedder] sentence-transformers を使用できません ({type(e).__name__}: {e})")
            print("[embedder] ハッシュ埋め込みフォールバックで続行します")
    return HashEmbedder()
