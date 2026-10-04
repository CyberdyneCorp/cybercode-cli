#!/usr/bin/env bash
# Print the busiest client IPs in an access log. See README.md for the contract.
set -eu
export LC_ALL=C

fail() {
  echo "top_ips.sh: $*" >&2
  exit 2
}

[ $# -ge 1 ] && [ $# -le 2 ] || fail "usage: top_ips.sh LOGFILE [N]"
log=$1
n=${2:-10}

[ -f "$log" ] && [ -r "$log" ] || fail "cannot read log file: $log"
case $n in
  '' | *[!0-9]*) fail "N must be a positive integer, got: $n" ;;
esac
[ "$n" -ge 1 ] || fail "N must be a positive integer, got: $n"

# Count lines per IP, sort by count desc then IP asc, keep the first N
# (awk instead of head so an early close never trips SIGPIPE).
awk 'NF { count[$1]++ } END { for (ip in count) print count[ip], ip }' "$log" |
  sort -k1,1nr -k2,2 |
  awk -v n="$n" 'NR <= n + 0'
