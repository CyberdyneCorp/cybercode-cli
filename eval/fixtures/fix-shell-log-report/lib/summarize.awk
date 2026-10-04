# Aggregate entries sorted by endpoint, then latency ascending.
# Input: "endpoint<TAB>status<TAB>latency". Output per endpoint:
# "endpoint<TAB>count<TAB>error%<TAB>p50<TAB>p95<TAB>p99<TAB>max".

function flush() {
  if (count == 0) return
  printf "%s\t%d\t%.1f%%\t%s\t%s\t%s\t%s\n", endpoint, count, errors * 100 / count,
    lat[int(50 * count / 100)], lat[int(95 * count / 100)], lat[int(99 * count / 100)], lat[count]
}

BEGIN { FS = "\t" }

$1 != endpoint {
  flush()
  endpoint = $1; count = 0; errors = 0
}

{
  lat[++count] = $3
  if ($2 >= 500) errors++
}

END { flush() }
