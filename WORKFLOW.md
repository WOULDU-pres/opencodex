# WORKFLOW.md

Fixed execution protocol. In this repository, Codex normalizes every request into the format below before taking action.

## 0) Mandatory Preflight

Before starting each new request, read both files:

- `AGENT.md`
- `WORKFLOW.md`

If either file changes during execution, re-read both immediately before continuing.

In the first progress update, include one line confirming preflight completion.

## 1) Input Normalization Rules

Convert user input into the execution prompt format below.
User input may be free-form and in any language, but internal canonical keys must stay in English.

```text
[mode] Research | Implementation | Research+Implementation
[goal] ...
[constraints] ...
[deliverables] ...
[verification] ...
```

If any section is missing, apply defaults.

- Default `[mode]`: `Research`
- Default `[deliverables]` in Research mode: `3 proposals (evidence/pros/cons/risks/estimated cost/recommended option)`
- Default `[verification]`: `explicit evidence, reproducible verification steps, test/check result reporting`
- Default `[constraints]`: `no regression of existing behavior, minimal changes, no expansion beyond requested scope`

If any requirement or intent is uncertain, state the uncertainty explicitly and ask a short clarification question before execution.

## 2) Process Priority (OpenSpec First)

For non-trivial work, OpenSpec takes priority over generic workflow decomposition.

Treat a request as non-trivial when any condition below is true:

- It touches 3 or more files, or 2 or more modules.
- It changes schema, data migration behavior, auth/authz, billing, security, or external API contracts.
- It introduces or changes external dependencies, deployment config, or runtime environment contracts.
- It requires more than one independent verification gate (for example, unit + integration, or migration + integration).
- It is expected to require coordinated multi-track execution rather than a single focused edit.

1. Start each non-trivial request with OpenSpec:
   - ideation or clarification: `/opsx-explore`
   - new scoped work: `/opsx-new` (or `/opsx-ff` when scope is clear)
   - follow-up on an existing change: `/opsx-continue`
2. Implement only from OpenSpec tasks via `/opsx-apply`.
3. Run `/opsx-verify` before declaring completion.
4. Archive completed work with `/opsx-archive` (or `/opsx-bulk-archive` for multiple completed changes).

If OpenSpec tooling is unavailable, resolve that blocker first and do not proceed with ad-hoc implementation.

## 3) Research Mode Execution Procedure

1. Decompose the problem into 4 tracks from the team-lead (orchestrator) viewpoint.
2. Break each track into 3-8 subtasks and investigate in parallel.
3. Run up to 6 concurrent child agents at a time; batch remaining subtasks.
4. Cross-review across tracks to remove overlap, conflict, and gaps.
5. Submit 3 final proposals.

Required structure for each proposal:

- Solution summary
- Evidence (data, code, documents, experiments)
- Pros and cons
- Risks and mitigations
- Estimated cost and timeline
- Recommended option and recommendation rationale

## 4) Implementation Mode Execution Procedure

1. Break the approved plan into 4 implementation tracks.
2. Implement or modify in parallel within the 6-concurrent-child-agent cap, then self-review each track.
3. Perform track review, then orchestrator integration review.
4. Run available quality gates.

Recommended quality gates:

- `lint`
- `typecheck`
- `unit test`
- `integration test`
- `e2e test`
- regression checks focused on impacted areas

Required final report structure:

- changed file list
- core logic change summary
- verification execution results
- remaining risks (if any)

## 5) Quality Declaration Rules

- The goal is defect minimization.
- Never use "zero bugs" as a guaranteed statement.
- Report confidence by listing executed verification and residual risks.

## 6) Communication Rules

- Share short and frequent status updates during execution.
- In final responses, report in this order: decision, result, evidence.
- If the user selects an approved proposal, switch immediately to implementation mode.

## 7) Fixed Execution Template

Users can issue instructions in the format below.

```text
[mode] Research
[goal] ...
[constraints] ...
[deliverables] 3 proposals (include evidence/pros/cons/risks/estimated cost/recommended option)
[verification] ...
```

Implementation instruction example:

```text
[mode] Implementation
[goal] ...
[constraints] ...
[deliverables] Implementation complete + change log + test results
[verification] lint/typecheck/unit/integration/e2e pass
```

## 8) Changelog Update Policy

- Do not append long-form changelog sections directly in `docs/CHANGELOG.md`.
- For each new changelog update, create a new file using local 24-hour time:
  - `docs/changelog/YYYY-MM-DD-HHMM.md`
- Maintain `docs/CHANGELOG.md` as an index and add links with this format:
  - `[YYYY-MM-DD HH:MM](./changelog/YYYY-MM-DD-HHMM.md)`
- Keep the latest entry at the top of the index list.
