# SPDX 2.3 license expressions (Annex D): license identifiers joined by AND and OR, an exception
# after WITH, and parentheses. Tokens are separated by spaces; any other whitespace or control
# character refuses the expression. Each identifier must be on the SPDX License List that
# spdx-ids.jq holds, matched in any case as Annex D asks: a license identifier before WITH or an
# operator, which alone may end with "+", and an exception identifier after WITH. A LicenseRef is
# refused, since the SBOM defines none, and so are the words npm and the license report use for no
# license or an unknown one, and an operator with "+" or in another case, such as "AND+" or "or":
# none of them is on the list.
include "spdx-ids";

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
        or (.expect as $expect | $token | ascii_downcase
          | if $expect == "term" then spdx_license_ids[rtrimstr("+")] else spdx_exception_ids[.] end
          | not) then
        .ok = false
      else .with = (.expect == "term") | .expect = "operator" end)
    | .ok and .expect == "operator" and .depth == 0);

# A declared license as SPDX states it: "MIT / Apache-2.0" as "MIT OR Apache-2.0", and anything
# that is not an expression, such as npm's "SEE LICENSE IN <file>" or one naming an identifier
# that is not on the list, as NOASSERTION.
def spdx_declared:
  [split("/")[] | sub("^\\s+"; "") | sub("\\s+$"; "") | select(. != "")]
  | join(" OR ")
  | if spdx_expression then . else "NOASSERTION" end;
