#!/usr/bin/env bash
# Smoke-test a built Sieve.app without the dev toolchain (no Homebrew, no cargo on PATH).
#
# Usage: scripts/bundle-smoke.sh <sample_dir> [work_dir]
#   sample_dir  a small folder of COPIED samples (never ~/Pictures)
#   work_dir    default: test-data/bundle-smoke
#
# 1. Checks the bundle: no /opt/homebrew or /usr/local references in any Mach-O, codesign valid.
# 2. Launches Contents/MacOS/sieve with a scratch catalog/cache for a few seconds: startup +
#    migrations, and lists the dylibs mapped into the process (`lsof`; the hardened runtime
#    ignores DYLD_PRINT_LIBRARIES) to prove each one comes from Contents/Frameworks.
# 3. Copies the .app, drops the `bundle_smoke` example binary into its Contents/MacOS (so it
#    resolves @rpath and models exactly like `sieve`), and runs import -> thumbnails -> culling
#    analysis -> render -> JPEG export with PATH=/usr/bin:/bin and DYLD_* cleared.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SAMPLES="${1:?usage: bundle-smoke.sh <sample_dir> [work_dir]}"
WORK="${2:-$ROOT/test-data/bundle-smoke}"
TARGET="$ROOT/src-tauri/target/release"
APP="$TARGET/bundle/macos/Sieve.app"
SMOKE_BIN="$TARGET/examples/bundle_smoke"

case "$(cd "$SAMPLES" && pwd)" in
  "$HOME/Pictures"*) echo "refusing to use ~/Pictures; copy samples into test-data first" >&2; exit 1 ;;
esac
[[ -d "$APP" ]] || { echo "missing $APP (run pnpm tauri build)" >&2; exit 1; }
[[ -x "$SMOKE_BIN" ]] || { echo "missing $SMOKE_BIN (cargo build --release --example bundle_smoke)" >&2; exit 1; }

mkdir -p "$WORK"
CLEAN_ENV=(env -i HOME="$HOME" PATH=/usr/bin:/bin:/usr/sbin:/sbin TMPDIR="${TMPDIR:-/tmp}")

echo "== bundle: $APP"
du -sh "$APP" | awk '{print "app size   " $1}'
DMG="$(ls "$TARGET"/bundle/dmg/*.dmg 2>/dev/null | head -1 || true)"
[[ -n "$DMG" ]] && du -h "$DMG" | awk '{print "dmg size   " $1}'
bad=0
while IFS= read -r f; do
  if file "$f" | grep -q "Mach-O"; then
    if otool -L "$f" | tail -n +2 | grep -E "/opt/homebrew|/usr/local"; then
      echo "FAIL Homebrew reference in $f"; bad=1
    fi
  fi
done < <(find "$APP/Contents" -type f)
[[ $bad -eq 0 ]] && echo "ok   no /opt/homebrew or /usr/local load commands"
codesign --verify --deep --strict "$APP" && echo "ok   codesign --verify --deep --strict"
echo "Frameworks:"; ls -1 "$APP/Contents/Frameworks" | sed 's/^/  /'
echo "models:"; ls -1 "$APP/Contents/Resources/models" | sed 's/^/  /'

echo "== launch Contents/MacOS/sieve (startup + migrations)"
rm -rf "$WORK/app"; mkdir -p "$WORK/app"
start=$(/usr/bin/perl -MTime::HiRes=time -e 'printf "%.3f", time')
"${CLEAN_ENV[@]}" SIEVE_CATALOG="$WORK/app/catalog.sqlite" SIEVE_CACHE="$WORK/app/cache" \
  SIEVE_LUTS="$WORK/app/luts" "$APP/Contents/MacOS/sieve" > "$WORK/app/stdout.log" 2> "$WORK/app/stderr.log" &
pid=$!
# Launch time = until the catalog (opened in setup, before the window) has its migrations.
for _ in $(seq 1 200); do
  if [[ -f "$WORK/app/catalog.sqlite" ]] && \
     [[ "$(sqlite3 "$WORK/app/catalog.sqlite" 'PRAGMA user_version' 2>/dev/null || echo 0)" != "0" ]]; then
    break
  fi
  sleep 0.05
done
ready=$(/usr/bin/perl -MTime::HiRes=time -e 'printf "%.3f", time')
sleep 4
if ! kill -0 "$pid" 2>/dev/null; then
  wait "$pid" || true; echo "FAIL sieve exited early"; tail -20 "$WORK/app/stderr.log"; exit 1
fi
echo "ok   running after 4 s (catalog ready in $(echo "$ready - $start" | bc) s, user_version $(sqlite3 "$WORK/app/catalog.sqlite" 'PRAGMA user_version'))"
lsof -p "$pid" -Fn | sed -n 's/^n//p' | grep '\.dylib$' | grep -vE '^/(System|usr/lib)/' | sort -u > "$WORK/app/dylibs.txt" || true
kill -TERM "$pid"; wait "$pid" 2>/dev/null || true
echo "dylibs mapped (non-system):"; sed 's/^/  /' "$WORK/app/dylibs.txt"
if grep -qE "/opt/homebrew|/usr/local" "$WORK/app/dylibs.txt"; then
  echo "FAIL a Homebrew dylib was loaded"; exit 1
fi
[[ "$(grep -c "$APP/Contents/Frameworks/" "$WORK/app/dylibs.txt")" -ge 5 ]] || { echo "FAIL bundled dylibs not mapped"; exit 1; }

echo "== pipeline smoke (bundle_smoke inside a copy of the bundle)"
COPY="$WORK/Sieve-smoke.app"
rm -rf "$COPY"; cp -R "$APP" "$COPY"
cp "$SMOKE_BIN" "$COPY/Contents/MacOS/bundle_smoke"
"${CLEAN_ENV[@]}" "$COPY/Contents/MacOS/bundle_smoke" "$SAMPLES" "$WORK/pipeline"
rm -rf "$COPY"
echo "== bundle smoke PASS"
