#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  printf 'usage: %s ARCHIVE TARGET\n' "$0" >&2
  exit 2
fi

archive_directory=$(cd "$(dirname "$1")" && pwd)
archive="$archive_directory/$(basename "$1")"
target=$2
workspace=$(mktemp -d)
state_dir="$workspace/state"
binaries="$workspace/binaries"
request_log="$workspace/requests.log"
cookie_jar="$workspace/cookies"
original_cookie_jar="$workspace/original-cookies"
logout_cookie_jar="$workspace/logout-cookies"
login_body="$workspace/login.json"
wrong_login_body="$workspace/wrong-login.json"
login_response="$workspace/login-response.json"
csrf_config="$workspace/csrf.curl"
cleanup() {
  if [[ -n "${binary:-}" && -x "${binary:-}" ]]; then
    "$binary" --state-dir "$state_dir" stop >/dev/null 2>&1 || true
  fi
  rm -rf "$workspace"
}
trap cleanup EXIT

mkdir -m 700 "$state_dir"
touch "$request_log" "$cookie_jar" "$original_cookie_jar" "$logout_cookie_jar" "$login_body" "$wrong_login_body" "$login_response" "$csrf_config"
chmod 600 "$request_log" "$cookie_jar" "$original_cookie_jar" "$logout_cookie_jar" "$login_body" "$wrong_login_body" "$login_response" "$csrf_config"
case "$target" in
  *-pc-windows-msvc)
    unzip -q "$archive" -d "$workspace/extracted"
    find "$workspace/extracted" -type f -name ma2a.exe -print > "$binaries"
    ;;
  *-apple-darwin|*-unknown-linux-gnu)
    mkdir "$workspace/extracted"
    tar -xJf "$archive" -C "$workspace/extracted"
    find "$workspace/extracted" -type f -name ma2a -print > "$binaries"
    ;;
  *)
    printf 'unknown release target: %s\n' "$target" >&2
    exit 2
    ;;
esac

binary_count=$(wc -l < "$binaries")
[[ "$binary_count" -eq 1 ]] || {
  printf 'expected exactly one extracted ma2a binary, observed %s\n' "$binary_count" >&2
  exit 1
}
binary=$(sed -n '1p' "$binaries")
chmod 700 "$binary"

version=$($binary --version)
[[ "$version" == ma2a\ * ]] || { printf 'unexpected version output: %s\n' "$version" >&2; exit 1; }

"$binary" --state-dir "$state_dir" start >/dev/null
status=$($binary --state-dir "$state_dir" status --json)
jq -e '.result.payload.endpoint.online == true and .result.payload.spaces == []' <<<"$status" >/dev/null

smoke_credential="ma2a-release-smoke-password"
printf '%s\n%s\n' "$smoke_credential" "$smoke_credential" |
  MA2A_PASSWORD_STDIN=1 "$binary" --state-dir "$state_dir" ui init >/dev/null

created=$($binary --state-dir "$state_dir" space create --name release-smoke --json)
jq -e '.result.payload.space_id | type == "string"' <<<"$created" >/dev/null

url=$($binary --state-dir "$state_dir" ui start --json | jq -er .url)
[[ "$url" == http://127.0.0.1:* ]] || { printf 'unexpected Web URL: %s\n' "$url" >&2; exit 1; }

same_origin_headers=(--header "Origin: $url" --header "Sec-Fetch-Site: same-origin")
html_file="$workspace/index.html"
status=$(curl --silent --show-error --output "$html_file" --write-out '%{http_code}' "$url/login")
printf 'GET /login %s\n' "$status" >> "$request_log"
[[ "$status" == 200 ]] || { printf 'login page returned HTTP %s\n' "$status" >&2; exit 1; }

declare -a asset_queue=("index.html")
asset_root="$workspace/assets"
mkdir "$asset_root"
cp "$html_file" "$asset_root/index.html"
seen_assets="$workspace/seen-assets"
touch "$seen_assets"
while [[ ${#asset_queue[@]} -gt 0 ]]; do
  current=${asset_queue[0]}
  asset_queue=("${asset_queue[@]:1}")
  grep -Fqx "$current" "$seen_assets" && continue
  printf '%s\n' "$current" >> "$seen_assets"
  local_asset="$asset_root/$current"
  if [[ "$current" != index.html ]]; then
    mkdir -p "$(dirname "$local_asset")"
    response=$(curl --silent --show-error --output "$local_asset" --write-out '%{http_code}\t%{content_type}' "$url/$current")
    status=${response%%$'\t'*}
    content_type=${response#*$'\t'}
    printf 'GET /%s %s\n' "$current" "$status" >> "$request_log"
    [[ "$status" == 200 ]] || { printf 'embedded asset /%s returned HTTP %s\n' "$current" "$status" >&2; exit 1; }
    [[ -s "$local_asset" ]] || { printf 'embedded asset /%s is empty\n' "$current" >&2; exit 1; }
    case "$current" in
      *.html) expected_type='text/html' ;;
      *.js) expected_type='text/javascript' ;;
      *.css) expected_type='text/css' ;;
      *.svg) expected_type='image/svg+xml' ;;
      *) expected_type='application/octet-stream' ;;
    esac
    [[ "$content_type" == "$expected_type"* ]] || {
      printf 'embedded asset /%s returned unexpected content type %s\n' "$current" "$content_type" >&2
      exit 1
    }
  fi
  references="$workspace/references"
  : > "$references"
  case "$current" in
    *.html) ./scripts/extract-runtime-references.pl html "$local_asset" > "$references" ;;
    *.svg) ./scripts/extract-runtime-references.pl svg "$local_asset" > "$references" ;;
    *.css) ./scripts/extract-runtime-references.pl css "$local_asset" > "$references" ;;
    *.js) ./scripts/extract-runtime-references.pl js "$local_asset" > "$references" ;;
  esac
  while IFS=$'\t' read -r reference_kind reference; do
    reference=${reference%%#*}
    reference=${reference%%\?*}
    [[ -n "$reference" ]] || continue
    case "$reference" in
      http://*|https://*|//*|ws://*|wss://*)
        printf 'external runtime %s reference rejected in %s: %s\n' "$reference_kind" "$current" "$reference" >&2
        exit 1
        ;;
      data:*|blob:*|\#*) continue ;;
    esac
    if [[ "$reference_kind" == network ]]; then
      case "$reference" in
        /*) continue ;;
        *:*)
          printf 'unsupported runtime network scheme rejected in %s: %s\n' "$current" "$reference" >&2
          exit 1
          ;;
        *) continue ;;
      esac
    fi
    case "$reference" in
      /*) referenced_asset=${reference#/} ;;
      ./*) referenced_asset="$(dirname "$current")/${reference#./}" ;;
      *:*)
        printf 'unsupported runtime asset scheme rejected in %s: %s\n' "$current" "$reference" >&2
        exit 1
        ;;
      *) referenced_asset="$(dirname "$current")/$reference" ;;
    esac
    referenced_asset=${referenced_asset#./}
    [[ "$referenced_asset" != *../* && "$referenced_asset" != ../* ]] || {
      printf 'parent-relative runtime asset reference rejected in %s: %s\n' "$current" "$reference" >&2
      exit 1
    }
    asset_queue+=("$referenced_asset")
  done < "$references"
done

status=$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' "$url/api/v1/snapshot")
printf 'GET /api/v1/snapshot %s\n' "$status" >> "$request_log"
[[ "$status" == 401 ]] || { printf 'unauthenticated snapshot returned HTTP %s\n' "$status" >&2; exit 1; }

jq -n --arg password "not-the-release-smoke-password" '{password: $password}' > "$wrong_login_body"
status=$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
  "${same_origin_headers[@]}" --header 'Content-Type: application/json' \
  --data-binary "@$wrong_login_body" "$url/api/v1/web/auth/login")
printf 'POST /api/v1/web/auth/login %s\n' "$status" >> "$request_log"
[[ "$status" == 401 ]] || { printf 'invalid login returned HTTP %s\n' "$status" >&2; exit 1; }

jq -n --arg password "$smoke_credential" '{password: $password}' > "$login_body"
status=$(curl --silent --show-error --output "$login_response" --write-out '%{http_code}' \
  "${same_origin_headers[@]}" --header 'Content-Type: application/json' \
  --cookie-jar "$cookie_jar" --data-binary "@$login_body" "$url/api/v1/web/auth/login")
printf 'POST /api/v1/web/auth/login %s\n' "$status" >> "$request_log"
[[ "$status" == 200 ]] || { printf 'authenticated login returned HTTP %s\n' "$status" >&2; exit 1; }
jq -e '.csrf_token | select(test("^[0-9a-f]{64}$"))' "$login_response" >/dev/null
jq -er '"header = \"x-csrf-token: \(.csrf_token)\""' "$login_response" > "$csrf_config"
cp "$cookie_jar" "$original_cookie_jar"

snapshot_file="$workspace/snapshot.json"
status=$(curl --silent --show-error --output "$snapshot_file" --write-out '%{http_code}' \
  --cookie "$cookie_jar" "$url/api/v1/snapshot")
printf 'GET /api/v1/snapshot %s\n' "$status" >> "$request_log"
[[ "$status" == 200 ]] || { printf 'authenticated snapshot returned HTTP %s\n' "$status" >&2; exit 1; }
jq -e '.endpoint != null and (.spaces | length == 1)' "$snapshot_file" >/dev/null

status=$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
  "${same_origin_headers[@]}" --cookie "$original_cookie_jar" --cookie-jar "$logout_cookie_jar" \
  --config "$csrf_config" --request POST "$url/api/v1/web/auth/logout")
printf 'POST /api/v1/web/auth/logout %s\n' "$status" >> "$request_log"
[[ "$status" == 204 ]] || { printf 'logout returned HTTP %s\n' "$status" >&2; exit 1; }

status=$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
  --cookie "$original_cookie_jar" "$url/api/v1/snapshot")
printf 'GET /api/v1/snapshot %s\n' "$status" >> "$request_log"
[[ "$status" == 401 ]] || { printf 'logged-out snapshot returned HTTP %s\n' "$status" >&2; exit 1; }

$binary --state-dir "$state_dir" stop >/dev/null
binary=
while IFS= read -r request; do
  printf 'smoke request: %s\n' "$request"
done < "$request_log"
printf 'clean-room smoke passed for %s\n' "$target"
