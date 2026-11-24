#!/usr/bin/env sh
set -eu

runs=${MA2A_E2E_RUNS:-3}
output=${MA2A_E2E_OUTPUT:-target/e2e-evidence}
# The Phase One evidence scenarios. The jq program below names each of them;
# the number of tests, by contrast, is read from nextest itself rather than
# maintained here, so adding a test cannot silently fail the harness.
expected_records=13

# A run count is a decimal integer from 1 to 9999 without leading zeros; an empty value means
# the default. Anything else, `00` and numbers too long for shell arithmetic included, is
# refused before the output directory is touched.
case "$runs" in
  ''|*[!0-9]*|0*|?????*)
    printf 'MA2A_E2E_RUNS must be an integer from 1 to 9999 without leading zeros\n' >&2; exit 2 ;;
esac
# Trailing separators and `.` components are dropped, so `dir`, `dir/` and `dir/.` name the same
# directory in every check and share one claim. A name ending in `.` or `..` has no sibling to
# hold that claim and is refused.
while :; do
  case "$output" in
    */.) output=${output%/.} ;;
    ?*/) output=${output%/} ;;
    *) break ;;
  esac
done
case "$output" in
  .|..|*/..)
    printf 'MA2A_E2E_OUTPUT must name the evidence directory itself, not . or ..\n' >&2; exit 2 ;;
esac
command -v jq >/dev/null 2>&1 || { printf 'jq is required\n' >&2; exit 2; }
# A run claims its output directory before looking into it and keeps the claim until its
# evidence is checked, so two runs given one directory never remove or mix each other's files.
# Runs given different directories still take turns on the shared JUnit report below.
claim="$output.lock"
claimed=false
held=false
release() { if [ "$held" = true ]; then rmdir "$lock" 2>/dev/null || true; held=false; fi; }
cleanup() {
  release
  if [ "$claimed" = true ]; then rmdir "$claim" 2>/dev/null || true; claimed=false; fi
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT; trap 'cleanup; exit 143' TERM; trap 'cleanup; exit 129' HUP
mkdir -p "$(dirname "$output")"
if ! mkdir "$claim" 2>/dev/null; then
  printf '%s is in use by another E2E run; remove %s if none is active\n' "$output" "$claim" >&2
  exit 2
fi
claimed=true
# An existing output directory is reused only when it is empty, or carries this script's marker
# and holds nothing but files this script writes. Exactly the files found in that check are
# removed, so a mistyped MA2A_E2E_OUTPUT, a file someone added to an evidence directory, or one
# that appears while the check runs is never deleted.
marker=.ma2a-e2e-evidence
# Whether the file name $1 is one this script writes: a fixed name, or run-N plus a suffix,
# where N is digits only.
ours() {
  case "$1" in
    "$marker"|inventory.json|inventory.txt|inventory.txt.unsorted|summary.txt|provenance.txt)
      return 0 ;;
    run-*) ;;
    *) return 1 ;;
  esac
  rest=${1#run-}
  number=${rest%%.*}
  case "$number" in ''|*[!0-9]*) return 1 ;; esac
  case "${rest#"$number"}" in
    .log|.jsonl|.junit.xml|.tests.txt|.tests.txt.unsorted|.sha256) return 0 ;;
  esac
  return 1
}
if [ -e "$output" ] || [ -L "$output" ]; then
  reusable=false
  if [ -d "$output" ] && [ ! -L "$output" ]; then
    entries=$(find "$output" -mindepth 1 -maxdepth 1 -print)
    reusable=true
    if [ -n "$entries" ] && [ ! -f "$output/$marker" ]; then
      reusable=false
    fi
    saved_ifs=$IFS
    IFS='
'
    set -f
    for entry in $entries; do
      # A name holding a newline splits into lines that lack the directory prefix.
      case "$entry" in "$output"/*) ;; *) reusable=false; break ;; esac
      if [ ! -f "$entry" ] || [ -L "$entry" ] || ! ours "${entry#"$output"/}"; then
        reusable=false
        break
      fi
    done
    if [ "$reusable" = true ]; then
      for entry in $entries; do
        rm -f -- "$entry"
      done
    fi
    set +f
    IFS=$saved_ifs
  fi
  if [ "$reusable" != true ]; then
    printf '%s is not an E2E evidence directory written by this script; refusing to reuse it\n' \
      "$output" >&2
    exit 2
  fi
fi
mkdir -p "$output"
: > "$output/$marker"
# nextest writes the JUnit report to one fixed place under the target directory, which may
# be set elsewhere, so it is read from Cargo. Runs sharing that directory take turns from
# removing the report to copying it, so each run copies its own.
metadata_json=$(cargo metadata --locked --format-version 1 --no-deps)
target=$(printf '%s\n' "$metadata_json" | jq -er '.target_directory' | tr -d '\r')
junit="$target/nextest/ci/junit.xml"
lock="$target/ma2a-e2e-junit.lock"
acquire() {
  mkdir -p "$target"
  waited=0
  until mkdir "$lock" 2>/dev/null; do
    if [ "$waited" -eq 30 ]; then
      printf 'waiting for %s; remove it if no other E2E run is active\n' "$lock" >&2
    fi
    if [ "$waited" -ge 600 ]; then
      printf 'timed out waiting for %s\n' "$lock" >&2
      exit 1
    fi
    waited=$((waited + 1))
    sleep 1
  done
  held=true
}
commit=$(GIT_MASTER=1 git rev-parse HEAD)
rustc_version=$(rustc --version)
nextest_raw=$(cargo nextest --version)
nextest_version=$(printf '%s\n' "$nextest_raw" | sed -n '1p')
# The authoritative inventory: every test nextest would run in this binary. An
# ignored test is refused outright, because nextest does not run it and would
# report the remainder as a clean pass.
inventory="$output/inventory.txt"
cargo nextest list -p ma2a-app --test e2e --profile ci --message-format json > "$output/inventory.json"
jq -e -r '
  [."rust-suites"[].testcases | to_entries[]] as $cases |
  if ($cases | length) == 0 then error("nextest listed no E2E tests")
  elif any($cases[]; .value.ignored) then error("an E2E test is ignored")
  else $cases[].key end
' "$output/inventory.json" | tr -d '\r' > "$inventory.unsorted"
LC_ALL=C sort "$inventory.unsorted" > "$inventory"
rm -f "$inventory.unsorted"
test -z "$(uniq -d "$inventory")" || { printf 'duplicate E2E test names\n' >&2; exit 1; }
expected_tests=$(wc -l < "$inventory" | tr -d ' ')
printf 'runs=%s expected_tests=%s expected_records=%s\n' \
  "$runs" "$expected_tests" "$expected_records" > "$output/summary.txt"
printf 'source_commit=%s\nrustc=%s\nnextest=%s\n' \
  "$commit" "$rustc_version" "$nextest_version" > "$output/provenance.txt"

run=1
while [ "$run" -le "$runs" ]; do
  log="$output/run-$run.log"
  evidence="$output/run-$run.jsonl"
  report="$output/run-$run.junit.xml"
  printf 'run=%s command=cargo nextest run -p ma2a-app --test e2e --profile ci --success-output final --no-output-indent --color never\n' "$run" >> "$output/summary.txt"
  acquire
  rm -f "$junit"
  set +e
  cargo nextest run -p ma2a-app --test e2e --profile ci --success-output final --no-output-indent --color never > "$log" 2>&1
  nextest_status=$?
  set -e
  if [ -f "$junit" ]; then
    cp "$junit" "$report"
  fi
  release
  if [ "$nextest_status" -ne 0 ]; then
    exit "$nextest_status"
  fi
  grep -E '^\{' "$log" > "$evidence"
  root="$(sed -n '2p' "$report")"
  case "$root" in
    *"tests=\"$expected_tests\""*"skipped=\"0\""*"failures=\"0\""*"errors=\"0\""*) ;;
    *) printf 'invalid JUnit totals in %s\n' "$report" >&2; exit 1 ;;
  esac
  # Not only as many tests as listed, but exactly the listed ones, once each.
  executed="$output/run-$run.tests.txt"
  sed -n 's/^ *<testcase name="\([^"]*\)".*/\1/p' "$report" > "$executed.unsorted"
  LC_ALL=C sort "$executed.unsorted" > "$executed"
  rm -f "$executed.unsorted"
  if ! cmp -s "$inventory" "$executed"; then
    printf 'executed tests differ from the nextest inventory in %s\n' "$report" >&2
    diff "$inventory" "$executed" >&2 || true
    exit 1
  fi
  test "$(wc -l < "$evidence")" -eq "$expected_records"
  jq -s -e '
    def exact($names): keys == ($names | sort);
    def ids: (.endpoint_ids | type == "object" and length > 0 and all(.[]; type == "string" and length > 0));
    def strings: type == "array" and all(.[]; type == "string" and length > 0);
    def numbers: type == "array" and length > 0 and all(.[]; type == "number");
    length == $expected and
    ([.[].scenario] | sort) == (["A","B","C","D","E","F","control-sync-active-dial","persistent-identity-restart","relay-outage-recovery","signer-claim-mismatch","slow-ipc-frame","sse-overflow-receiver-cleanup","store-process-kill-recovery"] | sort) and
    (group_by(.scenario) | all(length == 1)) and
    all(.[]; ids) and
    all(.[];
      if .scenario == "A" then
        exact(["connection_remote","endpoint_ids","excluded_space_relay","iroh_observed_effective_home","iroh_observed_path","iroh_selected_relay_path","reachability_state","record_sequences","scenario","supplied_relay_candidates"]) and
        (.record_sequences | numbers) and (.supplied_relay_candidates | strings)
      elif .scenario == "B" then
        exact(["endpoint_ids","home_connected","iroh_observed_effective_home","iroh_observed_path","reachability_state","record_sequences","scenario","supplied_relay_candidates"]) and
        .home_connected == true and .record_sequences == [] and (.supplied_relay_candidates | strings)
      elif .scenario == "C" then
        exact(["endpoint_ids","iroh_observed_effective_home","iroh_observed_path","public_fallback_enabled","reachability_state","record_sequences","scenario","selected_relay_paths","space_ids","supplied_relay_candidates"]) and
        .public_fallback_enabled == false and .record_sequences == [] and
        (.selected_relay_paths | strings) and (.space_ids | strings) and (.supplied_relay_candidates | strings)
      elif .scenario == "D" then
        exact(["endpoint_ids","iroh_observed_effective_home","iroh_observed_path","reachability_state","record_sequences","scenario","supplied_relay_candidates"]) and
        .reachability_state == "DegradedNoCommonHome" and .record_sequences == [] and
        .supplied_relay_candidates == [] and .iroh_observed_effective_home == null and .iroh_observed_path == null
      elif .scenario == "E" then
        exact(["connection_remote","endpoint_ids","fresh_signed_target_sequence","iroh_observed_effective_home","iroh_observed_path","reachability_state","record_sequences","scenario","selected_relay_path","supplied_relay_candidates","transient_route_established_before_restart"]) and
        .transient_route_established_before_restart == true and (.fresh_signed_target_sequence | type == "number") and
        (.record_sequences | numbers) and (.supplied_relay_candidates | strings)
      elif .scenario == "F" then
        exact(["connection_observations","control_sync_propagated","durable_received_high_water","endpoint_ids","iroh_observed_effective_home","iroh_observed_path","observed_home_after","observed_home_before","owner_runtime_endpoint_id","owner_runtime_high_water","reachability_state","record_sequences","record_sequences_after","record_sequences_before","scenario","supplied_relay_candidates","sync_revision"]) and
        .control_sync_propagated == true and (.durable_received_high_water | numbers) and
        (.owner_runtime_high_water | numbers) and (.record_sequences_after | numbers) and
        (.record_sequences_before | numbers) and .record_sequences == .record_sequences_after and
        .owner_runtime_high_water == .durable_received_high_water and
        ([.record_sequences_after, .owner_runtime_high_water] | transpose | all(.[]; .[1] == .[0] + 1)) and
        (.supplied_relay_candidates | strings)
      elif .scenario == "control-sync-active-dial" then
        exact(["address_high_water","endpoint_ids","manifest_high_water","private_space_leaked","relay_high_water","scenario"]) and
        .private_space_leaked == false and (.address_high_water | numbers) and
        (.manifest_high_water | numbers) and (.relay_high_water | numbers)
      elif .scenario == "persistent-identity-restart" then
        exact(["control_high_water_after","control_high_water_before","endpoint_ids","scenario"]) and
        .endpoint_ids.before == .endpoint_ids.after and
        .control_high_water_before == .control_high_water_after and
        (.control_high_water_before | type == "array" and length == 2 and all(.[];
          exact(["address_hash","address_sequence","manifest_generation","manifest_hash","relay_hash","relay_sequence","space_id"]) and
          (.manifest_generation | type == "number" and . > 0) and
          (.address_sequence | type == "number" and . > 0) and
          (.relay_sequence | type == "number" and . > 0) and
          (.manifest_hash | type == "string" and length == 64) and
          (.address_hash | type == "string" and length == 64) and
          (.relay_hash | type == "string" and length == 64)))
      elif .scenario == "slow-ipc-frame" then
        exact(["bounded_rejection","declared_payload_bytes","endpoint_ids","partial_payload_bytes","scenario","unrelated_client_progressed"]) and
        .bounded_rejection == true and .unrelated_client_progressed == true and
        .partial_payload_bytes == 1 and .declared_payload_bytes > .partial_payload_bytes
      elif .scenario == "store-process-kill-recovery" then
        exact(["all_or_none","endpoint_ids","integrity_check","revision","scenario","space_rows"]) and
        .all_or_none == true and .integrity_check == "ok" and .revision == 0 and .space_rows == 0
      elif .scenario == "sse-overflow-receiver-cleanup" then
        exact(["dropped_receiver_permit_reused","endpoint_ids","queued_revisions","resync_required","scenario"]) and
        .dropped_receiver_permit_reused == true and .resync_required == true and .queued_revisions > 8
      elif .scenario == "relay-outage-recovery" then
        exact(["endpoint_ids","failed_relay","public_internet_used","recovered","recovered_relay","scenario"]) and
        .recovered == true and .public_internet_used == false and
        (.failed_relay | type == "string" and length > 0) and
        (.recovered_relay | type == "string" and length > 0) and .failed_relay != .recovered_relay
      elif .scenario == "signer-claim-mismatch" then
        exact(["accepted","address_high_water","endpoint_ids","relay_high_water","revision","scenario"]) and
        .accepted == false
      else false end)
  ' --argjson expected "$expected_records" "$evidence" >/dev/null
  records=$(wc -l < "$evidence" | tr -d ' ')
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$log" "$report" "$evidence" > "$output/run-$run.sha256" && sha256sum -c "$output/run-$run.sha256" >/dev/null
  else
    shasum -a 256 "$log" "$report" "$evidence" > "$output/run-$run.sha256" && shasum -a 256 -c "$output/run-$run.sha256" >/dev/null
  fi
  printf 'run=%s exit=0 tests=%s records=%s junit=%s evidence=%s\n' \
    "$run" "$expected_tests" "$records" "$report" "$evidence" >> "$output/summary.txt"
  run=$((run + 1))
done

test "$(grep -c '^run=[0-9][0-9]* exit=0 ' "$output/summary.txt")" -eq "$runs"
test "$(find "$output" -maxdepth 1 -name 'run-*.junit.xml' -type f | wc -l)" -eq "$runs"
test "$(find "$output" -maxdepth 1 -name 'run-*.jsonl' -type f | wc -l)" -eq "$runs"
test "$(find "$output" -maxdepth 1 -name 'run-*.sha256' -type f | wc -l)" -eq "$runs"
