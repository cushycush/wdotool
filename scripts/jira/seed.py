#!/usr/bin/env python3
"""
wdotool Jira seeder. Lifted from wflow's seed.py; same shape, same
shared credentials file, parameterized for the WDOTOOL project.

Reads credentials from ~/.config/jira/env (chmod 600):

    JIRA_DOMAIN=<your-site>.atlassian.net
    JIRA_EMAIL=you@example.com
    JIRA_TOKEN=<api-token>

Falls back to ~/.config/drift/jira.env for a machine that hasn't
moved its env file to the canonical location yet. Environment
variables of the same names override the file.

Commands:
    ping         verify the token reaches Jira and prints who you are
    projects     list visible projects (sanity check that WDOTOOL exists)
    import-csv   bulk-create epics, tasks, then sub-tasks from issues.csv
    transitions  move each issue's status to match the CSV's Status column
    sync         push description / priority / labels from CSV to existing issues
    start        transition KEY to In Progress (optional --comment)
    done         transition KEY to Done (optional --comment)
    move         transition KEY to a named status (--to "In Review" etc.)
    comment      post a comment to KEY
    lookup       given a file path, print the WDOTOOL-XX from area-map.json
"""

import argparse
import base64
import csv
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

# Canonical location first, drift legacy path as a fallback.
CRED_CANDIDATES = [
    Path.home() / ".config" / "jira" / "env",
    Path.home() / ".config" / "drift" / "jira.env",
]


def load_creds():
    creds = {}
    creds_path = next((p for p in CRED_CANDIDATES if p.exists()), None)
    if creds_path is not None:
        for line in creds_path.read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            key, _, value = line.partition("=")
            creds[key.strip()] = value.strip().strip('"').strip("'")
    for key in ("JIRA_DOMAIN", "JIRA_EMAIL", "JIRA_TOKEN"):
        if key in os.environ:
            creds[key] = os.environ[key]
        if key not in creds:
            sys.exit(
                f"missing {key} in {CRED_CANDIDATES[0]} or environment.\n"
                f"create the file with 'mkdir -p ~/.config/jira && chmod 700 ~/.config/jira' "
                f"then write the three KEY=VALUE lines and 'chmod 600 ~/.config/jira/env'."
            )
    return creds


def api(creds, method, path, body=None, params=None):
    url = f"https://{creds['JIRA_DOMAIN']}/rest/api/3{path}"
    if params:
        url = f"{url}?{urllib.parse.urlencode(params)}"
    auth = base64.b64encode(
        f"{creds['JIRA_EMAIL']}:{creds['JIRA_TOKEN']}".encode()
    ).decode()
    headers = {
        "Authorization": f"Basic {auth}",
        "Accept": "application/json",
    }
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req) as resp:
            raw = resp.read().decode()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode()
        sys.exit(f"{method} {path} failed: HTTP {exc.code}\n{detail}")


def adf(text):
    return {
        "type": "doc",
        "version": 1,
        "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": text}]}
        ],
    }


def existing_summaries(creds, project_key):
    """Return {summary: key} for every issue currently in the project."""
    summaries = {}
    next_page_token = None
    while True:
        body = {
            "jql": f"project = {project_key}",
            "fields": ["summary"],
            "maxResults": 100,
        }
        if next_page_token:
            body["nextPageToken"] = next_page_token
        result = api(creds, "POST", "/search/jql", body)
        for issue in result.get("issues", []):
            summaries[issue["fields"]["summary"]] = issue["key"]
        next_page_token = result.get("nextPageToken")
        if not next_page_token:
            break
    return summaries


def cmd_ping(args, creds):
    me = api(creds, "GET", "/myself")
    print(f"Authenticated as {me['displayName']} <{me['emailAddress']}>")
    print(f"Account ID: {me['accountId']}")
    print(f"Site: https://{creds['JIRA_DOMAIN']}")


def cmd_projects(args, creds):
    result = api(creds, "GET", "/project/search")
    for project in result.get("values", []):
        print(f"{project['key']:>10}  {project['name']}")


def cmd_import_csv(args, creds):
    existing = {} if args.force else existing_summaries(creds, args.project)
    if existing:
        print(f"Found {len(existing)} existing issues in {args.project}; will skip by summary match.")
        print(f"(pass --force to create duplicates anyway.)\n")

    with open(args.csv) as f:
        rows = list(csv.DictReader(f))

    epics = [r for r in rows if r["Issue Type"] == "Epic"]
    # Team-managed kanban template ships Task / Subtask
    # (not Story / Sub-task like drift's scrum template), so the
    # middle tier is filtered by both names.
    stories = [r for r in rows if r["Issue Type"] in ("Task", "Story")]
    subtasks = [r for r in rows if r["Issue Type"] in ("Sub-task", "Subtask")]

    if args.limit:
        epics = epics[: args.limit]
        remaining = max(0, args.limit - len(epics))
        stories = stories[:remaining]
        remaining = max(0, args.limit - len(epics) - len(stories))
        subtasks = subtasks[:remaining]

    workid_to_key = {}
    # Pre-populate workid_to_key from existing summaries for any rows
    # already in Jira, so sub-tasks created in a later pass can resolve
    # umbrella-task parents that were imported in an earlier run.
    for row in rows:
        summary = row["Summary"]
        if summary in existing:
            workid_to_key[row["Work item ID"]] = existing[summary]

    print(f"=> Creating {len(epics)} epics")
    for row in epics:
        summary = row["Summary"]
        if summary in existing:
            print(f"   skip {existing[summary]:>10}  {summary}")
            continue
        body = build_issue(args.project, row, parent_key=None, issue_type="Epic")
        result = api(creds, "POST", "/issue", body)
        workid_to_key[row["Work item ID"]] = result["key"]
        print(f"   new  {result['key']:>10}  {summary}")
        time.sleep(0.2)

    print(f"\n=> Creating {len(stories)} tasks")
    for row in stories:
        summary = row["Summary"]
        if summary in existing:
            print(f"   skip {existing[summary]:>10}  {summary}")
            continue
        parent_key = workid_to_key.get(row["Parent"])
        if not parent_key:
            print(f"   WARN no parent for {summary} (Parent work_id={row['Parent']!r})")
            continue
        # CSV's Issue Type stays as the source of truth so a row marked
        # "Task" creates a Task and a row marked "Story" creates a Story.
        body = build_issue(args.project, row, parent_key=parent_key, issue_type=row["Issue Type"])
        result = api(creds, "POST", "/issue", body)
        workid_to_key[row["Work item ID"]] = result["key"]
        print(f"   new  {result['key']:>10}  {summary}  (parent {parent_key})")
        time.sleep(0.2)

    print(f"\n=> Creating {len(subtasks)} sub-tasks")
    for row in subtasks:
        summary = row["Summary"]
        if summary in existing:
            print(f"   skip {existing[summary]:>10}  {summary}")
            continue
        parent_key = workid_to_key.get(row["Parent"])
        if not parent_key:
            print(f"   WARN no parent for {summary} (Parent work_id={row['Parent']!r}; "
                  f"create the umbrella task first)")
            continue
        body = build_issue(args.project, row, parent_key=parent_key, issue_type=row["Issue Type"])
        result = api(creds, "POST", "/issue", body)
        workid_to_key[row["Work item ID"]] = result["key"]
        print(f"   new  {result['key']:>10}  {summary}  (parent {parent_key})")
        time.sleep(0.2)

    print(f"\nDone. Status starts at 'To Do' for everything created (Jira workflow "
          f"won't accept a target status on create). Run `transitions` to apply the "
          f"CSV's Status column.")


def cmd_transitions(args, creds):
    summary_to_key = existing_summaries(creds, args.project)

    with open(args.csv) as f:
        rows = list(csv.DictReader(f))

    moved = 0
    for row in rows:
        target = row["Status"]
        if target == "To Do":
            continue
        summary = row["Summary"]
        key = summary_to_key.get(summary)
        if not key:
            print(f"   miss {summary!r} not in {args.project}")
            continue

        issue = api(creds, "GET", f"/issue/{key}", params={"fields": "status"})
        current = issue["fields"]["status"]["name"]
        if current == target:
            print(f"   ok   {key:>10}  {current:11s}  {summary}")
            continue

        trans = api(creds, "GET", f"/issue/{key}/transitions")
        match = next(
            (t for t in trans["transitions"] if t["to"]["name"] == target),
            None,
        )
        if not match:
            available = ", ".join(t["to"]["name"] for t in trans["transitions"])
            print(f"   WARN {key:>10}  no transition {current!r} -> {target!r}; available: {available}")
            continue

        api(creds, "POST", f"/issue/{key}/transitions", {"transition": {"id": match["id"]}})
        print(f"   move {key:>10}  {current:11s} -> {target:11s}  {summary}")
        moved += 1
        time.sleep(0.2)

    print(f"\nMoved {moved} issues.")


def cmd_sync(args, creds):
    summary_to_key = existing_summaries(creds, args.project)

    with open(args.csv) as f:
        rows = list(csv.DictReader(f))

    touched = 0
    skipped = 0
    for row in rows:
        summary = row["Summary"]
        key = summary_to_key.get(summary)
        if not key:
            print(f"   miss {summary!r} not in {args.project}")
            continue

        current = api(creds, "GET", f"/issue/{key}", params={"fields": "description,priority,labels"})
        cur_desc = render_adf(current["fields"].get("description"))
        cur_priority = (current["fields"].get("priority") or {}).get("name", "")
        cur_labels = sorted(current["fields"].get("labels") or [])

        new_desc = row["Description"]
        new_priority = row["Priority"]
        new_labels = sorted(row["Labels"].split()) if row["Labels"] else []

        fields = {}
        if cur_desc.strip() != new_desc.strip():
            fields["description"] = adf(new_desc)
        if cur_priority != new_priority:
            fields["priority"] = {"name": new_priority}
        if cur_labels != new_labels:
            fields["labels"] = new_labels

        if not fields:
            skipped += 1
            continue

        api(creds, "PUT", f"/issue/{key}", {"fields": fields})
        changed = ", ".join(fields.keys())
        print(f"   sync {key:>10}  [{changed}]  {summary}")
        touched += 1
        time.sleep(0.2)

    print(f"\nSynced {touched} issues, skipped {skipped} unchanged.")


def transition_to(creds, key, target):
    """Move `key` to status `target` (e.g. "In Progress"). No-op if already there."""
    issue = api(creds, "GET", f"/issue/{key}", params={"fields": "status,summary"})
    current = issue["fields"]["status"]["name"]
    summary = issue["fields"]["summary"]
    if current == target:
        print(f"   ok   {key:>10}  already {current!r}  {summary}")
        return False
    trans = api(creds, "GET", f"/issue/{key}/transitions")
    match = next(
        (t for t in trans["transitions"] if t["to"]["name"] == target),
        None,
    )
    if not match:
        available = ", ".join(t["to"]["name"] for t in trans["transitions"])
        sys.exit(f"no transition {current!r} -> {target!r} on {key}; available: {available}")
    api(creds, "POST", f"/issue/{key}/transitions", {"transition": {"id": match["id"]}})
    print(f"   move {key:>10}  {current:11s} -> {target:11s}  {summary}")
    return True


def add_comment(creds, key, text):
    api(creds, "POST", f"/issue/{key}/comment", {"body": adf(text)})
    print(f"   note {key:>10}  {text[:70]}{'...' if len(text) > 70 else ''}")


def cmd_start(args, creds):
    transition_to(creds, args.key, "In Progress")
    if args.comment:
        add_comment(creds, args.key, args.comment)


def cmd_done(args, creds):
    transition_to(creds, args.key, "Done")
    if args.comment:
        add_comment(creds, args.key, args.comment)


def cmd_move(args, creds):
    transition_to(creds, args.key, args.to)
    if args.comment:
        add_comment(creds, args.key, args.comment)


def cmd_comment(args, creds):
    add_comment(creds, args.key, args.text)


def cmd_delete(args, creds):
    # Destructive: dropping an issue removes its history, comments, and key.
    # Require --yes to avoid accidental fires.
    if not args.yes:
        sys.exit(f"refusing to delete {args.key} without --yes")
    api(creds, "DELETE", f"/issue/{args.key}")
    print(f"   del  {args.key}")


AREA_MAP_PATH = Path(__file__).parent / "area-map.json"


def load_area_map():
    if not AREA_MAP_PATH.exists():
        return []
    raw = json.loads(AREA_MAP_PATH.read_text())
    # Expect a list of {"glob": "wdotool-core/src/backend/kde.rs", "key": "WDOTOOL-12"}
    # Keep insertion order so callers can put the most specific rules first.
    return raw if isinstance(raw, list) else []


def cmd_lookup(args, creds):
    import fnmatch
    rules = load_area_map()
    if not rules:
        sys.exit(f"no rules in {AREA_MAP_PATH}; create it as a JSON list of "
                 f"{{\"glob\": ..., \"key\": ...}} entries")
    matches = []
    for rule in rules:
        if fnmatch.fnmatch(args.path, rule["glob"]):
            matches.append(rule)
    if not matches:
        print(f"no Jira mapping for {args.path}")
        return
    for rule in matches:
        print(f"{rule['key']}\t{rule['glob']}")


def render_adf(adf_doc):
    """Flatten an ADF document back to plain text for comparison."""
    if not adf_doc:
        return ""
    out = []
    for block in adf_doc.get("content", []):
        if block.get("type") == "paragraph":
            for span in block.get("content", []):
                if span.get("type") == "text":
                    out.append(span.get("text", ""))
            out.append("\n")
    return "".join(out).strip()


def build_issue(project_key, row, parent_key, issue_type):
    labels = row["Labels"].split() if row["Labels"] else []
    fields = {
        "project": {"key": project_key},
        "summary": row["Summary"],
        "issuetype": {"name": issue_type},
        "description": adf(row["Description"]),
        "priority": {"name": row["Priority"]},
        "labels": labels,
    }
    if parent_key:
        fields["parent"] = {"key": parent_key}
    return {"fields": fields}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_ping = sub.add_parser("ping", help="verify credentials reach Jira")
    p_ping.set_defaults(func=cmd_ping)

    p_proj = sub.add_parser("projects", help="list visible projects")
    p_proj.set_defaults(func=cmd_projects)

    p_imp = sub.add_parser("import-csv", help="bulk-create issues from issues.csv")
    p_imp.add_argument("--project", default="WDOTOOL", help="project key (default WDOTOOL)")
    p_imp.add_argument(
        "--csv",
        default=str(Path(__file__).parent / "issues.csv"),
        help="path to issues.csv",
    )
    p_imp.add_argument("--force", action="store_true", help="create even if a summary already exists")
    p_imp.add_argument("--limit", type=int, default=0, help="create at most N issues (smoke test)")
    p_imp.set_defaults(func=cmd_import_csv)

    p_tr = sub.add_parser("transitions", help="move statuses to match the CSV's Status column")
    p_tr.add_argument("--project", default="WDOTOOL", help="project key (default WDOTOOL)")
    p_tr.add_argument(
        "--csv",
        default=str(Path(__file__).parent / "issues.csv"),
        help="path to issues.csv",
    )
    p_tr.set_defaults(func=cmd_transitions)

    p_sync = sub.add_parser("sync", help="push description / priority / labels from CSV to existing issues")
    p_sync.add_argument("--project", default="WDOTOOL", help="project key (default WDOTOOL)")
    p_sync.add_argument(
        "--csv",
        default=str(Path(__file__).parent / "issues.csv"),
        help="path to issues.csv",
    )
    p_sync.set_defaults(func=cmd_sync)

    p_start = sub.add_parser("start", help="transition KEY to In Progress")
    p_start.add_argument("key")
    p_start.add_argument("--comment", default="", help="optional comment to post on the transition")
    p_start.set_defaults(func=cmd_start)

    p_done = sub.add_parser("done", help="transition KEY to Done")
    p_done.add_argument("key")
    p_done.add_argument("--comment", default="", help="optional comment to post on the transition")
    p_done.set_defaults(func=cmd_done)

    p_move = sub.add_parser("move", help="transition KEY to a named status")
    p_move.add_argument("key")
    p_move.add_argument("--to", required=True, help='target status name, e.g. "In Review"')
    p_move.add_argument("--comment", default="", help="optional comment to post on the transition")
    p_move.set_defaults(func=cmd_move)

    p_comm = sub.add_parser("comment", help="post a comment to KEY")
    p_comm.add_argument("key")
    p_comm.add_argument("text")
    p_comm.set_defaults(func=cmd_comment)

    p_del = sub.add_parser("delete", help="delete KEY (irreversible; requires --yes)")
    p_del.add_argument("key")
    p_del.add_argument("--yes", action="store_true", help="confirm; without this the call refuses")
    p_del.set_defaults(func=cmd_delete)

    p_look = sub.add_parser("lookup", help="given a path, print the matching WDOTOOL-XX from area-map.json")
    p_look.add_argument("path")
    p_look.set_defaults(func=cmd_lookup)

    args = parser.parse_args()
    creds = load_creds()
    args.func(args, creds)


if __name__ == "__main__":
    main()
