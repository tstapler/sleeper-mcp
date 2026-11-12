#!/usr/bin/env bash
# Fails when the Bazel build and the Cargo manifests have drifted apart. Cargo.toml/Cargo.lock stay the source of
# truth; Bazel derives from them, so every check here reads both sides. Fails closed: a missing file or tool is a FAIL.
#   check-bazel-cargo-sync.sh [ROOT]   run all checks (ROOT defaults to the current directory)
#   check-bazel-cargo-sync.sh --selftest
# Checks:
#   toolchain  rust-toolchain.toml channel == RUST_VERSION in MODULE.bazel, and .bazelversion exists
#   version    Cargo.toml package version == CARGO_PKG_VERSION in BUILD.bazel
#   lock       packages in Cargo.lock == crates in cargo-bazel-lock.json (name + version)
#   targets    every tests/*.rs is picked up by the BUILD.bazel glob, and every [[bin]] path is named in BUILD.bazel
#   bazel      (DC_SYNC_BAZEL=0 skips) MODULE.bazel.lock is current, cargo-bazel-lock.json is not stale
#              (re-pinned in place; the committed file must not change)
set -euo pipefail

die() { echo "FAIL: $*"; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "required tool '$1' not found on PATH (the check cannot run, so it does not pass)"; }

static_checks() {
  python3 -I - "$1" <<'PY'
import json, re, sys, tomllib
from pathlib import Path

root = Path(sys.argv[1])
bad = []

def fail(msg): bad.append(msg)

def read(p):
    try: return (root / p).read_text()
    except OSError: fail(f"missing file {p}"); return ""

def toml(p):
    try: return tomllib.loads(read(p))
    except tomllib.TOMLDecodeError as e: fail(f"{p}: {e}"); return {}

cargo = toml("Cargo.toml")
module = read("MODULE.bazel")
build = read("BUILD.bazel")

chan = toml("rust-toolchain.toml").get("toolchain", {}).get("channel", "")
m = re.search(r'^RUST_VERSION *= *"([^"]+)"', module, re.M)
if not m: fail('MODULE.bazel has no RUST_VERSION = "x.y.z"')
elif chan != m.group(1): fail(f"rust-toolchain.toml channel {chan!r} != MODULE.bazel RUST_VERSION {m.group(1)!r}")
if not re.fullmatch(r"\d+\.\d+\.\d+", chan): fail(f"rust-toolchain.toml channel {chan!r} is not an explicit x.y.z version")
if not (root / ".bazelversion").is_file(): fail(".bazelversion missing")

ver = cargo.get("package", {}).get("version")
mv = re.search(r'CARGO_ENV *= *\{"CARGO_PKG_VERSION": *"([^"]+)"', build)
if not ver or not mv or mv.group(1) != ver:
    fail(f"Cargo.toml version {ver!r} != BUILD.bazel CARGO_PKG_VERSION {mv.group(1) if mv else None!r}")

lock = toml("Cargo.lock").get("package", [])
want = {f'{p["name"]} {p["version"]}' for p in lock}
try: have = set(json.loads(read("cargo-bazel-lock.json")).get("crates", {}))
except json.JSONDecodeError as e: fail(f"cargo-bazel-lock.json: {e}"); have = set()
if not want: fail("Cargo.lock lists no packages")
for k in sorted(want - have): fail(f"Cargo.lock has {k} but cargo-bazel-lock.json does not (repin: CARGO_BAZEL_REPIN=1 scripts/bazel.sh build --nobuild //...)")
for k in sorted(have - want): fail(f"cargo-bazel-lock.json has {k} but Cargo.lock does not (repin: CARGO_BAZEL_REPIN=1 scripts/bazel.sh build --nobuild //...)")

if 'glob(["tests/*.rs"])' not in build: fail('BUILD.bazel no longer globs ["tests/*.rs"]')
for sub in sorted((root / "tests").glob("*/main.rs")):
    fail(f"{sub.relative_to(root)} is a Cargo test target (tests/<dir>/main.rs) that the BUILD.bazel glob does not cover")
for t in cargo.get("bin", []):
    p = t.get("path", "")
    if p and p not in build: fail(f"Cargo.toml [[bin]] {t.get('name')} ({p}) has no Bazel target")
for sec in ("bench", "test", "example"):
    for t in cargo.get(sec, []):
        fail(f"Cargo.toml [[{sec}]] {t.get('name')} has no Bazel target")
if (root / "build.rs").is_file(): fail("build.rs exists but BUILD.bazel does not build it")

for b in bad: print("FAIL:", b)
sys.exit(1 if bad else 0)
PY
}

bazel_checks() {
  local root=$1 bz=${DC_BAZEL:-$1/scripts/bazel.sh} rc=0 before after
  [ -x "$bz" ] || command -v "$bz" >/dev/null 2>&1 || { echo "FAIL: $bz not found (DC_SYNC_BAZEL=0 skips the Bazel checks)"; return 1; }
  (cd "$root" && "$bz" build --nobuild --lockfile_mode=error //... >/dev/null 2>&1) || { echo "FAIL: MODULE.bazel.lock is out of date (run: scripts/bazel.sh build --nobuild --lockfile_mode=update //...) or bazel failed"; rc=1; }
  # A Cargo.toml dependency change leaves cargo-bazel-lock.json stale and Bazel then fails with "cannot find crate":
  # re-pin in place and fail if the committed lock changes.
  before=$(sha256sum "$root/cargo-bazel-lock.json" 2>/dev/null | cut -d' ' -f1)
  (cd "$root" && CARGO_BAZEL_REPIN=1 "$bz" build --nobuild //... >/dev/null 2>&1)
  after=$(sha256sum "$root/cargo-bazel-lock.json" 2>/dev/null | cut -d' ' -f1)
  if [ "$before" != "$after" ]; then
    echo "FAIL: cargo-bazel-lock.json was stale: a Cargo.toml dependency changed (it is now re-pinned in the working tree: review and commit it)"; rc=1
  fi
  return "$rc"
}

run() {
  local root=${1:-.} rc=0
  need python3
  root=$(cd "$root" && pwd)
  static_checks "$root" || rc=1
  if [ "${DC_SYNC_BAZEL:-1}" != 0 ]; then bazel_checks "$root" || rc=1; fi
  [ "$rc" -eq 0 ] && echo "ok: Cargo and Bazel agree (toolchain, version, lock, targets$([ "${DC_SYNC_BAZEL:-1}" != 0 ] && echo ', bazel'))"
  return "$rc"
}

make_good() {
  local d=$1
  mkdir -p "$d/tests/sub" "$d/src/bin"
  printf '[package]\nname = "a"\nversion = "0.1.0"\n[[bin]]\nname = "a"\npath = "src/bin/a.rs"\n' > "$d/Cargo.toml"
  printf 'RUST_VERSION = "1.98.1"\n' > "$d/MODULE.bazel"
  printf '[toolchain]\nchannel = "1.98.1"\n' > "$d/rust-toolchain.toml"
  echo 9.3.2 > "$d/.bazelversion"
  printf '[[package]]\nname = "a"\nversion = "0.1.0"\n[[package]]\nname = "serde"\nversion = "1.0.0"\nsource = "registry+x"\n' > "$d/Cargo.lock"
  echo '{"crates":{"a 0.1.0":{},"serde 1.0.0":{}}}' > "$d/cargo-bazel-lock.json"
  printf 'CARGO_ENV = {"CARGO_PKG_VERSION": "0.1.0"}\nsrcs = ["src/bin/a.rs"]\nglob(["tests/*.rs"])\n' > "$d/BUILD.bazel"
  echo '' > "$d/tests/one.rs"
}

selftest() {
  need python3
  local n=0 bad=0 d out rc
  t=$(mktemp -d); trap 'rm -rf "$t"' EXIT INT TERM
  export DC_SYNC_BAZEL=0
  expect() {  # want(pass|fail) name pattern mutation-command...
    local want=$1 name=$2 pat=$3; shift 3
    d=$t/r$n; make_good "$d"; (cd "$d" && eval "$*")
    rc=0; out=$(run "$d" 2>&1) || rc=$?
    if { [ "$want" = pass ] && [ $rc -eq 0 ]; } || { [ "$want" = fail ] && [ $rc -eq 1 ]; }; then
      if echo "$out" | grep -q -- "$pat"; then echo "ok: $name"; else echo "SELFTEST BAD: $name (output lacks '$pat'): $out"; bad=$((bad+1)); fi
    else echo "SELFTEST BAD: $name (rc=$rc): $out"; bad=$((bad+1)); fi
    n=$((n+1))
  }
  expect pass "a consistent repo passes" "ok: Cargo and Bazel agree" true
  expect fail "toolchain version drift" "rust-toolchain.toml channel" "sed -i 's/1.98.1/1.99.0/' MODULE.bazel"
  expect fail "a floating toolchain channel" "not an explicit x.y.z" "printf '[toolchain]\nchannel = \"stable\"\n' > rust-toolchain.toml; sed -i 's/1.98.1/stable/' MODULE.bazel"
  expect fail "a missing .bazelversion" ".bazelversion missing" "rm .bazelversion"
  expect fail "a version bump in Cargo.toml alone" "BUILD.bazel CARGO_PKG_VERSION" "sed -i 's/^version = \"0.1.0\"/version = \"0.2.0\"/' Cargo.toml"
  expect fail "a package missing from the crate lock" "Cargo.lock has serde 1.0.0" "echo '{\"crates\":{\"a 0.1.0\":{}}}' > cargo-bazel-lock.json"
  expect fail "a stale package in the crate lock" "cargo-bazel-lock.json has old 0.9.0" "echo '{\"crates\":{\"a 0.1.0\":{},\"serde 1.0.0\":{},\"old 0.9.0\":{}}}' > cargo-bazel-lock.json"
  expect fail "a bin nobody built" "has no Bazel target" "printf '[[bin]]\nname = \"b\"\npath = \"src/bin/b.rs\"\n' >> Cargo.toml"
  expect fail "tests/<dir>/main.rs the glob would miss" "tests/sub/main.rs is a Cargo test target" "echo '' > tests/sub/main.rs"
  expect fail "a build.rs nobody built" "build.rs exists" "echo 'fn main() {}' > build.rs"
  expect fail "a missing Cargo.lock" "missing file Cargo.lock" "rm Cargo.lock"
  echo "selftest: $((n - bad)) ok, $bad bad"; [ "$bad" -eq 0 ]
}

case "${1:-}" in
  --selftest) selftest ;;
  -h | --help) sed -n '2,13p' "$0" ;;
  *) run "${1:-.}" ;;
esac
