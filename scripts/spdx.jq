# SPDX 2.3 license expressions (Annex D), checked for syntax: license identifiers joined by AND
# and OR, an exception after WITH, and parentheses. Tokens are separated by spaces; any other
# whitespace or control character refuses the expression. An identifier starts with a letter or
# digit, and only a license identifier, not an exception, may end with "+". The identifiers are
# not looked up in the SPDX list. A LicenseRef is refused, in any case, since the SBOM defines
# none, and so are the words npm and the license report use for no license or an unknown one. An
# operator with "+" or in another case, such as "AND+" or "or", is no identifier either.
def spdx_expression:
  [scan("[()]|[^ ()]+")] as $tokens
  | ($tokens | length > 0)
    and (test("[[:cntrl:]]") | not)
    and (reduce $tokens[] as $token ({ok: true, expect: "term", with: false, depth: 0};
      if .ok | not then .
      elif $token == "(" then
        if .expect == "term" then .depth += 1 else .ok = false end
      elif $token == ")" then
        if .expect == "operator" and .depth > 0
        then .depth -= 1 | .with = false else .ok = false end
      elif $token == "AND" or $token == "OR" then
        if .expect == "operator" then .expect = "term" | .with = false else .ok = false end
      elif $token == "WITH" then
        if .with then .expect = "exception" | .with = false else .ok = false end
      elif .expect == "operator"
        or (.expect as $expect | $token | test(if $expect == "term" then "^[A-Za-z0-9][A-Za-z0-9.-]*\\+?$"
          else "^[A-Za-z0-9][A-Za-z0-9.-]*$" end) | not)
        or ($token | test("^LicenseRef-"; "i"))
        or ($token | ascii_upcase | rtrimstr("+")
          | IN("UNKNOWN", "UNLICENSED", "NOASSERTION", "NONE", "AND", "OR", "WITH")) then
        .ok = false
      else .with = (.expect == "term") | .expect = "operator" end)
    | .ok and .expect == "operator" and .depth == 0);

# A declared license as SPDX states it: "MIT / Apache-2.0" as "MIT OR Apache-2.0", and anything
# that is not an expression, such as npm's "SEE LICENSE IN <file>", as NOASSERTION.
def spdx_declared:
  [split("/")[] | sub("^\\s+"; "") | sub("\\s+$"; "") | select(. != "")]
  | join(" OR ")
  | if spdx_expression then . else "NOASSERTION" end;
