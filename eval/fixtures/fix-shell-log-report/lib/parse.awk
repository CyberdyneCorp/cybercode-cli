# Validate log lines, apply the --since/--status filters and normalize paths.
# Input: raw log lines. Output: one "endpoint<TAB>status<TAB>latency" line per kept entry.

function normalize(path,    n, i, segs, out) {
  sub(/\?.*/, "", path)
  gsub(/\/+/, "/", path)
  sub(/\/$/, "", path)
  n = split(path, segs, "/")
  out = segs[1]
  for (i = 2; i <= n; i++) {
    if (segs[i] ~ /^[0-9]+$/) segs[i] = ":id"
    else if (segs[i] ~ UUID) segs[i] = ":uuid"
    out = out "/" segs[i]
  }
  return out
}

BEGIN {
  h = "[0-9a-f]"
  UUID = "^" h h h h h h h h "-" h h h h "-" h h h h "-" h h h h "-" h h h h h h h h h h h h "$"
  bad = 0
}

{
  if (NF < 5 || $1 !~ /^[0-9-]+T[0-9:]+Z$/ || $3 !~ /^\// || $4 !~ /^[0-9]+$/ || $5 !~ /^[0-9]+$/) {
    bad++
    next
  }
  if (since != "" && $1 <= since) next
  if (status != "" && substr($4, 1, 1) != substr(status, 1, 1)) next
  print $2 " " normalize($3) "\t" $4 "\t" $5
}

END {
  if (bad > 0) print "skipped " bad " malformed lines" > "/dev/stderr"
}
