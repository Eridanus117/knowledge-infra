---
description: "Mac mini 上 KB reranker 评测结论：BGE/MPS 可做短文本慢路径，全文直塞会爆 MPS buffer，全文质量评测应改窗口化离线 runner。"
keywords: [mac-mini, reranker, BGE, MPS, "KB recall", fulltext, "windowed rerank", cross-encoder, checkpoint]
kind: research
code: [experiments/rerank-bench/src/rerank_bench/rerank.py, experiments/rerank-bench/src/rerank_bench/server.py]
---

# Mac mini reranker eval findings

## 结论

Mac mini 可以当 KB reranker 评测机，但当前不应把 reranker 默认接入实时 recall 主链路。

截至 2026-06-18 的实测口径：

- 短文本 BGE reranker 是唯一接近实时的候选：`BAAI/bge-reranker-v2-m3` 在 Mac mini M4 / 16GB 上走 PyTorch MPS，`max_text_chars=256` 时 top10 p95 约 588ms、top20 p95 约 1.05s。
- `512/top20` 约 1.9s p95，`1024/top20` 约 4.5s p95；更长输入不适合交互默认路径。
- mxbai、Qwen3 PyTorch/MPS、Qwen3 MLX 4bit 均慢于 BGE；Qwen3 仍可作为质量对照，但不是默认实时 endpoint。
- 当前 30 条 recall-export 样本没有 gold/preferred labels。BGE 改变 top1 的比例高，不等于质量提升。

## 全文评测发现

`/tmp/rerank-bench-30.jsonl` 原始样本并非全文：600 个候选里 585 个已被 export 截到 1200 chars。重新导出 `--max-text-chars 0` 后得到 `/tmp/rerank-bench-30-fulltext.jsonl`：600 个候选不截断，长度中位数约 5565 chars，p95 约 15288 chars，最长约 21865 chars。

不能把整篇文档直接作为一个 cross-encoder pair 塞给 BGE：`top20` 全文直接预测在 MPS 上触发过 `RuntimeError: Invalid buffer size: 80.00 GiB`。这不是没有走 GPU，而是长序列 attention buffer 爆了。

可行的全文质量形态是窗口化：把每个文档拆成窗口，例如 2048 chars chunk / 1024 stride，BGE 对窗口打分，再按 parent document 聚合取最高窗口分。这个形态能覆盖全文，但非常慢：5 条 pilot 的 p50 约 64.9s、p95 约 66.9s；逐 case runner 后前 7/30 条已完成，每条约 49-67s。

## 设备事实

远端 endpoint 曾以如下形态运行：

```text
rerank-endpoint --host 127.0.0.1 --port 8011 --backend sentence-transformers \
  --model fixtures/users/anonymous/srv/models/bge-reranker-v2-m3 \
  --device mps --max-text-chars 0 --predict-batch-size 8
```

远端 PyTorch 探针确认：`torch.backends.mps.is_available() == True`，MPS matmul 可在 `mps:0` 上执行，CUDA 不可用。Mac mini 的 Apple Silicon GPU 不是 NVIDIA CUDA 路径，不能用 `nvidia-smi` 式指标判断利用率。

`rerank-bench` commit `8aeb835` 增加了 `--predict-batch-size`，用于限制 sentence-transformers CrossEncoder predict batch size，避免全文/窗口化质量评测时一次性 batch 触发 MPS buffer 爆炸。

## 当前资产

- 代码仓：`fixtures/users/anonymous/workspace/experiments/rerank-bench`
- 最新相关提交：`8aeb835 Add configurable CrossEncoder batch size`
- 全文 export：`/tmp/rerank-bench-30-fulltext.jsonl`
- 窗口化 export：`/tmp/rerank-bench-30-fulltext-windows-2048-1024.jsonl`
- 已暂停 checkpoint：`/tmp/rerank-bge-windowed-checkpoint-20260618220449.jsonl`，已完成 7/30
- Mac mini endpoint 已停止；`8011` 无监听，机器可关机

## 下一步

后续不要再用一次 HTTP 请求跑整条窗口化 case。应改成真正的离线 runner：远端本地读 JSONL，逐 case 写 checkpoint，输出 parent-document 聚合排名、窗口命中位置、耗时和错误。然后基于 gold/preferred labels 比较 embedding 原排序、BGE 1024 截断、BGE windowed fulltext、Qwen/Qwen3-MLX 的质量，而不是只比较延迟。
