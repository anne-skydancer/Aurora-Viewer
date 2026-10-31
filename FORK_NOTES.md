# Fork notes: contributing to Aurora-Viewer from this fork

This branch (`fork-notes`) holds notes for this fork only. It is never merged
and never based on `main`, so nothing here reaches an upstream pull request.

`origin` = this fork, `upstream` = Aurora-Viewer/Aurora-Viewer. The fork
account has no write or triage rights upstream: maintainers approve CI runs,
set labels and merge.

## Follow the upstream PR protocol strictly

[AGENTS.md](https://github.com/Aurora-Viewer/Aurora-Viewer/blob/main/AGENTS.md)
on upstream `main` is authoritative; read it again at the start of every
session. The checklist below is a reminder, in order:

1. **Session start:** end the worktrees of merged PRs (`end-task.ps1`), check
   our open PRs and the issues they touch.
2. **One worktree per task:** `./scripts/new-task.ps1 -Name <name> -Kind
   <feat|fix|perf|refactor|docs|chore>`. Check that its base is
   `upstream/main` (`git fetch upstream`, rebase onto it if not). Build and
   test only in that worktree, default `dev` profile.
3. **TASKS.md:** set the task's line to 🔧 at the start. Search the open
   issues for the task; if one matches, link it (and take it on the board if
   the account allows it, otherwise say so).
4. **Legacy Viewer parity:** study `../phoenix-firestorm`, cite the source
   file and function in code comments, but write "Legacy Viewer" in anything
   new. Network and protocol follow it to the letter.
5. **Repository rules:** English everywhere; interface text through `tr!`
   with en, fr and ja keys; theme widgets, `Palette`, Tabler icons; unit
   tests for pure logic; a demo scenario (`AURORA_DEMO_*`) documented in
   `docs/TESTING.md`; `ARCHITECTURE.md` updated for new modules.
6. **`./scripts/check.ps1` green** (fmt, clippy `-D warnings`, workspace
   tests).
7. **Human validation before any PR** (except docs/CI/scripts-only PRs).
8. **Pull request:** title `type(area): summary`; body per
   `.github/pull_request_template.md` (`Closes #n` / `Refs #n` / `No issue`
   first). No mention of AI, no co-author or "generated with" lines. Labels
   cannot be set from the fork: name the intended type and area labels in
   the body.
9. **Self-review comment** in the exact `## Review` / `**Verdict:**` format
   of AGENTS.md section 11. Screenshots through `pr-captures.ps1` only when
   agreed (it pushes a `captures` branch to the fork).
10. **To resolve conflicts:** rebase on `upstream/main`, never merge `main`
    into the branch. After the maintainers merge: update the issue and
    `TASKS.md`, then `end-task.ps1`.
