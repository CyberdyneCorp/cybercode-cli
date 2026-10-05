# Qwen3.5 9B candidate

This candidate is under evaluation. The first slugify pilot passed the hidden grader after 12 Turns. Remaining duplicate smoke trials were stopped to reserve the serial engine for the long-task pilot and full suites; all completed outcomes are in `pilots/slugify/`, explicitly labeled incomplete. That is a task result, not passing full-suite or P0 evidence. Full suite reports require 20 tasks × 3 trials each, and the long-task result must also be reviewed.

Model and alias digests, observed GPU/context settings, decoding and compaction settings are recorded in `metadata.json`. The request uses Cyber's OpenAI-compatible adapter against local Ollama 0.32.1; no hosted inference or native Ollama adapter is used. The pilot release harness is `b99a74b`.

The pilots use that historical harness, which omits successful-compaction usage/cost and the separate reasoning-token class from evaluation totals. Full suites are held until a corrected release harness is built; their own reports identify its revision. Pilot grading outcomes remain valid, but their token totals are not interchangeable with corrected measurements.

The [Qwen3.5 model card](https://huggingface.co/Qwen/Qwen3.5-9B) lists non-thinking settings of temperature 0.7, top-p 0.8, top-k 20, min-p 0 and presence penalty 1.5. The Modelfile and request config use those settings with seed 42, a 32,768-token context and an 8,192-token output allowance. Compaction buffer and retained tail are each 8,192 tokens. Live runs remain nondeterministic; zero-dollar reported model cost is unpriced, not a measurement of electricity or hardware costs.

```sh
ollama pull qwen3.5:9b
ollama create cyber-qwen35:9b-32k -f eval/local-model/qwen3.5-9b/Modelfile
mkdir -p /tmp/cyber-local-qwen35/config
cp eval/local-model/qwen3.5-9b/cyber.json /tmp/cyber-local-qwen35/config/cyber.json
CYBER_HOME=/tmp/cyber-local-qwen35 CYBER_OFFLINE=1 target/release/cyber eval run \
  eval/manifests/suite-coding-v1.json --model local/qwen3.5:9b --trials 3 --jobs 1 \
  --output eval/results/suite-coding-v1/local-qwen35-9b
# Run after the previous suite finishes, including when it exits 1 for grader failures:
CYBER_HOME=/tmp/cyber-local-qwen35 CYBER_OFFLINE=1 target/release/cyber eval run \
  eval/manifests/suite-coding-hard-v1.json --model local/qwen3.5:9b --trials 3 --jobs 1 \
  --output eval/results/suite-coding-hard-v1/local-qwen35-9b
```

An attempted private server configured with `OLLAMA_NUM_PARALLEL=4` logged `model architecture does not currently support parallel requests` for `qwen35` and launched its runner with `-np 1`. That setup was stopped; any completed trial files remain in directories labeled `parallel-attempt`. The final full suites run sequentially with one job, after the separate smoke and long-task pilots finish. Do not describe configured parallelism as observed parallelism.
