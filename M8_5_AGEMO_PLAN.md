# M8.5 Implementation Plan — `agemo` CLI runner

**Date:** 2026-05-27
**Repo:** `motosan-dev/agemo` (to be created)
**Parent milestone:** [IMPLEMENTATION_PLAN.md §M8.5](IMPLEMENTATION_PLAN.md)
**Estimated effort:** 2-3 days (~16h)
**Status:** Awaiting approval

---

## 1. Purpose

Without a runner, M9's "buy 10 shares of AAPL" demo cannot execute and the contract layer cannot be validated end-to-end. `agemo` is the smallest possible binary that:

1. Takes a prompt on stdin or via `--prompt`.
2. Loads a configured `Harness` and an LLM provider.
3. Drives the `motosan-agent-loop` engine to completion.
4. Streams `AgentEvent` as JSONL on stdout.
5. Exits gracefully on Ctrl-C.

`agemo` is **scaffolding**, not a product. A real CLI / TUI is out of scope until after 1.0.0. Treat any feature creep as a defect.

## 2. Scope

### IN
- Single binary crate, ≤300 LOC of `src/*.rs` (excluding tests + Cargo.toml).
- Anthropic provider only (`motosan-ai` with `anthropic` feature).
- Compile-time harness selection via `--harness <name>` enum.
- JSONL stream on stdout, diagnostics on stderr.
- Ctrl-C → CancellationToken → graceful shutdown with hard 5s timeout.
- Single-turn execution (one prompt → run loop → exit).

### OUT — deferred to post-1.0
- TUI / pretty rendering.
- Multi-turn / interactive REPL.
- Config files (TOML, YAML, anything).
- Plugin-loaded harnesses (`dlopen`, `wasmtime`, registry lookup).
- Multi-provider support (`--provider openai|local|…`).
- Resume / session persistence (`AgentSession` integration).
- Output formats other than JSONL (no human-friendly mode).

### Out — never belongs here
- Any business logic (lives in harnesses).
- Any tool implementations (lives in `motosan-agent-tool` or harness crates).

## 3. Repo + crate layout

```
agemo/
├── .github/
│   └── workflows/
│       └── ci.yml                # cargo check + cargo test on push
├── .gitignore
├── Cargo.toml
├── Cargo.lock
├── LICENSE                       # Apache-2.0 (matches primitives + harness)
├── README.md                     # 1-page: install, run, example output
├── CHANGELOG.md
└── src/
    ├── main.rs                   # entry point: parse args, dispatch
    ├── cli.rs                    # clap derive structs
    ├── harness_registry.rs       # compile-time {null, echo_add, …} selection
    ├── provider.rs               # anthropic client construction
    └── jsonl.rs                  # AgentEvent stream serializer
```

Single binary, no library target. ≤300 LOC across the 5 src files.

## 4. Shared design decisions

### D-CLI-1. Single binary crate, no `lib`

No consumers, no testability requirement beyond what `cargo test --bin` provides. Splitting into lib + bin is wasted ceremony for ≤300 LOC.

### D-CLI-2. `clap` derive for argument parsing

Standard, low boilerplate, `--help` is free. Alternative (`argh`) is smaller but the ergonomics gap is not worth retraining muscle memory.

```rust
#[derive(clap::Parser)]
#[command(name = "agemo", version, about = "Minimal agent runner.")]
struct Cli {
    /// Harness to load. Use `--list-harnesses` to see options.
    #[arg(long, value_enum, default_value_t = HarnessKind::Null)]
    harness: HarnessKind,

    /// Prompt to send. If omitted, reads from stdin.
    #[arg(long)]
    prompt: Option<String>,

    /// Anthropic model id.
    #[arg(long, default_value = "claude-sonnet-4-6")]
    model: String,

    /// Anthropic API key. Falls back to env ANTHROPIC_API_KEY.
    #[arg(long, env = "ANTHROPIC_API_KEY")]
    api_key: String,

    /// Max engine iterations (safety cap).
    #[arg(long, default_value_t = 20)]
    max_iterations: usize,

    /// Hard shutdown timeout after Ctrl-C, in seconds.
    #[arg(long, default_value_t = 5)]
    shutdown_timeout_secs: u64,

    /// List available compile-time harnesses and exit.
    #[arg(long)]
    list_harnesses: bool,
}
```

### D-CLI-3. Compile-time harness selection

Harness loading via `dlopen` / wasm / config is out of scope. Available harnesses are a `#[derive(clap::ValueEnum)]`:

```rust
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum HarnessKind {
    /// No tools, no hooks. Tests engine + provider plumbing only.
    Null,
    /// Two stub tools `demo.echo` + `demo.add`. Tests tool dispatch.
    EchoAdd,
}
```

**⚠️ Sourcing the harness implementations.** `NullHarness` and `TwoToolHarness` currently live in `motosan-agent-harness/examples/*.rs`, which are NOT part of the library's public surface — example crates are only compiled when running `cargo run --example`, not when another crate depends on `motosan-agent-harness`. Two ways to consume them:

- **(a) Copy into agemo** — paste the ~80 LOC of both example bodies into `src/harness_registry.rs`. Fast, self-contained, fits inside the 300 LOC budget. Loses the "single source of truth" property.
- **(b) Bump `motosan-agent-harness` to 0.1.2** — expose `pub mod examples` from `src/lib.rs` so `motosan_agent_harness::examples::NullHarness` becomes a real import path. ~10 LOC change, requires a coordinated release.

**Recommendation: (a)** for M8.5. The example bodies are 13 + 80 LOC; reproducing them in agemo is cheaper than coordinating a harness bump for a scaffolding consumer. If M9's `FinanceHarness` reuses the same pattern, that's the trigger for (b).

Adding a harness later = one variant + a `match` arm in `harness_registry.rs`. M9's `FinanceHarness` will live in its own crate and be consumed via a normal `path` dep.

### D-CLI-4. Anthropic provider only

motosan-ai's backend is the only one wired up post-M8. Multi-provider can come back when there's a second.

```rust
use motosan_ai::{Client, ClientBuilder};
use motosan_agent_loop::MotosanAiClient;  // the LlmClient adapter

fn build_provider(cli: &Cli) -> Arc<dyn LlmClient> {
    let inner: Client = ClientBuilder::new()
        .api_key(&cli.api_key)
        .model(&cli.model)
        .build()
        .expect("client construction");  // verified non-empty at CLI parse
    Arc::new(MotosanAiClient::new(inner).with_max_tokens(2048))
}
```

The adapter `MotosanAiClient` lives in `motosan-agent-loop` (verified at `src/motosan_ai_impl.rs:278`) and gates on the `motosan-ai` feature, which is already enabled in our Cargo.toml. Verify the exact `ClientBuilder` API during Phase 5 — the sketch above may need adjustment.

If the API key is empty after env-var fallback, fail at parse time with a clear error (not at first API call).

### D-CLI-5. JSONL on stdout, diagnostics on stderr

Each line is a serialized `AgentEvent` per the primitives wire format. `AgentEvent` already serializes with `#[serde(tag = "event", rename_all = "snake_case")]` so consumers can match on the string tag without knowing every variant. Trailing `\n` per line, flush after every write.

stderr carries:
- argv-parse errors
- provider construction errors (bad API key)
- engine setup errors (harness collision)
- shutdown notices ("received SIGINT, shutting down…")

stdout NEVER carries human-readable text. A downstream consumer doing `agemo … | jq` must always see valid JSONL.

### D-CLI-6. Single-turn execution

One prompt → run loop → exit. Multi-turn interactive REPL is out of scope. If the user wants a follow-up, they pipe it in:

```bash
echo "what files are in /tmp?" | agemo --harness echo-add
```

Multi-turn comes back if/when `AgentSession` work in motosan-agent-loop stabilizes.

### D-CLI-7. Ctrl-C → graceful CancellationToken shutdown

```rust
let cancel = CancellationToken::new();
let shutdown = cancel.clone();
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.ok();
    eprintln!("agemo: received SIGINT, cancelling…");
    shutdown.cancel();
});

let result = tokio::time::timeout(
    Duration::from_secs(cli.shutdown_timeout_secs),
    engine.run_with_cancel(cancel.clone(), …),
).await;
```

If the engine doesn't honour cancellation within the timeout, exit with code 130 (SIGINT convention) and emit a final `AgentEvent::AgentStop { reason: StopReason::Cancelled }` if possible. The acceptance criterion is: `pkill -INT agemo` leaves no zombie tokio tasks.

### D-CLI-8. Path deps to all motosan-* crates

```toml
[dependencies]
motosan-agent-primitives = { path = "../motosan-agent-primitives" }
motosan-agent-tool       = { path = "../motosan-agent-tool" }
motosan-agent-loop       = { path = "../motosan-agent-loop", features = ["cancellation"] }
motosan-agent-harness    = { path = "../motosan-agent-harness" }
motosan-ai               = { path = "../motosan-ai/sdks/rust", features = ["anthropic"] }
tokio                    = { version = "1", features = ["rt-multi-thread", "macros", "signal", "time"] }
tokio-util               = { version = "0.7", features = ["sync"] }
clap                     = { version = "4", features = ["derive", "env"] }
serde_json               = "1"
anyhow                   = "1"
```

No crates.io publishing until post-1.0 (per IMPLEMENTATION_PLAN.md §M11).

### D-CLI-9. No config file system

The parent plan explicitly says "no config file system — JSONL only". This decision is non-negotiable for M8.5. Every config knob is either a CLI flag or an env var consumed by clap.

### D-CLI-10. JSONL schema = `AgentEvent` wire format, verbatim

Don't invent a new schema. The whole point of primitives M5 was to make `AgentEvent` the streamable contract. Emit it as-is. The 10 variants currently defined in `primitives/src/event.rs` (verified 2026-05-27) are: `AgentStart`, `AgentStop`, `MessageStart`, `MessageDelta`, `MessageEnd`, `ToolCallStart`, `ToolCallEnd`, `SubagentResult`, `AskUser`, `Error`. All serialize to snake_case via the enum's `#[serde(tag = "event", rename_all = "snake_case")]`.

```jsonl
{"event":"agent_start","session_id":"…","harness":"echo-add"}
{"event":"message_start","session_id":"…","message_id":"…","role":"assistant"}
{"event":"message_delta","session_id":"…","text":"Sure, I can…"}
{"event":"tool_call_start","session_id":"…","call":{"id":"c1","name":"demo.echo","input":{"message":"hi"}}}
{"event":"tool_call_end","session_id":"…","result":{"tool_use_id":"c1","content":[{"type":"text","text":"hi"}],"is_error":false}}
{"event":"message_end","session_id":"…","message_id":"…"}
{"event":"agent_stop","session_id":"…","reason":"completed"}
```

**Note:** there is no separate `tool_result` event — tool outcomes are encoded in `tool_call_end`. Acceptance gates and tests must use the real variant names.

If a downstream consumer needs metadata `AgentEvent` doesn't carry (e.g. token counts), the fix is to add a variant to primitives — not to wrap events in an agemo-specific envelope.

## 5. CLI surface (UX)

```bash
# Smallest valid invocation
echo "say hi" | ANTHROPIC_API_KEY=sk-… agemo --harness null

# With explicit prompt
agemo --harness echo-add --prompt "echo the word potato then add 2 and 3"

# Inspect available harnesses
agemo --list-harnesses

# Pipe into jq
agemo --harness null --prompt "ping" | jq -r 'select(.event == "message_delta") | .text'
```

Help output (the goal):

```
agemo 0.1.0
Minimal agent runner. Streams AgentEvent as JSONL on stdout.

USAGE:
    agemo [OPTIONS]

OPTIONS:
        --harness <HARNESS>                  [default: null] [possible values: null, echo-add]
        --prompt <PROMPT>                    If omitted, reads from stdin
        --model <MODEL>                      [default: claude-sonnet-4-6]
        --api-key <API_KEY>                  [env: ANTHROPIC_API_KEY]
        --max-iterations <MAX_ITERATIONS>    [default: 20]
        --shutdown-timeout-secs <SECS>       [default: 5]
        --list-harnesses
    -h, --help                               Print help
    -V, --version                            Print version
```

## 6. Implementation phases

### Phase 1 — Repo scaffolding (1h)

- `gh repo create motosan-dev/agemo --public --description "Minimal agent runner for the Motosan framework"`
- `cargo init --bin --name agemo`
- Drop in `.gitignore` (target/, Cargo.lock kept for binaries), LICENSE (Apache-2.0), README skeleton.
- Add `.github/workflows/ci.yml` (cargo check + cargo test on push).
- Push initial empty commit so CI is wired before the first real PR.

**Gate:** `cargo check` passes on empty `main.rs`.

### Phase 2 — Cargo.toml + deps (0.5h)

Per D-CLI-8. Verify all path deps resolve and that the dep graph compiles.

**Gate:** `cargo check` passes with all deps declared, even though `main.rs` is still `fn main() {}`.

### Phase 3 — CLI parsing + diagnostics (1.5h)

- Implement `src/cli.rs` per D-CLI-2.
- `--list-harnesses` prints harness names + descriptions to stdout (this is the ONE exception to D-CLI-5: `--list-harnesses` is itself JSONL-emitting? Decision: yes, emit one JSON object per harness so the listing is grep-able the same as everything else).
- Missing `--api-key` AND missing `ANTHROPIC_API_KEY` → exit 2 with clear stderr message.
- Empty prompt (no `--prompt`, empty stdin) → exit 2.

**Gate:** `agemo --help`, `agemo --list-harnesses`, and the two error paths all work without touching the network.

### Phase 4 — Harness registry (1h)

`src/harness_registry.rs`:

```rust
pub fn build(kind: HarnessKind) -> Arc<dyn Harness> {
    match kind {
        HarnessKind::Null => Arc::new(motosan_agent_harness::examples::NullHarness),
        HarnessKind::EchoAdd => Arc::new(motosan_agent_harness::examples::TwoToolHarness::new()),
    }
}

pub fn describe(kind: HarnessKind) -> &'static str {
    match kind { … }
}
```

**Gate:** `agemo --list-harnesses` enumerates both, `agemo --harness null --prompt "ping"` constructs the harness and exits cleanly even if the LLM call hasn't been wired yet (replace with a stub provider for this phase).

### Phase 5 — Provider construction (1h)

`src/provider.rs` per D-CLI-4. Wire `motosan-ai`'s Anthropic client into a `LlmClient` impl that `motosan-agent-loop` consumes.

**Gate:** With a real `ANTHROPIC_API_KEY`, `agemo --harness null --prompt "say hi in one word"` makes ONE API call and emits an `AgentEvent::MessageDelta` line.

### Phase 6 — Engine + JSONL stream (3h)

`src/main.rs` wires everything together. The actual loop API (verified at `motosan-agent-loop/src/core/engine.rs:4381`) is:

```rust
let cli = Cli::parse();
let harness = harness_registry::build(cli.harness);
let provider = provider::build(&cli);
let cancel = setup_signal_handler();

// Builder is Engine::builder(), not EngineBuilder::new().
let engine: Arc<Engine> = Engine::builder()
    .harness(harness)
    .max_iterations(cli.max_iterations)
    .build()?;

// run() returns a RunBuilder; .stream() terminates it into an event stream.
let mut stream = engine.run(provider, vec![Message::user(prompt)])
    .with_cancel(cancel.clone())   // feature = "cancellation"
    .stream();                     // → impl Stream<Item = AgentEvent>

let mut stdout = io::BufWriter::new(io::stdout().lock());
while let Some(event) = stream.next().await {
    serde_json::to_writer(&mut stdout, &event)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
}
```

Verify the exact `RunBuilder` terminator method name during Phase 6 — the loop crate has both `pub fn stream(...)` at line 4106 and other terminators; pick the one that returns a `Stream<Item = AgentEvent>` directly without consuming via channels (or consume the channel if that's the only path — adjust the sketch accordingly).

`src/jsonl.rs` exists only if serialization needs a helper; if `serde_json::to_writer` does the job, delete the file.

**Gate:** A full single-turn run with `echo-add` produces a JSONL stream containing at minimum `agent_start`, `message_delta`, `tool_call_start`, `tool_call_end`, `agent_stop` events. Each line is valid JSON.

### Phase 7 — Cancellation (1.5h)

Per D-CLI-7. Implement the SIGINT handler, wire the timeout, verify with:

```bash
agemo --harness echo-add --prompt "echo something long" &
sleep 0.5
kill -INT $!
wait $!
echo "exit: $?"   # should be 130
```

Test (skipped on Windows by `#[cfg(unix)]`): use `tokio::process::Command` to spawn `agemo` as a subprocess and SIGINT it.

**Gate:** SIGINT mid-run produces a final `agent_stop` event with `reason: cancelled` and exits within `--shutdown-timeout-secs`.

### Phase 8 — README + CHANGELOG (1h)

README sections:
- One-paragraph "what is this"
- Install: `cargo install --path ../agemo` or `cargo run --`
- Quick start: `echo "ping" | agemo --harness null`
- Example JSONL output (pasted from a real run)
- Harness extension guide: 1 paragraph + `match` arm example
- Link to IMPLEMENTATION_PLAN.md M8.5 and M9.

CHANGELOG: 0.1.0 entry listing what's in, what's deferred.

**Gate:** A reader new to the framework can clone the repo, follow the README, and get a JSONL line printed within 5 minutes.

### Phase 9 — Integration test (2h)

`tests/smoke.rs`:

```rust
#[test]
fn null_harness_with_stub_provider_emits_complete_event_stream() {
    // …spawn agemo as a subprocess with a stub provider env var that
    // bypasses network…
    // …assert stdout starts with agent_start and ends with agent_stop…
}

#[test]
#[cfg(unix)]
fn sigint_produces_cancelled_stop_event() { … }
```

A stub provider is needed because hitting the real Anthropic API in CI is too expensive / flaky. Cheapest option: an env var `AGEMO_STUB_PROVIDER=1` that swaps `provider::build` for a deterministic in-memory `LlmClient` that returns a canned response. This adds ~30 LOC; worth it for testability.

**Gate:** `cargo test` green; CI green.

### Phase 10 — Commit + push (0.5h)

One commit per phase is OK, or squash before push. Commit message convention: `feat: agemo 0.1.0 — minimal JSONL agent runner` for the final.

**Gate:** Repo pushed, CI green, `gh repo view motosan-dev/agemo` shows the README rendered.

## 7. Acceptance gates (final)

All must pass before declaring M8.5 done:

1. `cargo build` clean
2. `cargo test` green (including the SIGINT subprocess test on Unix)
3. `agemo --help` works without env vars set
4. `agemo --list-harnesses` emits valid JSONL
5. `echo "ping" | agemo --harness null` against a real Anthropic key produces a non-empty JSONL stream ending with `agent_stop`
6. `agemo --harness echo-add --prompt "echo X then add 2+3"` produces a JSONL stream containing both a `tool_call_start` AND a matching `tool_call_end` event for each invocation
7. SIGINT mid-run exits within `--shutdown-timeout-secs` with code 130 and a final `agent_stop:cancelled` event
8. Total `src/*.rs` LOC ≤ 300 (`tokei src/`)
9. README quick-start is reproducible from a clean clone in <5 min
10. CI green on GitHub Actions

## 8. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| `AgentEvent` doesn't carry everything a JSONL consumer needs (token counts, latency) | Medium | Add primitives variants when needed — don't wrap events in an agemo envelope (D-CLI-10) |
| motosan-agent-loop's `cancellation` feature has gaps that prevent clean SIGINT shutdown | Medium | Phase 7 acceptance gate flushes this out. If it fails, file a bug against loop and either fix there or relax shutdown_timeout default |
| Anthropic API costs balloon during dev (live tests) | Low | Use stub provider for tests (Phase 9). Only the manual acceptance gates 5+6 hit real API |
| `--list-harnesses` JSONL convention is annoying for humans | Low | Tradeoff is consistency. If it turns out painful in practice, add an explicit `--human` flag in a follow-up — don't add it pre-emptively |
| 300 LOC budget gets blown by clap boilerplate | Medium | If approaching 300, audit for accidental scope creep first; raising the budget is a last resort and signals a missing decision somewhere |
| The harness/loop integration surface changes during M9 and breaks agemo | Medium | Acceptable — agemo is scaffolding. Plan to rewrite ~50 LOC during M9 if needed |
| First-time `gh repo create` for `motosan-dev/agemo` fails due to org permissions | Low | Wade already created primitives + harness under motosan-dev; same path |

## 9. Open questions (resolve before Phase 1)

1. **`--list-harnesses` output: JSONL or plain text?**
   - Plan says JSONL (D-CLI-5 consistency).
   - Argument for plain text: it's a help-style command, humans run it.
   - **Recommendation:** JSONL. One object per harness with `{"name": "…", "description": "…"}`. `agemo --list-harnesses | jq -r '.name'` then works the same way as filtering an event stream.

2. **Stub provider mechanism: env var or build feature?**
   - Env var (`AGEMO_STUB_PROVIDER=1`) keeps the test binary identical to production.
   - Build feature (`--features stub-provider`) keeps the stub code out of production binaries.
   - **Recommendation:** env var. ~30 LOC of stub code in a production binary is acceptable for scaffolding; the test simplicity gain is worth it.

3. **Should `agemo` re-export anything as a library?**
   - Current plan: no — pure binary.
   - Counter: if M9's finance harness wants to embed agemo's JSONL-emitter loop, having `agemo::jsonl::Stream` would help.
   - **Recommendation:** stay binary-only until a second consumer materializes. M9 should drive the loop directly via motosan-agent-loop, not via agemo.

4. **Where does the harness's `Provider` come from?**
   - The current sketch has agemo construct the Anthropic provider and pass it to the engine.
   - Alternative: harnesses declare their preferred provider via a new trait method.
   - **Recommendation:** keep provider in agemo. Harnesses should be provider-agnostic — that's the whole point of `LlmClient` being a trait. Wiring provider into Harness creates coupling we'll regret.

## 10. What this plan does NOT cover

- Multi-turn / interactive REPL (post-1.0)
- TUI / pretty rendering (post-1.0)
- Plugin-loaded harnesses (post-1.0)
- Multi-provider support (when a second provider exists)
- M9 finance harness consumer (separate plan)
- Performance tuning (premature; ≤300 LOC binary has nothing to tune)
- Distribution (no `cargo install` instructions until the deps are on crates.io)

## 11. Estimated effort

| Phase | Hours |
|---|---|
| 1. Repo scaffolding | 1 |
| 2. Cargo.toml + deps | 0.5 |
| 3. CLI parsing | 1.5 |
| 4. Harness registry | 1 |
| 5. Provider | 1 |
| 6. Engine + JSONL stream | 3 |
| 7. Cancellation | 1.5 |
| 8. README + CHANGELOG | 1 |
| 9. Integration test | 2 |
| 10. Commit + push | 0.5 |
| **Total** | **13** |

Plus 2-3h of unknown-unknowns. Calendar: 2-3 days of focused work, matching the parent plan estimate.
