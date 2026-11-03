#!/usr/bin/env python3
"""Apply horizon-mixins bundles (native-mods/<id>/horizon-mixin.toml) to this checkout.

Stages the committed source (git archive HEAD), runs `horizon-mixins plan` then `apply`,
and copies the changed files back over the working tree, so the normal Android build
(tools/android_build.py) compiles the modded game. Run it in CI; it edits tracked files.
"""
import argparse, filecmp, re, shutil, subprocess, sys, tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--mods", type=Path, default=ROOT / "native-mods")
    ap.add_argument("--cli", type=Path, required=True, help="path to the horizon-mixins binary")
    a = ap.parse_args()
    bundles = sorted(p.parent for p in a.mods.glob("*/horizon-mixin.toml"))
    if not bundles:
        sys.exit(f"no bundles in {a.mods}")
    revs = {re.search(r'horizon_revision\s*=\s*"([0-9a-f]+)"', (b / "horizon-mixin.toml").read_text()).group(1) for b in bundles}
    work = ROOT / ".local/native"
    shutil.rmtree(work, ignore_errors=True)
    original, modded = work / "original", work / "modded"
    original.mkdir(parents=True)
    tar = subprocess.run(["git", "-C", str(ROOT), "archive", "--format=tar", "HEAD"], check=True, capture_output=True).stdout
    tmp = work / "src.tar"; tmp.write_bytes(tar)
    with tarfile.open(tmp) as t:
        t.extractall(original, filter="data")
    for f in original.rglob("*"):  # the CLI wants a symlink-free snapshot
        if f.is_symlink():
            data = f.read_bytes(); f.unlink(); f.write_bytes(data)
    # This fork is newer than the revision the bundles pin; the CLI still fails on any missing hook target.
    (original / ".horizon-revision").write_text(sorted(revs)[0] + "\n")
    base = ["--workspace", str(original), "--mods", str(a.mods)]
    subprocess.run([str(a.cli), "plan", *base, "--out", str(work / "plan.json")], check=True)
    out = subprocess.run([str(a.cli), "apply", *base, "--out", str(modded)], check=True, capture_output=True, text=True)
    print(out.stdout)
    changed = 0
    for f in sorted(modded.rglob("*")):
        rel = f.relative_to(modded)
        if not f.is_file() or rel.parts[0].startswith(".horizon"):
            continue
        dest = ROOT / rel
        if not dest.exists() or not filecmp.cmp(f, dest, shallow=False):
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(f, dest); changed += 1
    print(f"native mods applied: {len(bundles)} bundle(s), {changed} file(s) changed")

if __name__ == "__main__":
    main()
