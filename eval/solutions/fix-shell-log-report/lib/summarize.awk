# Aggregate entries sorted by endpoint, then latency ascending (numeric).
# Input: "endpoint<TAB>status<TAB>latency". Output per endpoint:
# "endpoint<TAB>count<TAB>error%<TAB>p50<TAB>p95<TAB>p99<TAB>max".

function rank(p, n) {
  # Nearest rank: ceil(p * n / 100), computed in integers.
  return int((p * n + 99) / 100)
}

function error_rate(errors, count,    tenths) {
  tenths = int((2000 * errors + count) / (2 * count))
  return int(tenths / 10) "." (tenths % 10) "%"
}

function flush() {
  if (count == 0) return
  printf "%s\t%d\t%s\t%d\t%d\t%d\t%d\n", endpoint, count, error_rate(errors, count),
    lat[rank(50, count)], lat[rank(95, count)], lat[rank(99, count)], lat[count]
}

BEGIN { FS = "\t"; count = 0 }

$1 != endpoint {
  flush()
  endpoint = $1; count = 0; errors = 0
}

{
  lat[++count] = $3 + 0
  if (substr($2, 1, 1) == "5") errors++
}

END { flush() }
