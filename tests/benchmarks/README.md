# Shared-Instance Benchmarks

This directory contains manual benchmark workflows for shared-instance IronClaw performance work, starting with issue `#1775`.

## Scope

The initial benchmark is intentionally narrow:

- one IronClaw instance
- one machine
- multiple authenticated users
- multiple SSE connections per user
- concurrent chat requests
- mock LLM backend so inference-service variance does not dominate the results

This is not a soak-test platform, a multi-process benchmark suite, or a CI load job.

## Prerequisites

- Rust toolchain
- Python 3.11+
- Python dependencies used by the existing E2E harness:

```bash
cd tests/e2e
pip install -e .
```

The benchmark driver reuses `aiohttp` and `httpx`, plus the existing mock LLM server in [tests/e2e/mock_llm.py](../e2e/mock_llm.py).

## Baseline Run

From the repo root:

```bash
python tests/benchmarks/shared_instance_benchmark.py \
  --label baseline \
  --user-count 20 \
  --sse-connections-per-user 2 \
  --senders-per-user 1 \
  --messages-per-user 5 \
  --max-in-flight-requests 20
```

The script will:

1. build `ironclaw` with the benchmark runtime feature if needed
2. start the mock LLM server
3. start one IronClaw gateway instance with libSQL
4. create benchmark users through `POST /api/admin/users`
5. pre-create one thread per measured request
6. open `user_count * sse_connections_per_user` SSE streams
7. run the measured chat workload against `POST /api/chat/send`

Results are written to the system temp directory under `ironclaw-benchmarks/` by default.

## Variant Runs

Current-thread Tokio runtime:

```bash
python tests/benchmarks/shared_instance_benchmark.py \
  --label runtime-current-thread \
  --runtime-mode current_thread
```

Lower global parallel jobs:

```bash
python tests/benchmarks/shared_instance_benchmark.py \
  --label lower-agent-jobs \
  --agent-max-parallel-jobs 8
```

Tighter per-user LLM concurrency:

```bash
python tests/benchmarks/shared_instance_benchmark.py \
  --label tighter-tenant-llm \
  --tenant-max-llm-concurrent 2
```

Tighter per-user job concurrency:

```bash
python tests/benchmarks/shared_instance_benchmark.py \
  --label tighter-tenant-jobs \
  --tenant-max-jobs-concurrent 1
```

## Workload Model

- one or more sender tasks per user
- one unique thread per measured request
- additional SSE streams for the same user act as passive listeners
- the benchmark records whether every open stream for that user observed any event and a final event for the request

Using unique thread IDs per measured request avoids ambiguity from trailing events on reused threads while keeping the workload on the real gateway path.

To exercise per-tenant concurrency directly, increase `--senders-per-user`. For example:

```bash
python tests/benchmarks/shared_instance_benchmark.py \
  --label tenant-llm-2senders \
  --user-count 20 \
  --sse-connections-per-user 2 \
  --senders-per-user 2 \
  --messages-per-user 6 \
  --max-in-flight-requests 20 \
  --tenant-max-llm-concurrent 1
```

With `--senders-per-user 1`, requests overlap across users but not within a single user. With `--senders-per-user > 1`, requests can overlap within the same tenant and the per-user concurrency knobs become meaningful.

## Reported Metrics

Each JSON result file includes:

- requests completed
- error count
- timeout count
- p50/p95/p99 `time_to_first_event_ms`
- p50/p95/p99 `time_to_final_event_ms`
- SSE delivery rates across all expected per-user connections
- CPU samples for the IronClaw process
- RSS samples for the IronClaw process

## Notes

- The gateway currently limits total SSE plus WebSocket connections to `100`. Keep `user_count * sse_connections_per_user <= 100`.
- The default workload does not use tools. If you add a tool-inclusive variant later, pair it with `--auto-approve-tools` so the run does not stall on approval UI.
- This benchmark targets IronClaw shared-instance behavior, not provider throughput. It intentionally routes all LLM calls to the local mock server.
