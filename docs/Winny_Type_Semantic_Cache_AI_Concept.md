# Winny型 Semantic Cache AI 構想

## コンセプト

現在のLLMは、同じような質問でも毎回推論を実施している。

しかし現実には、

- 同じ質問
- 似た質問
- 過去に誰かが回答済みの質問

が大量に存在する。

そこで、

**「まず知識キャッシュを検索し、見つからなければ推論する」**

という構成を採用する。

---

# 基本思想

Winnyのような分散ネットワーク上に、

- Semantic Cache
- 回答キャッシュ
- 知識断片

を保持する。

AIはまずキャッシュを探し、

無ければ推論する。

---

# 全体構成

```text
利用者
  ↓

意味検索(NPU)

  ↓

P2P Semantic Cache

  ↓ ヒット

回答返却

  ↓ ミス

Agent層

  ├ OpenAI
  ├ Claude
  ├ Gemini
  ├ Local LLM
  └ 社内LLM

  ↓

推論

  ↓

回答生成

  ↓

キャッシュ登録
```

---

# 目標

## GPU依存の削減

従来

```text
質問
 ↓
毎回推論
 ↓
回答
```

提案方式

```text
質問
 ↓
意味検索

ヒット
 ↓
回答

ミス
 ↓
推論
 ↓
保存
```

### 効果

- GPU消費削減
- 電力削減
- レイテンシ削減
- LLM APIコスト削減

---

# Colibrìとの共通点

Colibrìは、

「必要なモデル断片のみSSDからロードする」

仕組み。

本構想は、

「必要な知識断片のみP2Pキャッシュから取得する」

考え方である。

どちらも、

> 全部を常時ロードしない

という発想である。

---

# Semantic Cache

通常キャッシュは文字列一致が必要。

```text
Winnyとは？
```

と

```text
Winnyって何？
```

は別扱いになる。

---

Semantic Cacheは、

質問をEmbedding化し、

意味的に近い質問を同一視する。

```text
Winnyとは？
Winnyって何？
P2PソフトWinnyについて教えて
```

↓

```text
同じ質問群
```

として扱う。

---

# NPUの活用

NPUは推論よりも、

- Embedding生成
- 意味検索
- 類似度判定
- 情報分類

に利用する。

---

想定フロー

```text
質問
 ↓
Embedding生成
 ↓
キャッシュ検索

ヒット
 ↓
回答返却

ミス
 ↓
Agentへ問い合わせ
```

結果として、

GPUは未知問題だけ担当する。

---

# ネットワークモード

## Public

公開ネットワーク。

用途

- 一般知識
- OSS
- 公開情報

---

## Company

社内限定。

用途

- 社内FAQ
- ナレッジ
- 社内RAG

---

## Private

完全ローカル。

用途

- 個人メモ
- 個人知識
- 機密情報

---

# 起動方法

推論時に毎回判定するのではなく、

起動時にモードを選択する。

```bash
ai-node --mode public
```

```bash
ai-node --mode company
```

```bash
ai-node --mode private
```

---

# UI案

利用者が誤操作しないよう、

実行アイコンを分離する。

```text
AI Public
AI Company
AI Private
```

利用者が常に現在のモードを意識できる。

---

# プライバシー設計

AIによる判定ではなく、

参加するネットワーク自体を分離する。

```text
Private
 ↓
ローカルのみ

Company
 ↓
社内のみ

Public
 ↓
全体共有
```

---

# Agent層

P2Pネットワークは推論しない。

推論はAgentへ委譲する。

---

利用可能Agent

```text
OpenAI
Claude
Gemini
Llama
ローカルLLM
社内LLM
```

---

# 最大の課題

## 回答の不一致

同じ質問でも、

```text
OpenAI
Claude
Gemini
```

で回答が異なる。

---

例

```text
Winnyとは？
```

↓

```text
回答A
回答B
回答C
```

---

# 解決案

## 案1

複数回答を保持

```text
Q

A1
A2
A3
```

もっともWinny的。

---

## 案2

Agent込みでハッシュ化

```text
hash(
質問
+
Agent
+
Model
+
Prompt
)
```

整合性が高い。

---

## 案3

投票方式

```text
回答A ★100
回答B ★30
回答C ★5
```

人気順で利用。

---

## 案4

知識グラフ化

回答を事実へ分解。

```text
Winny

開発者=金子勇
種別=P2P
```

一致率で信頼度を算出する。

---

# 理想構成

```text
┌──────────────────┐
│ NPU Layer        │
│                  │
│ 類似検索         │
│ Embedding生成    │
│ 分類             │
└──────┬───────────┘
       │
       ▼

┌──────────────────┐
│ Semantic Cache   │
│ P2P Network      │
└──────┬───────────┘
       │

Hit
 │
 ▼

回答返却

Miss
 │
 ▼

┌──────────────────┐
│ Agent Layer      │
└──────┬───────────┘
       │

OpenAI
Claude
Gemini
Local LLM

       │
       ▼

回答生成
       │
       ▼

P2P Cache登録
```

---

# 最終的なビジョン

これは単なる分散LLMではない。

むしろ、

**「人類規模の意味キャッシュネットワーク」**

である。

---

## キーワード

- Winny
- Semantic Cache
- P2P
- Agent
- NPU
- Embedding
- Distributed Knowledge
- Human Knowledge Cache
- Local First AI

---

## 一言で言うと

> AIに毎回考えさせるのではなく、
> 人類が過去に考えた結果をまず探し、
> 見つからなかった時だけAIに考えさせる。
>
> その知識の蓄積をWinnyのような分散ネットワークで実現する構想。
`