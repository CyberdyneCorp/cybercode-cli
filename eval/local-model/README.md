# Local compatible-adapter baseline

The local baseline uses Qwen3 8B Q4_K_M on Ollama 0.32.1, through `/v1/chat/completions` and Cyber's **OpenAI-compatible** adapter. It does not use a native Ollama adapter or a hosted model. See `metadata.json` for hardware, model identity and decoding settings.

Status: the full suites are in progress, not accepted P0 evidence. Individual trial files appear before the final report; do not interpret an incomplete directory as a complete baseline. Successful structured tool calls establish adapter connectivity, not coding quality. The published quality and long-task gates remain open until the complete reports are reviewed.

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
