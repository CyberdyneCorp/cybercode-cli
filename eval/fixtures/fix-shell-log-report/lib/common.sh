# Shared helpers for latency_report.sh (sourced, not executed).

USAGE="usage: latency_report.sh [-n TOP] [--since TS] [--status CLASS] [--] LOG..."

die() {
  echo "latency_report.sh: $1"
  echo "$USAGE"
  exit 1
}

check_number() {
  [[ $1 =~ ^[0-9]+$ ]]
}

check_timestamp() {
  [[ $1 =~ ^[0-9-]+T[0-9:]+Z$ ]]
}

# Print the contents of every log; archives are decompressed.
read_logs() {
  case $1 in
    *.gz) gzip -dc $@ ;;
    *) cat $@ ;;
  esac
}
