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
CSS
cat > "$temporary/app.js" <<'JS'
import "/module.js";
new URL("/worker.js", import.meta.url);
new URL("/api/v1/snapshot", window.location.origin);
fetch("https://api.invalid/data");
new WebSocket("wss://socket.invalid/");
new EventSource("//events.invalid/stream");
new Worker("https://worker.invalid/worker.js");
new SharedWorker("//worker.invalid/shared.js");
navigator.sendBeacon("https://beacon.invalid/", "body");
xhr.open("GET", "https://xhr.invalid/data");
JS

html=$(./scripts/extract-runtime-references.pl html "$temporary/index.html")
css=$(./scripts/extract-runtime-references.pl css "$temporary/app.css")
javascript=$(./scripts/extract-runtime-references.pl js "$temporary/app.js")

grep -F $'asset\t/app.js' <<<"$html" >/dev/null
grep -F $'asset\thttps://cdn.invalid/large.png' <<<"$html" >/dev/null
grep -F $'asset\t//cdn.invalid/background.png' <<<"$css" >/dev/null
grep -F $'asset\t/module.js' <<<"$javascript" >/dev/null
grep -F $'asset\t/worker.js' <<<"$javascript" >/dev/null
for reference in \
  /api/v1/snapshot \
  https://api.invalid/data \
  wss://socket.invalid/ \
  //events.invalid/stream \
  https://worker.invalid/worker.js \
  //worker.invalid/shared.js \
  https://beacon.invalid/ \
  https://xhr.invalid/data
do
  grep -F $'network\t'"$reference" <<<"$javascript" >/dev/null
done

printf 'runtime reference scanner probes passed\n'
