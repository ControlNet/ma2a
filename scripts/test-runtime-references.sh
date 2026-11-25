#!/usr/bin/env bash
set -euo pipefail

temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT

cat > "$temporary/index.html" <<'HTML'
<script src="/app.js"></script>
<img srcset="/small.png 1x, https://cdn.invalid/large.png 2x" src="/fallback.png">
HTML
cat > "$temporary/app.css" <<'CSS'
@import "/theme.css";
body { background: url(//cdn.invalid/background.png); }
a{background:url(data:image/gif;base64,AA==)}b{background:url(https://evil.invalid/track.png)}
c{background:url("https://evil.invalid/a'b.png")}
CSS
cat > "$temporary/app.js" <<'JS'
import "/module.js";
new URL("/worker.js", import.meta.url);
new URL("/api/v1/snapshot", window.location.origin);
fetch("https://api.invalid/data");
new WebSocket("ws://socket.invalid/plain");
new WebSocket("wss://socket.invalid/");
new EventSource("//events.invalid/stream");
new Worker("https://worker.invalid/worker.js");
new SharedWorker("//worker.invalid/shared.js");
navigator.sendBeacon("https://beacon.invalid/", "body");
xhr.open("GET", "https://xhr.invalid/data");
import{a as b}from"./chunk.js";export*from`./more.js`;
JS
# Minified shapes that only look like references: interface text, comments, and patterns.
cat > "$temporary/text.js" <<'JS'
const a=[`reply from `,b(c.target_endpoint_id)],d="import "+e+`:x`;
// import "https://comment.invalid/x.js"
/* fetch("https://comment.invalid/y") */
const f=/from "https:\/\/pattern.invalid"/.test(g),h=1/2/3,i=`${j(`from "a:b"`)} import 'c:d'`;
k.from("e:f");l.import("g:h");
JS

html=$(./scripts/extract-runtime-references.pl html "$temporary/index.html")
css=$(./scripts/extract-runtime-references.pl css "$temporary/app.css")
javascript=$(./scripts/extract-runtime-references.pl js "$temporary/app.js")

grep -F $'asset\t/app.js' <<<"$html" >/dev/null
grep -F $'asset\thttps://cdn.invalid/large.png' <<<"$html" >/dev/null
grep -F $'asset\t//cdn.invalid/background.png' <<<"$css" >/dev/null
grep -F $'asset\thttps://evil.invalid/track.png' <<<"$css" >/dev/null
grep -F $'asset\thttps://evil.invalid/a\'b.png' <<<"$css" >/dev/null
grep -F $'asset\t/module.js' <<<"$javascript" >/dev/null
grep -F $'asset\t/worker.js' <<<"$javascript" >/dev/null
grep -F $'asset\t./chunk.js' <<<"$javascript" >/dev/null
grep -F $'asset\t./more.js' <<<"$javascript" >/dev/null
text=$(./scripts/extract-runtime-references.pl js "$temporary/text.js")
[[ -z "$text" ]] || { printf 'string, comment, or pattern text read as a reference:\n%s\n' "$text" >&2; exit 1; }
for reference in \
  /api/v1/snapshot \
  https://api.invalid/data \
  ws://socket.invalid/plain \
  wss://socket.invalid/ \
  //events.invalid/stream \
  https://worker.invalid/worker.js \
  //worker.invalid/shared.js \
  https://beacon.invalid/ \
  https://xhr.invalid/data
do
  grep -F $'network\t'"$reference" <<<"$javascript" >/dev/null
done

# Fails when a built asset holds a reference that is not same-origin, or cannot be scanned.
scan_built() {
  local built=$1 references reference
  references=$(./scripts/extract-runtime-references.pl js "$built") || return 1
  [[ -n "$references" ]] || return 0
  while IFS=$'\t' read -r _ reference; do
    [[ "$reference" =~ ^\.{0,2}/[A-Za-z0-9._/-]*$ && "$reference" != //* ]] || {
      printf 'unexpected runtime reference in %s: %s\n' "$built" "$reference" >&2
      return 1
    }
  done <<<"$references"
}

# An asset the extractor cannot read fails the scan, even though it printed nothing.
if scan_built "$temporary/missing.js" 2>/dev/null; then
  printf 'an unreadable asset passed the scan\n' >&2
  exit 1
fi

# A protocol-relative reference names another host.
printf 'fetch("//evil.invalid/steal");\n' > "$temporary/protocol-relative.js"
if scan_built "$temporary/protocol-relative.js" 2>/dev/null; then
  printf 'a protocol-relative reference passed the scan\n' >&2
  exit 1
fi

# Browsers drop tabs and newlines anywhere in a URL and control characters around it, so a
# reference holding any of them is refused rather than checked as written.
printf '<img src="/\t/bad.test/x">\n' > "$temporary/tab.html"
printf '<img src="/\n/bad.test/x">\n' > "$temporary/newline.html"
printf 'fetch("\001//bad.test/x");\n' > "$temporary/control.js"
printf 'fetch(`http\n://bad.test/x`);\n' > "$temporary/template.js"
for probe in tab.html newline.html control.js template.js; do
  if ./scripts/extract-runtime-references.pl "${probe##*.}" "$temporary/$probe" >/dev/null 2>&1; then
    printf 'a reference with a control character in %s was extracted\n' "$probe" >&2
    exit 1
  fi
done

# The built console, when present, must carry only same-origin references.
shopt -s nullglob
for built in web/dist/assets/*.js; do
  scan_built "$built"
done

printf 'runtime reference scanner probes passed\n'
