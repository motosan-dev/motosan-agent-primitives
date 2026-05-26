# Sub-Agent Dispatch Guide

How to break the implementation plan into sub-agent tasks, with
ready-to-paste prompts for each phase.

The full plan lives at:
`~/Projects/wade/motosan-agent-primitives/IMPLEMENTATION_PLAN.md`

---

## Dispatch strategy

| Phase | Milestones | Repos touched | Sub-agent? |
|-------|-----------|---------------|------------|
| **A** | M0-M6 | `motosan-agent-primitives` only | ✅ One sub-agent, sequential |
| **B** | M7 | `motosan-agent-harness` (new) | ✅ One sub-agent |
| **C** | M8 Step 1 (audit) | reads many | ✅ One sub-agent (read-only) |
| **C** | M8 Step 2 (refactor) | `motosan-agent-loop`, `motosan-ai`, `motosan-agent-tool`, more | ❌ **Human-led** — too risky |
| **D** | M8.5 | `motosan-agent-cli` (new) | ✅ One sub-agent |
| **E** | M9 gate | sequence diagram only | ⚠️ Optional — you may want to do this yourself |
| **E** | M9 implementation | `motosan-agent-harness-finance` (new) | ✅ One sub-agent |
| **F** | M10-M11 | mixed | Case by case |

### Why M8 Step 2 must be human-led

It touches 3-5+ existing repos with downstream consumers. One bad rename
breaks everything. The audit (Step 1) is safe to delegate; the actual
refactor needs your eye.

### Parallelism

Phases A and B can run **in parallel** — they touch different repos. Phase
C Step 1 can also run in parallel with A and B (it's read-only).

After all three finish, Phase C Step 2 runs human-led, then D/E sequentially.

---

## Prompts

### Phase A — Build motosan-agent-primitives (M0-M6)

Paste this entire block to a fresh sub-agent. It expects access to
`~/Projects/wade/motosan-agent-primitives/`.

````
You are implementing the motosan-agent-primitives Rust crate — the contract
layer of the Motosan agent framework. Your scope is **milestones M0 through M6
only** (the crate itself, not the harness crate or downstream wiring).

## Required reading BEFORE writing any code

Read these files in order:

1. `~/Projects/wade/motosan-agent-primitives/IMPLEMENTATION_PLAN.md`
   — entire file. Pay special attention to:
   - Section 2 (Scope) — what's in / out
   - Section 4 (Design decisions) — 12 answered decisions including
     three flipped after research (D1, D2, D5)
   - Section 5 milestones M0-M6 — your task
   - Appendix A — the existing `src/*.rs` files are wrong on multiple
     counts and should be deleted before you start

2. `~/Projects/wade/motosan-agent-primitives/README.md` — context

3. (Optional, for style reference) `~/Projects/wade/codex/codex-rs/Cargo.toml`
   and `codex-rs/core/src/tools/context.rs` — Codex is the design reference
   for ToolContext + CancellationToken pattern (D5).

## Your task

Execute milestones M0 through M6 from the plan, in order. For each milestone:

1. Read its "Acceptance" criteria from the plan.
2. Implement until acceptance is met.
3. Run `cargo check` and `cargo test` after each milestone.
4. Commit with message `M{N}: {milestone title}` (do NOT push).

## Hard rules (do not violate)

- **Delete the existing `src/*.rs` first** (`rm -rf src/*.rs`). They predate
  the answered decisions and are wrong. Recreate from scratch per the plan.
- **D1 = B**: `Tool` trait does NOT live in this crate. Only `ToolCall`,
  `ToolResult`, `ToolAnnotations` (data types) under `src/tool.rs`.
- **D2 = B (flipped)**: Hook uses return-by-value
  `HookResult::Continue { updated_input: Option<Value> }`,
  NOT `&mut Ctx`. Never write `ctx.field = new` style mutation.
- **D5 = A (flipped)**: `tokio-util` with `sync` feature is a dependency.
  Hook context structs each carry `cancellation_token: CancellationToken`.
- **9 hook events** (not 8): include `post_tool_use_failure` (fires on
  tool error or cancel, separate from `post_tool_use` which fires on success).
- **Cargo.toml deps**: exactly as M0 lists — `async-trait`, `chrono`
  (`default-features = false`, features `["serde", "clock"]`), `serde`,
  `serde_json`, `thiserror`, `uuid` (`["v4", "serde"]`), `tokio-util`
  (`["sync"]`). No others without asking first.
- **Apache-2.0 LICENSE** file at repo root.
- **`#![warn(missing_docs)]`** at the top of `lib.rs`. Every public item
  needs rustdoc. Two load-bearing warnings must appear:
  - In `ToolAnnotations` rustdoc: warn about destructive-annotation
    correctness in plan mode (see plan M2).
  - In `PermissionMode::Plan` rustdoc: warn that destructive=false tools
    can hit network in plan mode (see plan M3).
- **Do NOT push to GitHub**. Commit only. Wade reviews before pushing.
- **Do NOT modify IMPLEMENTATION_PLAN.md** — it's frozen.
- **Do NOT touch other motosan-* repos** under `~/Projects/wade/`.

## When you finish

Report:

1. List of files created / modified (with LOC).
2. Acceptance criteria per milestone, each marked ✓ or ✗ with specifics.
3. Output of `cargo check --all-targets` and `cargo test`.
4. Output of `cargo doc --no-deps` (any warnings).
5. Any deviation from the plan, with rationale.
6. Suggested next phase to dispatch.

## If you get stuck

Do NOT improvise. Specifically, if any of the following happens, stop and
report to Wade for direction:

- Acceptance criterion seems impossible without changing the plan
- A design decision feels wrong now that you're implementing it
- You'd need a dep not on the M0 list
- `cargo check` fails in a way the plan doesn't address
- You can't meet `#![warn(missing_docs)]` without doc-bombing

Start by reading the plan top to bottom, then `rm -rf src/*.rs`, then M0.
````

### Phase B — Build motosan-agent-harness (M7)

Paste this to a fresh sub-agent. It needs permission to create a new
directory `~/Projects/wade/motosan-agent-harness/`.

````
You are creating the motosan-agent-harness Rust crate, which holds the
`Harness` trait — the composition contract for vertical domain bundles.
Your scope is **milestone M7 only**.

## Required reading

1. `~/Projects/wade/motosan-agent-primitives/IMPLEMENTATION_PLAN.md`
   — entire file. Pay special attention to:
   - Section 5 M7 — your task
   - Section 1 layering diagram — your crate sits ABOVE
     motosan-agent-tool and motosan-agent-primitives
   - D1 (Option B) — explains why this crate exists
   - D3 (most-restrictive-wins) — how PermissionPolicy composition works
     when multiple Harnesses stack

2. `~/Projects/wade/motosan-agent-primitives/src/lib.rs` and
   `~/Projects/wade/motosan-agent-primitives/src/harness.rs` — wait, harness.rs
   should NOT exist in primitives. If it does, that's the leftover draft
   code that Phase A should have deleted. Do not import from it.

3. `~/Projects/wade/motosan-agent-tool/` — read its `src/lib.rs` to find the
   `Tool` trait you'll reference in `Harness::tools()`.

## Your task

Create a new Rust crate at `~/Projects/wade/motosan-agent-harness/`:

- `Cargo.toml`:
  - package name `motosan-agent-harness`, version `0.1.0`, edition `2021`
  - dependencies:
    - `motosan-agent-primitives` (path = `../motosan-agent-primitives`)
    - `motosan-agent-tool` (path = `../motosan-agent-tool`)
    - Anything transitively needed (probably nothing else)
  - Apache-2.0 license, repo `motosan-dev/motosan-agent-harness`

- `src/lib.rs` with crate-level rustdoc explaining the role
- `src/harness.rs` with the `Harness` trait:

```rust
pub trait Harness: Send + Sync {
    fn name(&self) -> &str;
    fn system_prompt(&self) -> Option<String> { None }
    fn tools(&self) -> Vec<Arc<dyn motosan_agent_tool::Tool>>;
    fn hooks(&self) -> Vec<Arc<dyn motosan_agent_primitives::Hook>> { Vec::new() }
    fn permission_policy(&self) -> Option<Arc<dyn motosan_agent_primitives::PermissionPolicy>> { None }
    fn memory_schema(&self) -> Option<motosan_agent_primitives::MemorySchema> { None }
}
```

(adjust imports per actual paths)

- rustdoc must document composition rules:
  - tool name uniqueness across stacked harnesses
  - hook ordering = registration order
  - PermissionPolicy composition = most-restrictive-wins (Allow / AskUser / Deny)
- Two example impls in `examples/`:
  - `null_harness.rs` — returns no tools, no hooks
  - `two_tool_harness.rs` — returns two stub Tool impls

## Acceptance

- `cargo check` passes in the new crate
- `cargo run --example null_harness` and `cargo run --example two_tool_harness`
  both work
- README in the new repo with the layering diagram from the plan
- Apache-2.0 LICENSE

## Hard rules

- Do NOT push to GitHub. Commit only.
- Do NOT modify `motosan-agent-primitives` or `motosan-agent-tool`.
- If `motosan-agent-tool::Tool` signature is incompatible with what M7 needs,
  STOP and report — that means Phase C (M8) hasn't aligned yet.

When you finish, report files created, acceptance status, and any blockers.
````

### Phase C Step 1 — Audit motosan-agent-tool downstream (M8 Step 1)

This one is read-only and safe to delegate. Paste to a fresh sub-agent.

````
You are auditing the impact of refactoring `motosan-agent-tool` to use the
new `motosan-agent-primitives` types. This is **read-only**; do not modify
any files.

## Task

Run this audit and produce `~/Projects/wade/motosan-agent-primitives/M8_AUDIT.md`:

1. `grep -rln "motosan_agent_tool" ~/Projects/wade/motosan-*` — list every
   repo that imports motosan-agent-tool.

2. For each impacted repo, inside its Rust files:
   - Which symbols from `motosan_agent_tool` are imported? (use grep)
   - How many files use it? How many lines reference it?
   - What's the public-API surface of the repo that depends on those
     symbols? (i.e., would a downstream of THIS repo also break?)

3. Identify the most painful migration:
   - The repo with the most call sites
   - Any repo where motosan-agent-tool symbols leak into its public API
     (those have second-order blast radius)

4. Estimate effort per repo:
   - Trivial (< 1 hour): just imports, no signature changes
   - Medium (1-4 hours): some impl Tool sites that need adjustment
   - Heavy (full day+): public API exposes Tool-derived types

## Output format

Write the report to
`~/Projects/wade/motosan-agent-primitives/M8_AUDIT.md` with this structure:

```markdown
# M8 Audit — motosan-agent-tool downstream impact

Date: [today]
Auditor: sub-agent

## Summary

- Total impacted repos: N
- Trivial: X
- Medium: Y
- Heavy: Z

## Per-repo detail

### motosan-agent-loop
- Files using motosan-agent-tool: N
- Symbols imported: [list]
- Public API leak: yes / no
- Effort estimate: [bucket]
- Notes: [whatever you found]

### [other repos...]

## Recommended ordering for M8 Step 2

1. ...
2. ...

## Risks surfaced

- [anything unexpected]
```

## Hard rules

- Read-only. Do not modify any file outside `motosan-agent-primitives/M8_AUDIT.md`.
- Do not change git state in any other repo.
- If a repo has no Rust files or no actual use of motosan-agent-tool symbols,
  note that and move on.

Report when M8_AUDIT.md is written.
````

### Phase C Step 2 — Refactor (M8 Step 2) — HUMAN-LED

**Do not delegate this.** It touches 3-5+ live repos. Use the audit from
Step 1 to plan it yourself. Open a branch in each repo, do the migration
manually, test before merging.

If you really must delegate, dispatch one sub-agent **per repo**, with a
prompt that:
- References M8_AUDIT.md for the specific repo
- Says exactly which import lines to change
- Forbids any other change
- Has the user merge each repo manually after review

But honestly: just do it yourself. The audit will tell you it's 2-3 hours
of mechanical work.

### Phase D — Minimal runner (M8.5)

````
You are building motosan-agent-cli — a minimal binary that drives the agent
end-to-end for harness validation. This is **scaffolding**, not a product.
Your scope is M8.5 from the plan.

## Required reading

1. `~/Projects/wade/motosan-agent-primitives/IMPLEMENTATION_PLAN.md`
   — entire file. Focus on M8.5 acceptance and the layering diagram.

2. After M8 is done, the new shape of:
   - `motosan-agent-primitives` (types, AgentEvent)
   - `motosan-agent-tool` (Tool trait)
   - `motosan-agent-harness` (Harness trait)
   - `motosan-agent-loop` (the engine you'll invoke)
   - `motosan-ai` (provider)

## Your task

Create `~/Projects/wade/motosan-agent-cli/`:

- Single binary, ≤ 300 LOC
- Reads prompt from CLI arg or stdin
- Loads a `Harness` (start with a stub `null` harness that returns no tools)
- Loads a provider from env vars (whatever motosan-ai expects)
- Drives `motosan-agent-loop` and streams `AgentEvent` to stdout as JSONL
  (one JSON object per line)
- Ctrl-C cleanly terminates (subprocess tools must die)

## Hard rules

- NO TUI, NO config file system, NO fancy output formatting
- ≤ 300 LOC total
- Do NOT modify any other motosan-* repo
- Do NOT push to GitHub

## Acceptance

- `echo "hello" | motosan-agent-cli --harness null` produces a JSONL stream
- Ctrl-C mid-run leaves no zombie processes

Report when done.
````

### Phase E gate — M9 sequence diagram (do this yourself or with a thinking sub-agent)

This is too important to fully delegate. The whole point of the gate is
that **you** verify the Harness model handles finance before code is written.

A thinking sub-agent CAN help draft, but you must read and approve.

````
You are drafting the M9 gate sequence diagram. Do NOT write any Rust code.

## Task

Produce `~/Projects/wade/motosan-agent-harness-finance/M9_GATE_DIAGRAM.md`
(create the repo if needed) containing:

1. A 1-page sequence diagram (Mermaid or ASCII) of this user flow:
   "User: buy 10 shares of AAPL if it's under $200"
   → Agent thinks → calls get_quote → checks price → calls place_order
   with approval → done.

2. For EVERY arrow in the diagram, annotate which planned API surface
   handles it. Use only:
   - `motosan_agent_primitives::Message`, `ContentBlock`
   - `motosan_agent_primitives::AgentEvent`
   - `motosan_agent_primitives::Hook` + ctx structs
   - `motosan_agent_primitives::PermissionPolicy`, `Permission`
   - `motosan_agent_tool::Tool`
   - `motosan_agent_harness::Harness`

3. If ANY arrow requires a concept not on that list, write a section
   "GAP" at the bottom of the doc, naming the missing concept. Do NOT
   invent a new API; just flag the gap.

## Hard rules

- No Rust code yet.
- No new API names invented.
- If you find ≥1 gap, recommend STOPPING M9 implementation until primitives
  / harness API is revised to cover it.

Report when M9_GATE_DIAGRAM.md is written, with a summary of gaps (if any).
````

### Phase E implementation — M9 finance harness (after gate passes)

````
You are implementing `motosan-agent-harness-finance`, the first real vertical
adapter using motosan-agent-harness. Your scope is M9 Step 1 (after the
M9 gate diagram is approved).

## Required reading

1. `~/Projects/wade/motosan-agent-primitives/IMPLEMENTATION_PLAN.md` M9
2. `~/Projects/wade/motosan-agent-harness-finance/M9_GATE_DIAGRAM.md` —
   the design must match this diagram. If you find it doesn't match, STOP.
3. `~/Projects/wade/motosan-finance/` and
   `~/Projects/wade/motosan-hyperliquid/` — for real finance tool inspiration
   (don't depend on them yet; mock the broker calls for now).

## Your task

In `~/Projects/wade/motosan-agent-harness-finance/`:

- `Cargo.toml` depending on `motosan-agent-harness`, `motosan-agent-tool`,
  `motosan-agent-primitives`
- 3-5 finance tools (impl `Tool`):
  - `get_quote(symbol)` — mock returns `Decimal`
  - `get_position(symbol)` — mock returns held quantity
  - `place_order(symbol, qty, side)` — mock returns order id
  - (optional) `backtest`, `list_holdings`
- `FinanceApprovalPolicy` (impl `PermissionPolicy`):
  - `place_order` → `AskUser` always
  - `get_*` (read-only) → `Allow`
- `AuditLogHook` (impl `Hook`):
  - Overrides `post_tool_use` AND `post_tool_use_failure` (per D2-B + 9
    events)
  - Writes JSONL audit log to `~/.motosan/finance-audit.jsonl`
- `FinanceHarness` (impl `Harness`):
  - bundles the above
  - `system_prompt()` returns a domain persona

## Acceptance

- `cargo check` and `cargo test` pass
- Running the agent (via motosan-agent-cli) with this harness can handle:
  "buy 10 AAPL if under $200" end-to-end, including the AskUser approval
  prompt for `place_order`
- Audit log file is written after each tool call

## Awkwardness log

While implementing, keep `AWKWARDNESS_LOG.md` in this repo: every time the
primitives or harness API forced you to write something clunky, record:
- What you tried to write
- Why it didn't work
- What you had to do instead

This log drives M10.

## Hard rules

- Mock the broker for now — no real API calls
- Do NOT modify `motosan-agent-primitives` or `motosan-agent-harness` even
  if you find them awkward (log instead)
- Do NOT push to GitHub
````

---

## Anti-patterns to avoid

1. **Don't run multiple sub-agents on the same repo in parallel.** They'll
   step on each other. Sequential within a repo, parallel across repos only.

2. **Don't let sub-agent push to GitHub.** Every prompt above says "do NOT
   push". Verify they obeyed.

3. **Don't skip the M9 gate.** Whatever cost the diagram takes, it's
   1/100th of redesigning primitives after writing 2000 LOC of harness.

4. **Don't delegate M8 Step 2.** The audit (Step 1) is fine. The actual
   refactor across 3-5 repos is too risky.

5. **Don't let sub-agent modify IMPLEMENTATION_PLAN.md.** It's frozen. If a
   decision looks wrong during implementation, sub-agent should STOP and
   report, not silently fix.

6. **Don't run Phase E (M9) before M8 Step 2 is human-verified.** Sub-agent
   in Phase E assumes motosan-agent-loop, motosan-ai, motosan-agent-tool are
   all upgraded. If they're not, M9 will fail in confusing ways.

---

## How to verify each sub-agent's output

For each phase, after the sub-agent reports completion:

1. **Read its file list.** Open at least 3 random files it claims to have
   created. Verify they exist and contain real code.

2. **Run `cargo check` and `cargo test` yourself.** Don't trust the
   sub-agent's claim that they pass.

3. **Check `git status` and `git log`.** Verify nothing was pushed, the
   commits are clean, no other repo was touched.

4. **Read 1 doc file in full.** Rustdoc is where sub-agents often
   hallucinate or skip — verify the load-bearing warnings (D4 destructive,
   plan-mode trust) are actually present.

5. **Run `cargo doc --no-deps -- -D missing_docs` and look at warnings.**
   Should be zero.

6. **Verify the dependency list in Cargo.toml.** No surprise crates.

Only after these checks pass: merge to main and dispatch the next phase.

---

## Recommended order of dispatch

1. Dispatch **Phase A** (M0-M6, primitives crate). Wait for completion.
2. Verify Phase A output (see checklist above).
3. Dispatch **Phase B** (M7, harness crate) AND **Phase C Step 1** (audit)
   in parallel. They don't conflict.
4. Verify both.
5. Do **Phase C Step 2** yourself (M8 refactor) using the audit.
6. Dispatch **Phase D** (M8.5, CLI runner).
7. Verify.
8. Do **Phase E gate** yourself or with a thinking sub-agent. Approve the
   diagram.
9. Dispatch **Phase E implementation** (M9 finance harness).
10. Verify. Read AWKWARDNESS_LOG.md.
11. Iterate to M10 (refactor) and M11 (rental + publish) as separate
    sub-agent jobs.
