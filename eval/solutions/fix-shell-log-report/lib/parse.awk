# Validate log lines, apply the --since/--status filters and normalize paths.
# Input: raw log lines. Output: one "endpoint<TAB>status<TAB>latency" line per kept entry.
# Variables: since (timestamp or empty), status (class digit or empty), malformed_file (path
# that receives the number of malformed lines).

function repeat(s, n,    out) {
  out = ""
  while (n-- > 0) out = out s
  return out
}

function normalize(path,    cut, n, i, segs, out) {
  cut = match(path, /[?#]/)
  if (cut) path = substr(path, 1, cut - 1)
  gsub(/\/+/, "/", path)
  if (length(path) > 1 && substr(path, length(path)) == "/") path = substr(path, 1, length(path) - 1)
  n = split(path, segs, "/")
  out = segs[1]
  for (i = 2; i <= n; i++) {
    if (segs[i] ~ /^[0-9]+$/) segs[i] = ":id"
    else if (segs[i] ~ UUID) segs[i] = ":uuid"
    out = out "/" segs[i]
  }
  return out
}

function valid() {
  return NF == 5 && $1 ~ TIMESTAMP && $2 ~ /^[A-Z]+$/ && $3 ~ /^\// &&
    $4 ~ /^[1-5][0-9][0-9]$/ && $5 ~ /^[0-9]+$/ && length($5) <= 9
}

BEGIN {
  FS = "[ \t]+"
  d = "[0-9]"; h = "[0-9a-fA-F]"
  TIMESTAMP = "^" repeat(d, 4) "-" d d "-" d d "T" d d ":" d d ":" d d "Z$"
  UUID = "^" repeat(h, 8) "-" repeat(h, 4) "-" repeat(h, 4) "-" repeat(h, 4) "-" repeat(h, 12) "$"
  malformed = 0
}

{
  # Trim leading/trailing blanks so FS splitting yields exactly the fields.
  sub(/^[ \t]+/, ""); sub(/[ \t]+$/, "")
  if ($0 == "") next
  if (!valid()) { malformed++; next }
  if (since != "" && $1 < since) next
  if (status != "" && substr($4, 1, 1) != status) next
  printf "%s %s\t%s\t%d\n", $2, normalize($3), $4, $5 + 0
}

END {
  print malformed > malformed_file
}
