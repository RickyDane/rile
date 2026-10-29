"""Bundle every suite's SVGs into suites.js for the review page (one request)."""
import json, os, sys
root = sys.argv[1]
names = [l.strip() for l in open(os.path.join(root, "names.txt")) if l.strip()]
suites = ["lucide", "bold", "sketch", "neon", "badge", "pixel"]
data = {s: {n: open(os.path.join(root, s, f"{n}.svg")).read().strip() for n in names} for s in suites}
with open(os.path.join(root, "suites.js"), "w") as f:
    f.write("window.ICON_NAMES = " + json.dumps(names) + ";\nwindow.ICON_SUITES = " + json.dumps(data) + ";\n")
print("bundled", len(suites), "x", len(names))
