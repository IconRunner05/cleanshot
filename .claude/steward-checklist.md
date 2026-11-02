# Steward checklist (Cleanshot)

Repo-specific checks on top of STANDARDS.md and the gate. The owner's bar for every commit:
clean, well organised, and written by a human for a human.

- **Crate ownership.** The change sits in the crate CHARTER §5 gives that concern. No OS call
  outside `cleanshot-win`; no `unsafe` outside it; every `unsafe` has `// SAFETY:` (C-16).
- **Charter rules hold.** No network or process-launch code, no dependency outside CHARTER §6,
  no `deny.toml` loosening without a Decisions-log row (C-10, C-11, §6).
- **Comments.** None by default. The one allowed kind is a charter ID on the line that
  enforces it (`// C-15: reject reserved names`). Anything longer is a CHANGES or a refine.
- **Reads like a person wrote it.** Spoken-word names, no stock LLM phrasing, no narration
  of the edit, plain ASCII, in code, comments, workflow step names and commit messages.
- **No new prose files.** Findings and measurements go in the commit message; decisions get
  one row in CHARTER §11 with `Status: assumed`.
- **Tests named after IDs** where they prove a criterion (`f21_never_overwrites_existing_file`).
- **Charter in step.** A behaviour change updates `docs/CHARTER.md` in the same change set.
- **Windows-only tests are shown red and green on GitHub Actions.** The commit message quotes
  the failure line, the URLs of a `ci/<seat>-red` run (fix removed) and a `ci/<seat>` run, and
  the SHA and tree hash the green run built (amending the message changes the SHA, not the
  tree). Check:
  - `gh run view <green> --json headSha` equals the named SHA;
  - `git rev-parse <named>^{tree}` equals `git rev-parse <commit>^{tree}`;
  - the red run's head commit has the named SHA as its parent, and `gh run view <red>` fails on
    that test for the reason quoted.

  A PASS survives a clean rebase onto a moved main (no conflict, no edit by the builder): the
  orchestrator folds with `--allow-moved`, pushes main at once, and main's own CI run is the
  tree-exact Windows run; if it goes red, the fold is reverted. A rebase with a conflict, or
  any edit, needs a fresh green run.

  One green per stack tip: a commit is covered by the green run of any descendant whose only
  extra content is commits on the same stack. One red per Windows-only test, each with a
  single break; a red's parent is its commit or any descendant on the same stack. A red
  counts only if its failing step is the test step and the named test is the one that
  fails; a red stopped earlier (formatting, clippy, build) proves nothing.
- **Decisions log order.** Rows stay in descending D-id order, and a new row goes in its id
  position, never at the top or the end of the table.
- **A head conflict is the steward's to resolve.** When the only thing keeping a commit from
  merging onto main is two new Decisions-log rows at the head, or a CHARTER checklist line both
  sides touched, the steward rebases the commit onto main, keeps the rows in descending order
  (charter-gates checks it), pins the rebased commit, and says so in the notes. CHANGES only when
  the conflict touches a row's content. (Orchestrator's rule, 2026-10-31, after five round trips
  under `decision-row-head-conflict`.)
- **`cleanshot-app/src/main.rs` stays wiring (CHARTER 5, "thin").** A commit that adds a
  pipeline there (a sequence that captures, cuts, delivers, times or asks the user, rather than
  connecting one module's event to another's call) gets a refine moving it into its own file
  in `cleanshot-app`, or CHANGES if the move needs the builder's decision. Measure main.rs
  before and after (`scc --by-file`), whatever its size.
- **`docs/security.md` in step.** A change to what `cleanshot.exe` touches (files, registry,
  clipboard, window messages, hotkeys, DLL loading, network, processes) updates
  `docs/security.md` in the same commit; CHANGES otherwise. (Orchestrator's rule, 2026-11-02.)
