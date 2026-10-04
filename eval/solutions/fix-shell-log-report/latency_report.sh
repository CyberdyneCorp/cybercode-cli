#!/usr/bin/env bash
# Summarize API access logs per normalized endpoint. See README.md for the contract.
set -u
export LC_ALL=C

LIB_DIR=$(cd -- "$(dirname -- "$0")/lib" && pwd) || exit 1
. "$LIB_DIR/common.sh"

top=10
since=
status=

# Options are only recognized before the first LOG argument.
while [ $# -gt 0 ]; do
  case $1 in
    --) shift; break ;;
    -n | --since | --status)
      [ $# -ge 2 ] || usage_error "missing value for $1"
      case $1 in
        -n) is_positive_int "$2" || usage_error "invalid -n value: $2"; top=$2 ;;
        --since) is_timestamp "$2" || usage_error "invalid --since value: $2"; since=$2 ;;
        --status) is_status_class "$2" || usage_error "invalid --status value: $2"; status=$2 ;;
      esac
      shift 2
      ;;
    -*) usage_error "unknown option: $1" ;;
    *) break ;;
  esac
done
[ $# -gt 0 ] || usage_error "no log files given"

WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/latency_report.XXXXXX") || die "cannot create a temporary directory"
trap 'rm -rf -- "$WORK_DIR"' EXIT

# Check every file before printing anything; .gz files are decompressed here once.
inputs=()
i=0
for log in "$@"; do
  [ -f "$log" ] && [ -r "$log" ] || die "cannot read $log"
  case $log in
    *.gz)
      i=$((i + 1))
      gzip -dc < "$log" > "$WORK_DIR/$i.log" 2> /dev/null || die "cannot decompress $log"
      inputs+=("$WORK_DIR/$i.log")
      ;;
    *) inputs+=("$log") ;;
  esac
done

# Each input is passed as an awk operand of the form ./path or /path so that awk never mistakes
# it for an option or a var=value assignment.
operands=()
for input in "${inputs[@]}"; do
  case $input in
    /*) operands+=("$input") ;;
    *) operands+=("./$input") ;;
  esac
done

TAB=$(printf '\t')
awk -v since="$since" -v status="${status%xx}" -v malformed_file="$WORK_DIR/malformed" \
    -f "$LIB_DIR/parse.awk" "${operands[@]}" > "$WORK_DIR/entries" || die "cannot parse logs"

{
  printf 'endpoint\tcount\terror%%\tp50\tp95\tp99\tmax\n'
  sort -t "$TAB" -k1,1 -k3,3n "$WORK_DIR/entries" |
    awk -f "$LIB_DIR/summarize.awk" |
    sort -t "$TAB" -k2,2nr -k1,1 |
    awk -v top="$top" 'NR <= top + 0'
} > "$WORK_DIR/report"

cat "$WORK_DIR/report"
malformed=$(cat "$WORK_DIR/malformed")
[ "$malformed" -eq 0 ] || warn "skipped $malformed malformed lines"
exit 0
