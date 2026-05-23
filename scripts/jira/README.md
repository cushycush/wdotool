# Jira bootstrap

This directory holds the artifacts that move wdotool's planning out of `ACTIVE.md` / `CHANGELOG.md` (plus the project-memory backlog) and into a real issue tracker. Same shape as drift's and wflow's setups; the three projects share the same credentials file and the same seeder.

Three tiers under each Epic, matching wflow's team-managed kanban template:

- **Epic** is a long-running area of work (Engine, Backends, CLI, Recorder & replay, Verification & QA, Packaging & distribution, Drift integration, Wflow integration). Eight of them. Roughly stable for the life of the project.
- **Task (umbrella)** groups discrete pieces that all live in one surface. Under the Backends epic that's the five per-backend Tasks (wlr-protocols, libei, kde, gnome, uinput). A Task stays In Progress as long as anything under it is moving and only closes when the whole surface is shipped to its current target.
- **Subtask** is the leaf, parented to a Task. "Per-output virtual-pointer (wlroots)" is a Subtask under "wlr-protocols backend". This is where day-to-day work happens and where comments accumulate.

## One-time setup

1. The Atlassian site already exists at `cushycush.atlassian.net` (shared with DRIFT and WFLOW). Sign in.
2. **Projects → Create project**. Pick the **Team-managed software** template, kanban. Name it **wdotool**, key **WDOTOOL**.
3. In the new project, **Project settings → Features**, make sure **Epics**, **Backlog**, and **Board** are on. Sub-tasks too.
4. Confirm the project shows up: `python3 scripts/jira/seed.py projects` should now list `WDOTOOL`.

Credentials live at `~/.config/jira/env` (chmod 600), shared with drift and wflow:

```
JIRA_DOMAIN=cushycush.atlassian.net
JIRA_EMAIL=you@example.com
JIRA_TOKEN=<api-token-from-id.atlassian.com>
```

If the file is missing, `seed.py` falls back to the legacy `~/.config/drift/jira.env` path before erroring.

## Seeding

```sh
python3 scripts/jira/seed.py ping                          # verify the token reaches Jira
python3 scripts/jira/seed.py projects                      # confirm WDOTOOL exists
python3 scripts/jira/seed.py import-csv --limit 8          # smoke test, just the epics
python3 scripts/jira/seed.py import-csv                    # full run: 8 epics + tasks + subtasks
python3 scripts/jira/seed.py transitions                   # apply the CSV's Status column
```

The seeder is idempotent on Summary: re-running skips any issue whose Summary already exists. Pass `--force` to create duplicates anyway.

Jira's create-issue API does not accept a target Status (workflow transitions are separate calls), so everything lands in "To Do" regardless of the CSV's Status column. The `transitions` command moves each issue to the Status named in the CSV; run it after `import-csv`.

If you need to rewrite issue descriptions, priorities, or labels later, edit `issues.csv` and run `sync` (creates nothing, just pushes the three fields).

## What's in the CSV

Roughly 60 issues across 8 epics. Priorities map from the in-flight state:

- `Highest` = critical to v1.0 (shipped backends, current verification matrix, prime, KDE Plasma 6 path)
- `High` = important, scheduled (Drift integration surfaces, GNOME verification)
- `Medium` = nice to have, ideated
- `Low` = future polish

Status maps from the CHANGELOG and ACTIVE.md: shipped → Done, in-flight → In Progress, everything else → To Do.

Labels carry the metadata that doesn't fit a standard Jira field:

- `tier:free` (everything in wdotool is open and free; no Pro tier on this project).
- `effort:S` / `effort:M` / `effort:L`.
- `area:engine` / `backends` / `cli` / `recorder` / `qa` / `packaging` / `drift` / `wflow`.
- `status:shipped` / `status:scoped` / `status:idea` / `status:blocked` for the finer-grained roadmap state on Subtasks. The Jira Status field carries the workflow state; this label adds the nuance.

## Day-to-day commands

The agent-facing verbs that wire Jira into normal coding sessions:

```sh
seed.py start WDOTOOL-XX [--comment "..."]   # transition to In Progress
seed.py done  WDOTOOL-XX [--comment "..."]   # transition to Done
seed.py move  WDOTOOL-XX --to "In Review"    # any other status
seed.py comment WDOTOOL-XX "what changed"    # post a comment
seed.py delete WDOTOOL-XX --yes              # delete an issue (irreversible)
seed.py lookup <path>                        # path to WDOTOOL-XX via area-map.json
```

Path-to-ticket mappings live in `area-map.json`; first matching glob wins. The `lookup` command is what `CLAUDE.md` tells future sessions to run before starting work on any file under `wdotool-core/`, `wdotool/`, `packaging/`, or `docs/`.

## Git hook

`scripts/git-hooks/post-commit-jira` reads `Refs: WDOTOOL-XX` and `Closes: WDOTOOL-XX` trailers off each commit and posts the commit subject as a comment on every referenced ticket (and transitions `Closes:` tickets to Done). Cross-project refs (`Refs: WFLOW-12`, `Refs: DRIFT-72`) also land correctly. Install per-repo:

```sh
scripts/git-hooks/install.sh
```

Set `WDOTOOL_NO_JIRA=1` in the environment to skip per-commit.

## Future work

- A `seed.py migrate` command that scripts cross-project moves (the way drift's notes describe migrating Story → Sub-task in the UI).
- Sub-task auto-creation from "TODO" lines in commit messages.
- Status round-trip: a `wdotool-jira-status` script that queries Jira and renders a markdown snapshot back into the repo for offline reading. Same shape would help drift and wflow too.
