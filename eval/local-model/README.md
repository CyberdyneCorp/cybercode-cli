# Local compatible-adapter baseline

The original Qwen3 investigation is recorded below. [Qwen3.5 9B](qwen3.5-9b/README.md) is the next candidate; it has one passing small-task pilot and pending full-suite/long-task evidence.

The local baseline uses Qwen3 8B Q4_K_M on Ollama 0.32.1, through `/v1/chat/completions` and Cyber's **OpenAI-compatible** adapter. It does not use a native Ollama adapter or a hosted model. See `metadata.json` for hardware, model identity and decoding settings.

Status: the original full suites were stopped after 30 completed trials, all failed, when review found that temperature 0 conflicts with Qwen3's decoding guidance. The suite directories preserve every completed outcome and an explicit `run-status.json`; they are not complete baselines or accepted P0 evidence. Successful structured tool calls establish adapter connectivity, not coding quality. The quality and long-task gates remain open.

Create the 16,384-token context alias (it shares the downloaded weights):

```sh
ollama pull qwen3:8b
ollama create cyber-qwen3:8b-16k -f eval/local-model/qwen3-8b.Modelfile
mkdir -p /tmp/cyber-local-eval/config
cp eval/local-model/cyber.json /tmp/cyber-local-eval/config/cyber.json
cargo build --locked --release -p cyber-cli
CYBER_HOME=/tmp/cyber-local-eval CYBER_OFFLINE=1 target/release/cyber eval run \
  eval/manifests/suite-coding-v1.json --model local/qwen3:8b --trials 3 --jobs 2 \
  --output eval/results/suite-coding-v1/local-qwen3-8b
CYBER_HOME=/tmp/cyber-local-eval CYBER_OFFLINE=1 target/release/cyber eval run \
  eval/manifests/suite-coding-hard-v1.json --model local/qwen3:8b --trials 3 --jobs 2 \
  --output eval/results/suite-coding-hard-v1/local-qwen3-8b
```

[Ollama's OpenAI compatibility documentation](https://docs.ollama.com/api/openai-compatibility) requires a Modelfile to set local context size. The Cyber config uses that alias as the API model ID and labels the model `local/qwen3:8b`; `reasoning_effort: none` disables thinking for this baseline. Temperature and seed are fixed, but these are live runs, not deterministic fixtures. Model costs are unpriced; zero dollars is not a hardware or electricity cost estimate.

A preliminary Qwen2.5-Coder 7B smoke test emitted the proposed `read` call as plain assistant text, rather than a structured tool call. The slugify grader failed with zero tool executions. Qwen3's smoke test executed two structured calls but failed the grader, so subsequent suite failures must remain visible. No fallback interprets assistant text as executable tool calls. Only the two newly downloaded Qwen2.5-Coder names were removed when replacing the candidate to recover disk space; pre-existing models were retained.

A separate thinking pilot on `fix-slugify` also failed: one Turn, zero tool calls, 4,096 output tokens and 233.60 seconds. It used the same model/context alias but omitted `reasoning_effort: none` and set compaction buffer/keep tokens to 4,096 each. This pilot is not merged into the non-thinking suite results.

The [Qwen3 model card](https://huggingface.co/Qwen/Qwen3-8B) recommends non-thinking temperature 0.7/top-p 0.8 and warns against greedy decoding. A separately labeled pilot with those settings and a 4,096-token compaction buffer/keep budget also failed the slugify grader (one Turn, zero tool calls). A subsequent diagnostic using the same settings performed real `read` and `edit` calls, introduced an incorrect ASCII check, then ended before completing the requested fix or running tests. Its transcript and both pilot reports/configurations are retained in `pilots/`. These outcomes do not establish the cause of every original failure or rule out a better configuration.

The 20,000-token default compaction buffer exceeds this model's configured 16,384-token context. The existing threshold consequently requests compaction whenever there is older history to summarize. The sampled pilot's smaller buffer avoids this configuration mismatch; it is not a change to the production compaction contract.
