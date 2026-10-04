#!/usr/bin/env bash
# Summarize API access logs per normalized endpoint. See README.md for the contract.

LIB_DIR=$(dirname $0)/lib
source $LIB_DIR/common.sh

TOP=10
SINCE=""
STATUS=""
FILES=""

while [ $# -gt 0 ]; do
  case "$1" in
    -n)
      check_number $2 || die "invalid -n value: $2"
      TOP=$2
      shift 2
      ;;
    --since)
      check_timestamp $2 || die "invalid --since value: $2"
      SINCE=$2
      shift 2
      ;;
    --status)
      STATUS=$2
      shift 2
      ;;
    --)
      shift
      ;;
    -*)
      die "unknown option: $1"
      ;;
    *)
      FILES="$FILES $1"
      shift
      ;;
  esac
done

if [ -z "$FILES" ]; then
  die "no log files given"
fi

for f in $FILES; do
  if [ ! -e $f ]; then
    die "cannot read $f"
  fi
done

TMP=$(mktemp -d)
trap "rm -rf $TMP" EXIT

read_logs $FILES |
  awk -v since="$SINCE" -v status="$STATUS" -f $LIB_DIR/parse.awk > $TMP/entries

echo -e "endpoint\tcount\terror%\tp50\tp95\tp99\tmax"
sort -t "	" -k1,1 -k3,3 $TMP/entries |
  awk -f $LIB_DIR/summarize.awk |
  sort -t "	" -k2,2nr |
  head -n $TOP
