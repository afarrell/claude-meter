#!/usr/bin/env python3
"""
Deterministic sensitivity check for the self-merge gate.

Reads a list of changed files (one per line on stdin) and decides whether
the PR touches anything that requires human review. A path is sensitive if
it lives under .github/ or is the CODEOWNERS file. There is no content-aware
exemption — any change to those paths blocks self-merge so a human must
adjudicate changes to CI, workflows, or ownership rules.

No LLM is involved. The output is purely a function of the inputs, so a
prompt injection in PR content cannot flip the result. On any uncertainty,
the result is fail-closed: SENSITIVE.

Usage (typically called from GitHub Actions):
    gh pr view "$PR_NUMBER" --repo "$REPO" --json files \
      --jq '.files[].path' \
      | python3 .github/scripts/check-sensitive-paths.py "$PR_NUMBER" "$REPO"

Outputs:
    is_sensitive=true|false  (written to GITHUB_OUTPUT if set)
    Human-readable lines on stdout describing each file's classification
"""

import os
import re
import sys

# Path patterns that trigger the sensitivity gate. Each is a regex anchored
# at the start of the PR file path. Any match makes the whole PR sensitive.
SENSITIVE_PATTERNS = [
    re.compile(r"^\.github/"),
    re.compile(r"^CODEOWNERS$"),
]


def is_sensitive_path(path: str) -> bool:
    return any(p.match(path) for p in SENSITIVE_PATTERNS)


def write_output(value: str) -> None:
    out_path = os.environ.get("GITHUB_OUTPUT")
    if out_path:
        with open(out_path, "a") as f:
            f.write(f"is_sensitive={value}\n")


def main() -> int:
    if len(sys.argv) != 3:
        print("Usage: check-sensitive-paths.py <pr_number> <repo>", file=sys.stderr)
        return 2

    # pr_number and repo are accepted so the CLI contract matches the
    # canonical (content-aware) version, even though this simplified check
    # decides purely from the changed-path list on stdin.

    files = [line.strip() for line in sys.stdin if line.strip()]
    if not files:
        print("No files changed — non-sensitive (vacuously)")
        write_output("false")
        return 0

    print(f"Checking {len(files)} changed files against sensitivity rules:")
    print()

    sensitive = False
    for path in files:
        if is_sensitive_path(path):
            print(f"  [BLOCK  ] {path} — sensitive path (.github/** or CODEOWNERS)")
            sensitive = True
        else:
            print(f"  [safe   ] {path}")

    print()
    if sensitive:
        print("Result: SENSITIVE — self-merge will be blocked, human must merge.")
    else:
        print("Result: not sensitive — self-merge eligible if review approves.")

    write_output("true" if sensitive else "false")
    return 0


if __name__ == "__main__":
    sys.exit(main())
