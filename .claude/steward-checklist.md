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
  one row in `docs/decisions.md` with `Status: assumed`.
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
- **Decisions log order.** A new row is appended at the end of `docs/decisions.md`, in fold
  order. Ids are unique, not sequential (charter-gates checks uniqueness and Status). The file
  merges with `merge=union`, so two branches that each append a row fold with both kept (D-104).
- **A head conflict is the steward's to resolve.** When the only thing keeping a commit from
  merging onto main is a CHARTER checklist line both sides touched, the steward rebases the
  commit onto main, pins the rebased commit, and says so in the notes. CHANGES only when the
  conflict touches a row's or line's content. A union merge that keeps two edits of one row
  shows as a duplicate id in charter-gates: CHANGES. (Orchestrator's rule, 2026-10-31; rows
  moved out of the conflict by D-104.)
- **`cleanshot-app/src/main.rs` stays wiring (CHARTER 5, "thin").** A commit that adds a
  pipeline there (a sequence that captures, cuts, delivers, times or asks the user, rather than
  connecting one module's event to another's call) gets a refine moving it into its own file
  in `cleanshot-app`, or CHANGES if the move needs the builder's decision. Measure main.rs
  before and after (`scc --by-file`), whatever its size.
- **`docs/security.md` in step.** A change to what `cleanshot.exe` touches (files, registry,
  clipboard, window messages, hotkeys, DLL loading, network, processes) updates
  `docs/security.md` in the same commit; CHANGES otherwise. (Orchestrator's rule, 2026-11-02.)
- **A hook is called, or its call is a named task.** When a commit adds a function another
  crate must call for the feature to show (an editor hook the app wires, such as
  `follow_window_surface`), `references` on it must find that call in the same commit or
  stack, or the notes name the follow-up task that adds it (brief id). CHANGES when neither
  holds. (Orchestrator's rule, 2026-11-06, after D-142 and D-144 folded with no caller.)
