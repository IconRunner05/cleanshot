# STANDARDS.md

General rules for writing and reviewing code. The test behind every one: **would a careful
human maintainer have written this, and could a newcomer find it?** Code written by an LLM
fails that test in recognisable ways (too much code, put in odd places, over-explained, in
words no person uses), and most rules below name the habit they correct. Nothing here names
a module of this repo: CLAUDE.md's "This repo" sections say how each rule applies here, and
a reviewer checks a change against both.

**This file wins.** Where a loaded skill, style guide or tool default says otherwise (for
example "comment liberally"), follow this file and say so in the review.

## Less code

- **Reuse before writing.** Look for an existing function, type or module that already does
  it (through the symbol server) before adding one. A near-copy of something that exists is a
  defect, even when it works.
- **The smallest change that does the job.** No unrequested options, flags, config, layers,
  extension points or "for later" generality. No wrapper that only forwards a call. No
  abstraction with one user.
- **Stay inside the change.** Do not reformat, rename or "tidy" code the task did not need.
  A reviewer should see only what the change is about.
- **Trust the types.** No checks for cases the types already rule out, no handler around
  code that cannot fail, no fallback for a state that cannot happen.
- **Delete, don't keep.** Dead code is removed, never commented out. No `old_`, `new_`, `v2`,
  `legacy` or `_backup` copies beside the real one.
- **Long means wrong.** A change much longer than the problem it solves points at the design.
  Re-derive the design before adding more code to it.

## Code where a person would look

- **Ask "where would I look for this?" and put it there.** Next to the code it belongs with,
  in the unit whose one-sentence purpose it fits, in a file named for what is in it.
- **A big file is a design question.** A source file past about 1000 code lines very likely
  holds more than one concept and should be split along them, unless a reasonable case shows
  the split would leave the code less clear, worse organised or less effective. "It was
  already big" and "this only adds a little" are not that case. Tests split with the module
  they test.
- **No new file for a function, no new module for a helper.** A new unit is for a new
  concept, not for a few lines that have a home.
- **Group by domain, never by pattern.** No `utils`, `helpers`, `common`, `misc`, `shared`,
  `types`, `constants`, `services` or `models` modules or folders; a function or type lives
  with the concept it serves. One coherent module beats several fragments a caller must
  import together.
- **Match the neighbours.** A new function reads like the code around it: same idioms, same
  naming, same layout, same density of comments. A file that looks different from its
  siblings is a review finding.
- **Flat and direct.** Early returns over nesting; one level of abstraction per function;
  functions short, but not chopped into one-line pieces that make the reader jump around.

## Documentation: less, and only the contract

- **Default to no comment.** Good names and types carry what the code does. A comment says
  why, and only when the why is not obvious: a constraint, a trade-off, a deliberate absence.
  This overrides any skill or style guide that says to comment liberally.
- **A comment that restates the code means the code needs a better name.** Rename and delete
  the comment.
- **A doc comment is the contract**, on public items only, in a sentence or two, short enough
  for a hover, sitting directly on its own definition. It says what the caller gets and must
  give, never how the body works or what each parameter's type already says.
- **None of these, ever:** "Note:", "IMPORTANT:", "This function...", "Helper to...",
  section banners, emoji, comments narrating the edit ("now uses", "fixed", "updated to",
  "new"), commented-out code, TODOs with no owner.
- **No new prose files.** A change does not add a README, summary, report or plan document
  unless asked. What a change did goes in its commit message.

## Words a person would use

- **Names are words you would say out loud** about the domain. No abbreviations. No empty
  nouns or suffixes: `data`, `info`, `item`, `obj`, `Manager`, `Handler`, `Processor`,
  `Service`, `Helper`, `Util`, `Wrapper`, `Impl`, `Base`. No hype: `Advanced`, `Enhanced`,
  `Smart`, `Robust`. No variant prefixed with its type's name.
- **One word per role, never mixed.** Each kind of actor or thing has one name, defined once,
  and that name means only that.
- **Say the specific thing.** No stock LLM phrasing in code, comments, messages or commits:
  "robust", "seamless", "comprehensive", "leverage", "utilize", "ensure", "delve",
  "streamline", "facilitate", "crucial", "essential", "various", "enhance", "it's worth
  noting", "in order to". No hedging, no lists of three for rhythm, no bold for emphasis.
- **Plain ASCII** in code and commit messages: no em dashes, arrows or smart quotes.
- **Messages a user reads are plain sentences** saying what happened and what to do.
- **Text shown to an LLM is code.** A prompt stays true after a behaviour change and never
  points at something the LLM is no longer shown.

## Organisation

- **One concern per unit.** A package, crate or module is about one thing, said in a
  sentence.
- **Dependencies point one way, and a test enforces it.** Core never depends on what is built
  on it; optional parts (a UI, a plugin, a second binary) depend on the core and can be left
  out.
- **Explicit public surface.** Each unit says which modules are its API; the rest are
  internal. A type in a public API comes from a public module.
- **One copy of shared code,** in the lowest unit both sides may import. Logic two callers
  both need is computed once, there, not in each caller.
- **Upstream is not ours.** A dependency someone else owns is used with its types as they
  are; a gap goes to its owner as a request, never a local patch or mirror. Pinned, and moved
  deliberately and everywhere at once.
- **Fixtures live with what they fake,** and only tests call them.

## Design

- **Make illegal states unrepresentable.** A sum type over a Bool or a pair of optionals, a
  non-empty list where empty means nothing, a tagged id over a raw string. Never store what
  can be derived.
- **Gates over checks.** A rule that must hold is a type whose only constructors check it,
  not a check repeated at every caller.
- **One door for each kind of change,** one exhaustive match with no catch-all, so a new
  case forces a decision at every pairing. No catch-all on a type you own anywhere: the
  compiler can only point at every place a new variant needs if none hides behind `_`.
- **Concrete over abstract.** Solve the specific problem with plain functions and data; no
  generic machinery (a trait or behaviour with one implementation, hand-rolled type classes,
  plugin registries) for one use.
- **Policy apart from mechanism.** Logic another strategy would decide differently reads
  the mechanism's state through its public queries and never re-implements its checks.
- **Foreign data gets specific types,** decoded at the boundary, once. Nothing dynamic or
  untyped travels past the module that received it.
- **Dropped on purpose stays dropped** until a new decision brings it back.

## Errors

Three kinds, handled three ways:

- **Expected failures are values.** A failure the caller can act on is a variant of the
  module's own error type, naming what went wrong (`NotFound(id)`, `TooLong`), carrying
  data, never a rendered sentence. Text for a person is made where it is shown.
- **Never swallow an error.** An error handled by ignoring it is still bound and reported
  (logged, or carried into the result). Discarding it silently hides the failure from
  everyone; the language sections name the forms this takes.
- **Unexpected failures do not carry on.** A broken invariant is not caught and worked
  around. Where something supervises the code, it crashes and is restarted; elsewhere it
  fails loudly with the invariant named. The repo's CLAUDE.md says where crashing is
  allowed.

## Records

- **The map stays in step.** The README names every unit and module and what it does; a new
  or moved module gets its row in the same commit.
- **Decisions say why, never how,** with whose call it was and when, in the repo's decision
  record. How the code works is read from the code.
- **Evidence goes in the commit message,** not the decision record. A record never cites a
  one-off run or a count that will go stale.
- **Every claim is true of the code.**

## Tests and measurement

- **Test behaviour, not structure.** One test per rule that matters, named as a sentence
  saying what holds; no test per getter, no test that restates the implementation.
- **Tests never call an outside service;** scripted stand-ins keep them deterministic.
- **A test must fail without its fix.** Show it red with the fix removed, then green.
- **Measure with the product's own commands,** never a private script.
- **Fast loop per change, full run at milestones.** A long end-to-end run is one noisy
  sample, not a measurement.
- **One decision per commit,** with its measured numbers, so it reviews and reverts alone.

## Tools

- **Symbols through the language server,** never grep: where a thing is defined, who calls
  it, what a change breaks. Look up callers before changing anything.
- **A missing tool is said, never worked around silently.**
- **Load the skill for the task** before writing code, and name the skills used. Where a
  skill disagrees with this file, this file wins.

## Rust

- **Errors.** A library crate's expected failures are its own error enum, one variant per
  failure the caller can act on; a catch-all error type (`anyhow`, `Box<dyn Error>`) only at
  a binary's top level, where errors are reported, not matched. Propagate with `?`.
  Swallowing looks like `let _ = fallible()`, `.ok()`, `.unwrap_or_default()` or an empty
  `Err(_) => {}`; bind the error and report it, or the code says why it is safe to drop.
- **Panics.** No `unwrap()` outside tests. `expect("<the invariant>")` only for an invariant
  the types cannot express, its message naming the invariant. A library never panics on
  input it did not create.
- **No `_ =>` on an enum you own.** Match every variant. `#[non_exhaustive]` only on a
  public type meant for crates you do not own.
- **Visibility is as narrow as the callers:** private by default, `pub(crate)` for the crate,
  `pub` only for the crate's API.
- **No trait with one implementation,** unless it is a test seam named as one.
- **Serde types live at the boundary** and convert into domain types once, with the
  conversion returning the domain error.
- **`unsafe` only with a `// SAFETY:` comment** stating the invariant that makes it sound.
- **Tasks**
  - **A task only when it earns one,** as for a process: concurrent waiting, or work that
    outlives the call. CPU work stays synchronous.
  - **Every spawned task is owned.** Its `JoinHandle` is awaited or held by a `JoinSet` or
    a supervisor that sees it fail; a dropped handle is a task that can die unnoticed.
  - **Nothing blocks inside async code;** blocking work goes to `spawn_blocking` or a
    thread.
  - **A channel's messages are private** to the module that owns the receiver; callers use
    its functions.
  - **Timeouts are decided, not defaulted,** and a timeout is an error the caller sees.
- **Checks:** `cargo fmt --check` and `cargo clippy` with warnings denied.
