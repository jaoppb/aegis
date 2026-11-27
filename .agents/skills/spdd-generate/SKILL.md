---
name: spdd-generate
description: "Generate Aegis code and tests from a REASONS Canvas structured prompt, executing its Operations in order under its Norms and Safeguards. Use when a canvas from spdd-reasons-canvas is confirmed and ready to implement, or to regenerate the code affected by a canvas update. Do not use without a canvas (use spdd-reasons-canvas), and do not use it to patch code that diverged from the canvas (update the canvas first with spdd-prompt-update)."
---

# SPDD Generate

Generate implementation code from a structured SPDD prompt (a REASONS Canvas). Follow its Operations sequence, Norms and Safeguards strictly, within Aegis's rules (`AGENTS.md`).

**Input**: the canvas path, e.g. `@spdd/prompt/GH-<n>-<timestamp>-[Feat]-<scope>-<description>.md`.

## Steps

1. **Get the canvas.** If no path was given, ask the user for one. Do NOT proceed without a valid canvas.

2. **Resolve the work item (REQUIRED)**
   a. Take it from the `GH-<n>-` file-name prefix (or the header line). If the canvas has none, stop: fix the canvas first with `/spdd-prompt-update`.
   b. Verify it with `gh issue view <n> --json number,title,state`. Not found → stop. Closed → warn and ask.
   c. The canvas is an ephemeral working file: never stage it, commit it or name it by path in a commit, issue or pull request (write `#<n>`). If it does not exist, stop and run `/spdd-reasons-canvas #<n>`, which regenerates it from the analysis and the code already written.

3. **Read the whole canvas** and extract its sections:

   | Section | Usage |
   |---------|-------|
   | **R** Requirements | Business context, Definition of Done, out of scope |
   | **E** Entities | Types, stereotypes, placement |
   | **A** Approach | Strategy, errors, tests |
   | **S** Structure | crates, modules, implementations, dependencies, boundaries |
   | **O** Operations | **Execute in the defined order** |
   | **N** Norms | Change-specific standards on top of `AGENTS.md` |
   | **S** Safeguards | Non-negotiable constraints, exact error messages |

4. **Load the project rules and context**: `AGENTS.md` (the baseline norms every canvas inherits), the tool configs below, and the modules named in Structure together with their closest existing patterns.
   - Root `Cargo.toml` (workspace members, `[workspace.dependencies]`, `[workspace.lints]`)
   - `arch-lint.toml` (layer scopes and `restrict-use` rules)
   - `clippy.toml` (complexity, argument and nesting thresholds)
   - `tools.toml` (labels, milestones, issue templates, workflow statuses)
   - `justfile` (the check recipes)

   Generated code MUST follow the existing conventions.

5. **Validate the Operations sequence**
   - The dependency order is right (domain value objects → domain entities → application ports → use cases → adapters → binary wiring → `arch-lint.toml` boundaries → docs → verify), and no operation uses a type defined by a later one
   - No dependency breaks the boundary rules:
     - Dependencies point inward: `domain` ← `application` ← `adapters` ← binaries (`cli`, `controller`, `agent`)
     - `domain` is pure: no IO, process, environment, async runtime, HTTP, CLI, logging or serialization wire formats
     - `application` depends only on `domain`, declares ports (traits) and does no IO
     - `adapters` implement application ports and never depend on a binary
     - Binaries only wire things together, hold no logic and never depend on each other
   - Each operation is atomic, names its crate and module, and lists its tests. Together they cover all of Structure, and they match its implementations, dependencies and boundary rules.

   If you find a problem, report it and suggest a `/spdd-prompt-update` before proceeding. Do NOT re-plan the sequence: the Operations order is the designed order.

6. **Generate the code, one operation at a time, in order**

   a. Read the operation spec: responsibility, representation, signatures, errors, behaviour and tests.

   b. **Apply the Norms** (`AGENTS.md` and the canvas Norms):
   - Newtypes with validating constructors; private fields; tell-don't-ask methods
   - Functions stay small and flat: at most 25 lines, 3 parameters and 2 levels of nesting (`clippy.toml`); use early returns and `?`
   - One error enum per layer boundary; no `unwrap`, `expect`, `panic!`, indexing or `as` conversions outside tests
   - `///` docs on every public item; no `unreachable_pub`; identifiers of at least two characters
   - No printing, `dbg!`, `todo!`, `unimplemented!` or `unreachable!` outside tests; never log secrets
   - No unchecked arithmetic, floats or string slicing: `[workspace.lints]` in `Cargo.toml` is the full list of forbidden lints, and cognitive complexity stays at or under 8 (`clippy.toml`)
   - New IO goes behind a port in `application` and an adapter in `adapters`; binaries hold no logic

   c. **Enforce the Safeguards**:
   - Use the exact error names and messages (do not modify them)
   - Apply the domain, data and security constraints exactly as written
   - **Zero lint exceptions**: never write `#[allow(...)]`, `#[expect(...)]` or any other lint suppression attribute. If code cannot fit a rule, stop and report it. The fix is the canvas, not an exemption.

   d. **Write the tests the operation lists**:
   - Behaviour tests through public use cases and ports, with in-memory fakes
   - Unit tests sit beside the code; cross-crate and architecture tests live under `tests/`
   - Tests may use `unwrap`, `expect`, `panic` and printing
   - A bug fix starts with a failing test

   e. Write the code in the crate and module the operation names.

   Do NOT deviate from the Operations, add features, methods or fields that are not specified, or change the Safeguard messages.

7. **Batch validation**

   After ALL the code is generated, run `just check` (`fmt-check`, `lint` (clippy with warnings as errors), `workflows-check`, `arch-check` and `test` (cargo nextest)).

   a. Fix failures by changing the code to fit the rule, never by adding exceptions.

   b. Cross-check every Definition of Done item, and check that the error names and messages match the Safeguards exactly.

   c. Check that placement matches Structure, that no forbidden dependency was introduced, and that `git grep -nE '#!?\[(cfg_attr\([^]]*)?(allow|expect)\(' -- 'crates/*.rs' 'tests/*.rs'` prints nothing.

   If a failure cannot be fixed without deviating from the canvas, stop and propose a `/spdd-prompt-update`.

8. **Report**: the work item and the canvas, the files created or changed and their responsibilities, any deviations or assumptions, the `just check` result, and the Definition of Done checklist (pass or fail per item).
   The pull request body names the issue with `Closes #<n>`.

## Review and iteration loop

> **When reality diverges, fix the prompt first, then update the code.**

If the code was already changed by hand (a review fix) and more operations remain, first bring the canvas in line with it (`/spdd-reasons-canvas #<n>`, stale canvas), so that generation does not undo the change.

1. Identify the incorrect or missing behaviour.
2. Trace it to a section: wrong interpretation → Requirements; missing type or relationship → Entities; flawed strategy → Approach; wrong placement → Structure; wrong detail → Operations; missing standard → Norms; missing constraint → Safeguards.
3. Update the canvas with `/spdd-prompt-update`.
4. Regenerate only the affected components.
5. Commit the code and its tests, referencing the issue (`Refs #<n>` on intermediate commits, `Closes #<n>` on the branch's final commit). The canvas is never part of a commit.

## Markdown output

Everything this skill writes to Markdown must pass `rumdl check`.

## Guardrails

- Do NOT generate code before reading the entire canvas and `AGENTS.md`
- Do NOT generate code for a canvas without a resolved work item
- Do NOT re-plan, reorder or skip operations
- Do NOT change signatures, field names, variant names or error messages from the specification
- Do NOT add features, types or fields that are not specified
- Do NOT add lint exceptions of any kind
- Do NOT patch code directly when issues are found: update the canvas first
- Always verify against the Definition of Done, and always finish with `just check` passing
- Never stage the canvas: commit only code and tests
- When generating reveals a gotcha worth keeping, offer to save it with the `ai-memory` write tools, and write it only if the user agrees (durable writes only on request, as the ai-memory block of `AGENTS.md` says). A trap that later changes must obey goes in a code comment. The memory tools are optional: if they are unavailable, say so in one line and continue.
