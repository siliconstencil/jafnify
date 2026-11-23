#!/usr/bin/env bash
# Generate THIRD_PARTY_NOTICES.txt for the packaged targets, fully offline.
#
# The input is `cargo metadata --offline` of linux/ (x86_64 Linux) and
# app/wgserv/ (aarch64 Android). Only normal dependencies are walked: dev
# dependencies and build-only crates do not ship, so they are not listed
# unless the same crate is also linked as a normal dependency.
#
# Every crate gets its version, its licence expression and the full text of
# its licence/notice files from ~/.cargo/registry/src. A licence that is not
# permissive (GPL/LGPL/AGPL/MPL, or unknown) is printed loudly at the top and
# the script fails unless the crate is in ALLOW below.
#
# Usage: tools/release/notices.sh [output-file]
#   default: <tree>/tmp/release/THIRD_PARTY_NOTICES.txt
set -eu

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/tmp/release/THIRD_PARTY_NOTICES.txt}"
VERSION="${JAFNIFY_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/linux/Cargo.toml" | head -n 1)}"

# Non-permissive crates explicitly allowed, one "name version"
# per line, with the reason in a # comment on the same line.
ALLOW="
epaint_default_fonts 0.37.0  # SIL OFL-1.1 and Ubuntu font licence: the font files may be redistributed with their licence texts, which this file includes
option-ext 0.2.0  # MPL-2.0: file-level copyleft, used unmodified, full text included below
"

mkdir -p "$(dirname "$OUT")"

python3 - "$ROOT" "$OUT" "$VERSION" "$ALLOW" <<'PY'
import json
import os
import subprocess
import sys

root, out_path, version, allowed_raw = sys.argv[1:5]
allowed = set()
for line in allowed_raw.splitlines():
    entry = line.split("#", 1)[0].strip().rstrip(",").strip()
    if entry:
        allowed.add(entry)

PERMISSIVE = (
    "MIT",
    "APACHE-2.0",
    "BSD-2-CLAUSE",
    "BSD-3-CLAUSE",
    "BSD-3-CLAUSE-CLEAR",
    "ISC",
    "0BSD",
    "UNLICENSE",
    "ZLIB",
    "CC0-1.0",
    "CC-BY-4.0",
    "UNICODE-3.0",
    "UNICODE-DFS-2016",
)
FLAGGED = ("GPL", "LGPL", "AGPL", "MPL")


def metadata(manifest, target):
    cmd = [
        "cargo",
        "metadata",
        "--offline",
        "--format-version",
        "1",
        "--manifest-path",
        os.path.join(root, manifest),
        "--filter-platform",
        target,
    ]
    return json.loads(subprocess.check_output(cmd, cwd=root))


def normal_reachable(meta):
    packages = {pkg["id"]: pkg for pkg in meta["packages"]}
    nodes = {node["id"]: node for node in meta["resolve"]["nodes"]}
    root_id = meta["resolve"]["root"]
    if root_id is None:
        return {}
    seen = {}
    stack = [root_id]
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen[pid] = packages[pid]
        for dep in nodes[pid]["deps"]:
            kinds = dep.get("dep_kinds") or [{"kind": None}]
            if any(kind.get("kind") in (None, "normal") for kind in kinds):
                stack.append(dep["pkg"])
    return seen


def licence_files(directory):
    found = []
    if not os.path.isdir(directory):
        return found
    for name in sorted(os.listdir(directory)):
        lower = name.lower()
        if lower.startswith(("license", "licence", "copying", "notice")):
            path = os.path.join(directory, name)
            if os.path.isfile(path):
                found.append(path)
    return found


def texts(pkg):
    dirs = [os.path.dirname(pkg["manifest_path"])]
    # Path crates keep their licence at the tree root; look one level up too.
    if pkg.get("source") is None:
        dirs.append(os.path.dirname(dirs[0]))
    result = []
    seen = set()
    for directory in dirs:
        for path in licence_files(directory):
            if path in seen:
                continue
            seen.add(path)
            try:
                with open(path, "r", encoding="utf-8", errors="replace") as handle:
                    result.append((path, handle.read()))
            except OSError:
                continue
    return result


def expression(pkg):
    if pkg.get("license"):
        return pkg["license"].strip()
    # A crate in this tree without its own licence field is this project's,
    # covered by the tree's MIT LICENSE.
    if pkg.get("source") is None:
        return "MIT"
    if pkg.get("license_file"):
        return "see " + os.path.basename(pkg["license_file"])
    return "UNKNOWN"


def permissive(expr):
    upper = expr.upper().replace("(", " ").replace(")", " ").strip()
    if "UNKNOWN" in upper:
        return False
    # An AND alternative ships everything, so every part must be permissive.
    if " AND " in upper:
        return all(permissive(part) for part in upper.split(" AND "))
    # An OR alternative ships one of them, so one permissive part is enough.
    if " OR " in upper:
        return any(permissive(part) for part in upper.split(" OR "))
    if any(word in upper for word in FLAGGED):
        return False
    tokens = upper.replace("/", " ").split()
    operators = {"OR", "AND", "WITH"}
    return all(
        token in operators or token in PERMISSIVE or token.endswith("EXCEPTION")
        for token in tokens
    )


crates = {}
for manifest, target in (
    ("linux/Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("app/wgserv/Cargo.toml", "aarch64-linux-android"),
):
    for pkg in normal_reachable(metadata(manifest, target)).values():
        crates[(pkg["name"], pkg["version"])] = pkg

ordered = [crates[key] for key in sorted(crates)]

flagged = []
summary = {}
sections = []
for pkg in ordered:
    expr = expression(pkg)
    summary[expr] = summary.get(expr, 0) + 1
    marker = "" if pkg.get("source") else " (this project)"
    missing = not texts(pkg)
    sections.append((pkg, expr, marker, missing))
    if not permissive(expr):
        flagged.append((pkg, expr))

lines = []
lines.append("Jafnify third-party notices")
lines.append("===========================")
lines.append("")
lines.append(
    "Jafnify %s is licensed under the MIT licence (see LICENSE)." % version
)
lines.append("")
lines.append(
    "This file lists every crate linked into the Linux build (tarball and .deb)"
)
lines.append(
    "and the Android native library, with its version, its licence expression"
)
lines.append(
    "and the full text of its licence and notice files from the local cargo"
)
lines.append("registry. It is generated offline by tools/release/notices.sh.")
lines.append("")

if flagged:
    lines.append("!" * 70)
    lines.append("LICENCES THAT NEED A REVIEW (not permissive):")
    for pkg, expr in flagged:
        key = "%s %s" % (pkg["name"], pkg["version"])
        if key in allowed:
            lines.append("  allowed: %s - %s" % (key, expr))
        else:
            lines.append("  FLAGGED: %s - %s" % (key, expr))
    lines.append("!" * 70)
    lines.append("")

lines.append("Licence summary")
lines.append("---------------")
for expr in sorted(summary):
    lines.append("%-40s %d" % (expr, summary[expr]))
lines.append("")

for pkg, expr, marker, missing in sections:
    lines.append("Crate: %s %s - %s%s" % (pkg["name"], pkg["version"], expr, marker))
    lines.append("-" * 60)
    if missing:
        lines.append(
            "(no LICENSE/COPYING/NOTICE file was found in the local registry "
            "for this crate; the licence expression above is the crate's own)"
        )
    for path, text in texts(pkg):
        lines.append("File: %s" % os.path.basename(path))
        lines.append(text.rstrip())
        lines.append("")
    lines.append("")

# The embedded UI and log fonts are not Cargo crates; list them with their
# licence texts from the tree so the packaged notice file covers them too.
for font_name, licence_name in (
    ("Inter (Regular and SemiBold)", "Inter-OFL.txt"),
    ("JetBrains Mono (Regular)", "JetBrainsMono-OFL.txt"),
):
    lines.append("Font: %s - SIL Open Font License 1.1" % font_name)
    lines.append("-" * 60)
    licence_path = os.path.join(root, "linux", "assets", "fonts", licence_name)
    try:
        with open(licence_path, "r", encoding="utf-8") as handle:
            licence_text = handle.read()
    except OSError:
        lines.append("(licence text missing at %s)" % licence_path)
    else:
        lines.append("File: %s" % licence_name)
        lines.append(licence_text.rstrip())
    lines.append("")

with open(out_path, "w", encoding="utf-8") as handle:
    handle.write("\n".join(lines).rstrip() + "\n")

print("wrote %s (%d crates)" % (out_path, len(ordered)))
refused = 0
for pkg, expr in flagged:
    key = "%s %s" % (pkg["name"], pkg["version"])
    if key not in allowed:
        refused += 1
        print(
            "notices: FLAGGED %s - %s (add it to ALLOW only with a reason)"
            % (key, expr),
            file=sys.stderr,
        )
if refused:
    sys.exit(1)
PY
