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
