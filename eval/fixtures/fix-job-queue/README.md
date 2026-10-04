# job-queue

In-process job queue for Node 22 (ES modules, no dependencies): priorities, a concurrency limit,
retries with exponential backoff, cancellation, pause/resume, drain and events. Public API in
`src/index.js`. Tests: `node --test`.

## API

```js
import { JobQueue, CancelSource, CancelledError } from "./src/index.js";

const queue = new JobQueue({ concurrency: 2, retries: 3, baseDelay: 100, maxDelay: 10_000 });
const source = new CancelSource();
const result = await queue.add(async ({ attempt, token }) => fetchThing(), { priority: 5, token: source.token });
```

`new JobQueue({ concurrency = 1, retries = 0, baseDelay = 100, maxDelay = 10000, timers = realTimers })`

- `concurrency`: integer `>= 1`; `retries`: integer `>= 0` (default for every job);
  `baseDelay`, `maxDelay`: numbers `>= 0` (ms). Invalid values throw a `RangeError`.
- `timers` is `{ setTimeout(fn, ms), clearTimeout(handle) }`; the queue uses it for every
  backoff wait and nothing else, so tests can inject fake timers.

`queue.add(task, { priority = 0, retries, token, id } = {})` returns a promise for the job's
result. `task` must be a function (else `TypeError`), `priority` a finite number (else
`TypeError`), `retries` an integer `>= 0` (else `RangeError`; default: the queue's `retries`),
`token` an optional `CancelSource().token`, `id` an optional job id (default: 1, 2, 3, ...
numbering the jobs added to this queue without an `id`, in order). Invalid arguments throw
synchronously and add nothing. The task is called as `task({ attempt, token })` and may return a
value, return a promise, or throw.

Each job is reported to listeners as a job object `{ id, priority, attempt }`; the same object
is passed to every event of that job, and `attempt` is the number of the attempt that most
recently started (0 before the first start).

## Contract

**Order.** A job waiting in the queue is started when a slot is free and the queue is not
paused. The job started is always the waiting job with the highest priority; among equal
priorities, the one that entered the queue first (FIFO). A job enters the queue when it is
added and again each time its backoff delay ends (it then goes behind jobs of the same priority
already waiting).

**Concurrency.** A job holds a slot from the moment its attempt starts until that attempt's
outcome is known (the returned promise settles, or the task throws synchronously). Never more
than `concurrency` attempts hold slots at once, and while the queue is not paused, no slot stays
free while a job is waiting: a freed slot is filled immediately (before any timer runs).

**Failures and retries.** An attempt fails if the task throws synchronously or its promise
rejects; a synchronous throw is handled exactly like a rejection (the slot is released, retries
apply). After failed attempt `n`, if `n <= retries` the job waits `backoffDelay(n)` ms =
`min(maxDelay, baseDelay * 2 ** (n - 1))` (on `timers.setTimeout`, holding no slot) and then
re-enters the queue; otherwise the job fails with the error of attempt `n`. A job is therefore
attempted at most `retries + 1` times.

**Cancellation.** `source.cancel(reason = "cancelled")` cancels every job added with
`source.token`:

- a job that is waiting in the queue, or waiting for its backoff delay, ends immediately as
  cancelled: it will never start again, it no longer counts as queued or retrying, and its
  pending backoff timer is cleared with `timers.clearTimeout`;
- a job whose attempt is running is not interrupted (the task can watch `token`): if that attempt
  succeeds the job succeeds; if it fails the job ends as cancelled (no retry);
- adding a job with an already-cancelled token ends it as cancelled at once (it never starts);
- cancelling a job that has already ended does nothing.

A cancelled job's promise rejects with a `CancelledError` (`name` `"CancelledError"`, message
`"job cancelled"`, `reason` the cancel reason).

**Pause / resume.** `pause()` stops starting jobs (added jobs and jobs whose backoff ends just
wait in the queue); running attempts continue and finish normally. `resume()` starts waiting jobs
into the free slots, so that afterwards at most `concurrency` attempts are running. `pause()` and
`resume()` are idempotent.

**Drain.** `drain()` returns a promise that resolves (with `undefined`) once no job is queued,
running or waiting for a backoff delay (immediately if that is already the case). Job failures do not reject it.
While paused with jobs queued, it does not resolve.

**Stats.** `queue.stats` is `{ queued, running, retrying, paused }`: jobs waiting in the queue,
attempts holding a slot, jobs waiting for a backoff delay, and whether the queue is paused.

**Events.** `queue.on(event, listener)` (also `off`, `once`). Listeners are called synchronously:

| Event | Arguments | When |
|---|---|---|
| `start` | `(job)` | an attempt starts (after `job.attempt` is incremented, before the task is called) |
| `retry` | `(job, error, delay)` | attempt failed and will be retried after `delay` ms |
| `success` | `(job, value)` | the job succeeded |
| `failure` | `(job, error)` | the job failed for good |
| `cancel` | `(job, reason)` | the job ended as cancelled |

Every job gets exactly one of `success`, `failure`, `cancel`, as its last event, and its promise
settles after that event's listeners have run. The events of one job therefore read
`start (retry start)* (success | failure | cancel)`, or just `cancel` for a job cancelled before
it ever started (`retry` may be followed by `cancel` when it is cancelled during the backoff).
