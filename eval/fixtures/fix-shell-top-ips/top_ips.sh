#!/usr/bin/env bash
# Print the busiest client IPs in an access log. See README.md for the contract.

LOG=$1
N=${2:-5}

if [ ! -f $LOG ]; then
  echo "no such file: $LOG"
  exit 1
fi

awk '{print $1}' $LOG | sort | uniq -c | sort -rn | head -n $N | awk '{print $1, $2}'
