#!/usr/bin/env python3
"""Count tracked source per module against the limits in SPEC.md section 6; exit 1 on a failure."""
from pathlib import Path
import re
import subprocess
import sys

# Each module's directories, under the first word of its row; a file directly in src/ is its own.
ROWS = {"Protocol": "src/protocol", "Session": "src/controller", "Record": "src/record",
        "Context": "src/profiles", "Model": "src/responses src/claude bridge",
        "Tool": "src/tools src/code_mode", "Shell": "src/shell",
        "Children": "src/children src/residents src/memory src/launcher.rs",
        "API": "src/api src/config src/devices.rs src/main.rs src/lib.rs", "Extension": "src/extensions",
        "Terminal": "src/terminal", "Flows": "flows scripts eval", "Performance": "perf"}
COMMENTS = {".rs": "//", ".ts": "//", ".js": "//", ".py": "#", ".sh": "#"}

root = Path(__file__).resolve().parents[1]
section = (root / "SPEC.md").read_text().split("## 6. Budgets", 1)[1].split("## 7.", 1)[0]
limits = {name.strip("* "): int(limit.strip("*").replace(",", ""))
          for name, limit in re.findall(r"\| ([^|]+) \| ([\d,*]+) \|", section)}
modules = {directory: next((name for name in limits if name.startswith(row)), None)
           for row, directories in ROWS.items() for directory in directories.split()}
counts = dict.fromkeys(limits, 0)
failures = []
for name in subprocess.check_output(["git", "ls-files", "-z"], cwd=root, text=True).split("\0"):
    path = Path(name)
    # An eval task's repository is a fixture the model edits, not our source; its check counts.
    if (not name or path.parts[0] in ("planning", "docs") or path.suffix not in COMMENTS
            or path.parts[0] == "eval" and path.parts[3:4] == ("repo",)):
        continue
    text = (root / path).read_text()
    if path.parts[0] != "flows" and ("tests" in path.parts or path.name.startswith("test_")
                                     or ".test." in name or ".spec." in name
                                     or path.suffix == ".rs" and re.search(r"^\s*#\[(\w+::)?test\b", text, re.M)):
        failures.append(f"test outside flows {name}")
    row = modules.get("/".join(path.parts[:2]) if path.parts[0] == "src" else path.parts[0])
    if row not in counts:
        failures.append(f"unmapped source {name}")
        continue
    counts[row] += sum(bool(line.strip()) and not line.lstrip().startswith(COMMENTS[path.suffix])
                       for line in text.splitlines())
counts["Total"] = sum(counts.values())
for name, limit in limits.items():
    print(f"{'PASS' if counts[name] <= limit else 'FAIL'} size {name}: {counts[name]:,}/{limit:,}")
    if counts[name] > limit:
        failures.append(f"{name} exceeds its line budget")
for failure in failures:
    print(f"FAIL size: {failure}")
sys.exit(bool(failures))
