# Shared helpers for latency_report.sh (sourced, not executed).

PROG=latency_report.sh
USAGE="usage: $PROG [-n TOP] [--since TS] [--status CLASS] [--] LOG..."

warn() {
  printf '%s: %s\n' "$PROG" "$*" >&2
}

# Runtime failure: unreadable input and the like.
die() {
  warn "$*"
  exit 1
}

usage_error() {
  warn "$*"
  printf '%s\n' "$USAGE" >&2
  exit 2
}

# Digits only and at least 1.
is_positive_int() {
  case $1 in
    '' | *[!0-9]*) return 1 ;;
  esac
  case $1 in
    *[1-9]*) return 0 ;;
  esac
  return 1
}

# YYYY-MM-DDTHH:MM:SSZ with ASCII digits.
is_timestamp() {
  case $1 in
    [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9]Z) return 0 ;;
  esac
  return 1
}

is_status_class() {
  case $1 in
    [1-5]xx) return 0 ;;
  esac
  return 1
}
