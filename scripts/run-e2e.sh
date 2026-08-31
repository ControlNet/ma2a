#!/usr/bin/env sh
set -eu

runs=${MA2A_E2E_RUNS:-3}
output=${MA2A_E2E_OUTPUT:-target/e2e-evidence}
junit=target/nextest/ci/junit.xml
expected_tests=42
expected_records=9

case "$runs" in
  ''|*[!0-9]*|0) printf 'MA2A_E2E_RUNS must be a positive integer\n' >&2; exit 2 ;;
esac
command -v jq >/dev/null 2>&1 || { printf 'jq is required\n' >&2; exit 2; }
rm -rf "$output"
mkdir -p "$output"
commit=$(GIT_MASTER=1 git rev-parse HEAD)
rustc_version=$(rustc --version)
nextest_version=$(cargo nextest --version | sed -n '1p')
printf 'runs=%s expected_tests=%s expected_records=%s\n' \
  "$runs" "$expected_tests" "$expected_records" > "$output/summary.txt"
printf 'source_commit=%s\nrustc=%s\nnextest=%s\n' \
  "$commit" "$rustc_version" "$nextest_version" > "$output/provenance.txt"

run=1
while [ "$run" -le "$runs" ]; do
  log="$output/run-$run.log"
  evidence="$output/run-$run.jsonl"
  report="$output/run-$run.junit.xml"
  rm -f "$junit"
  printf 'run=%s command=cargo nextest run -p ma2a-app --test e2e --profile ci --success-output final --no-output-indent --color never\n' "$run" >> "$output/summary.txt"
  cargo nextest run -p ma2a-app --test e2e --profile ci --success-output final --no-output-indent --color never > "$log" 2>&1
  cp "$junit" "$report"
  grep -E '^\{' "$log" > "$evidence"
  root="$(sed -n '2p' "$report")"
  case "$root" in
    *"tests=\"$expected_tests\""*"skipped=\"0\""*"failures=\"0\""*"errors=\"0\""*) ;;
    *) printf 'invalid JUnit totals in %s\n' "$report" >&2; exit 1 ;;
  esac
  test "$(grep -c '<testcase ' "$report")" -eq "$expected_tests"
  test "$(wc -l < "$evidence")" -eq "$expected_records"
  jq -s -e '
    def exact($names): keys == ($names | sort);
    def ids: (.endpoint_ids | type == "object" and length > 0 and all(.[]; type == "string" and length > 0));
    def strings: type == "array" and all(.[]; type == "string" and length > 0);
    def numbers: type == "array" and length > 0 and all(.[]; type == "number");
    length == 9 and
    ([.[].scenario] | sort) == (["A","B","C","D","E","F","control-sync-active-dial","persistent-identity-restart","signer-claim-mismatch"] | sort) and
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
        (.record_sequences_after | numbers) and (.record_sequences_before | numbers) and
        .record_sequences == .record_sequences_after and .record_sequences_after == .durable_received_high_water and
        (.supplied_relay_candidates | strings)
      elif .scenario == "control-sync-active-dial" then
        exact(["address_high_water","endpoint_ids","manifest_high_water","private_space_leaked","relay_high_water","scenario"]) and
        .private_space_leaked == false and (.address_high_water | numbers) and
        (.manifest_high_water | numbers) and (.relay_high_water | numbers)
      elif .scenario == "persistent-identity-restart" then
        exact(["endpoint_ids","revision_after","revision_before","scenario"]) and
        .endpoint_ids.before == .endpoint_ids.after and .revision_after >= .revision_before
      elif .scenario == "signer-claim-mismatch" then
        exact(["accepted","address_high_water","endpoint_ids","relay_high_water","revision","scenario"]) and
        .accepted == false
      else false end)
  ' "$evidence" >/dev/null
  records=$(wc -l < "$evidence")
  sha256sum "$log" "$report" "$evidence" > "$output/run-$run.sha256"
  printf 'run=%s exit=0 tests=%s records=%s junit=%s evidence=%s\n' \
    "$run" "$expected_tests" "$records" "$report" "$evidence" >> "$output/summary.txt"
  run=$((run + 1))
done
