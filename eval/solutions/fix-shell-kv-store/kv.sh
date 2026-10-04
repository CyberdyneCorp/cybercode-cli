#!/usr/bin/env bash
# kv.sh - a tiny file-backed key-value store. See README.md for the contract.
set -u
export LC_ALL=C

PROG=kv.sh
TAB=$(printf '\t')
NL='
'
CR=$(printf '\r')
MAX_INT=999999999999999999
LOCK=

# ---------------------------------------------------------------- errors

die() { # MESSAGE [STATUS]
  printf '%s: %s\n' "$PROG" "$1" >&2
  exit "${2:-1}"
}

usage() { # SYNOPSIS
  die "usage: $PROG [-d DIR] $1" 2
}

check_key() {
  case $1 in
    '' | *"$NL"*) die "invalid key" 2 ;;
  esac
}

# ---------------------------------------------------------------- encoding

# Unsigned value of the single byte $1, in REPLY.
byte_value() {
  printf -v REPLY '%d' "'$1"
  REPLY=$((REPLY & 255))
}

# File name of key $1, in REPLY.
encode_key() {
  local key=$1 out= c i
  for ((i = 0; i < ${#key}; i++)); do
    c=${key:i:1}
    case $c in
      [a-z0-9_-]) out+=$c ;;
      .) if ((i > 0)); then out+=.; else out+=%2E; fi ;;
      *)
        byte_value "$c"
        printf -v c '%%%02X' "$REPLY"
        out+=$c
        ;;
    esac
  done
  REPLY=$out
}

# Key stored in file name $1, in REPLY. Names only contain [a-z0-9_.-] and %XX, so turning
# every %XX into an octal escape gives a safe printf format.
decode_name() {
  local name=$1 format= c i
  for ((i = 0; i < ${#name}; i++)); do
    c=${name:i:1}
    if [ "$c" = % ]; then
      printf -v c '\\%03o' "$((16#${name:i+1:2}))"
      i=$((i + 2))
    fi
    format+=$c
  done
  printf -v REPLY -- "$format"
}

# Escape $1 for the export line format, in REPLY.
escape_text() {
  local s=$1 out= c i
  for ((i = 0; i < ${#s}; i++)); do
    c=${s:i:1}
    case $c in
      '\') out+='\\' ;;
      "$TAB") out+='\t' ;;
      "$NL") out+='\n' ;;
      *) out+=$c ;;
    esac
  done
  REPLY=$out
}

# Undo escape_text on $1, in REPLY; returns 1 on a bad escape.
unescape_text() {
  local s=$1 out= c i
  for ((i = 0; i < ${#s}; i++)); do
    c=${s:i:1}
    if [ "$c" = '\' ]; then
      i=$((i + 1))
      case ${s:i:1} in
        '\') c='\' ;;
        t) c=$TAB ;;
        n) c=$NL ;;
        *) return 1 ;;
      esac
    fi
    out+=$c
  done
  REPLY=$out
}

# JSON string body for $1, in REPLY.
escape_json() {
  local s=$1 out= c i
  for ((i = 0; i < ${#s}; i++)); do
    c=${s:i:1}
    case $c in
      '"') out+='\"' ;;
      '\') out+='\\' ;;
      "$NL") out+='\n' ;;
      "$TAB") out+='\t' ;;
      "$CR") out+='\r' ;;
      [[:cntrl:]])
        byte_value "$c"
        printf -v c '\\u%04x' "$REPLY"
        out+=$c
        ;;
      *) out+=$c ;;
    esac
  done
  REPLY=$out
}

# Canonical decimal of integer text $1 (see is_integer), in REPLY.
to_decimal() {
  local sign= digits=$1
  case $digits in -*) sign=-; digits=${digits#-} ;; esac
  REPLY=$((${sign}10#$digits))
}

# Optional '-' then 1 to 18 ASCII digits.
is_integer() {
  local digits=${1#-}
  case $digits in '' | *[!0-9]*) return 1 ;; esac
  [ ${#digits} -le 18 ]
}

# ---------------------------------------------------------------- store

key_path() {
  encode_key "$1"
  REPLY=$DIR/$REPLY
}

# Exact contents of file $1 (trailing newlines included), in REPLY.
read_file() {
  REPLY=$(cat < "$1" && printf x) || die "cannot read $1"
  REPLY=${REPLY%x}
}

# Atomically replace file $1 with the bytes $2.
write_file() {
  local tmp="$DIR/.tmp.$$"
  printf '%s' "$2" > "$tmp" && mv -f "$tmp" "$1" || {
    rm -f "$tmp"
    die "cannot write $1"
  }
}

release_lock() {
  [ -z "$LOCK" ] || rm -rf "$LOCK"
}

lock_timeout() {
  local timeout=${KV_LOCK_TIMEOUT:-}
  case $timeout in '' | *[!0-9]*) timeout=10 ;; esac
  [ "$((10#$timeout))" -gt 0 ] || timeout=10
  REPLY=$((10#$timeout))
}

# Remove lock $1 if its holder, process $2, is dead. Breakers serialize on a second lock and
# re-read the pid under it, so a lock just taken by a live process is never removed.
# Returns 0 only when the stale lock was removed.
break_stale_lock() {
  local lock=$1 pid=$2 status=1
  case $pid in '' | *[!0-9]*) return 1 ;; esac
  ! ps -p "$pid" > /dev/null 2>&1 || return 1
  mkdir "$lock.break" 2> /dev/null || return 1
  if [ "$(cat "$lock/pid" 2> /dev/null)" = "$pid" ]; then
    rm -rf "$lock" && status=0
  fi
  rmdir "$lock.break"
  return $status
}

acquire_lock() {
  local lock="$DIR/.lock" pid tries=0 limit
  mkdir -p "$DIR" 2> /dev/null || die "cannot create store $DIR"
  lock_timeout
  limit=$((REPLY * 10))
  until mkdir "$lock" 2> /dev/null; do
    pid=$(cat "$lock/pid" 2> /dev/null) || pid=
    break_stale_lock "$lock" "$pid" && continue
    [ "$tries" -lt "$limit" ] || die "store is locked" 3
    tries=$((tries + 1))
    sleep 0.1
  done
  LOCK=$lock
  trap release_lock EXIT
  trap 'exit 130' INT TERM
  printf '%s\n' "$$" > "$lock/pid"
}

# Decoded keys starting with prefix $1, sorted, one per line.
sorted_keys() {
  local prefix=$1 path
  [ -d "$DIR" ] || return 0
  for path in "$DIR"/*; do
    [ -f "$path" ] || continue
    decode_name "${path##*/}"
    case $REPLY in
      "$prefix"*) printf '%s\n' "$REPLY" ;;
    esac
  done | sort
}

# ---------------------------------------------------------------- commands

cmd_set() {
  [ $# -eq 2 ] || usage "set KEY VALUE"
  check_key "$1"
  acquire_lock
  key_path "$1"
  write_file "$REPLY" "$2"
}

cmd_get() {
  [ $# -eq 1 ] || usage "get KEY"
  check_key "$1"
  key_path "$1"
  [ -f "$REPLY" ] || die "no such key: $1"
  cat < "$REPLY"
}

cmd_del() {
  [ $# -eq 1 ] || usage "del KEY"
  check_key "$1"
  acquire_lock
  key_path "$1"
  [ -f "$REPLY" ] || die "no such key: $1"
  rm -f "$REPLY"
}

cmd_list() {
  [ $# -le 1 ] || usage "list [PREFIX]"
  sorted_keys "${1:-}"
}

cmd_export() {
  local json=false key value sep=
  case $#:${1:-} in
    0:) ;;
    1:--json) json=true ;;
    *) usage "export [--json]" ;;
  esac
  $json && printf '{'
  while IFS= read -r key; do
    key_path "$key"
    read_file "$REPLY"
    value=$REPLY
    if $json; then
      escape_json "$key"
      printf '%s"%s":' "$sep" "$REPLY"
      escape_json "$value"
      printf '"%s"' "$REPLY"
      sep=,
    else
      escape_text "$key"
      printf '%s\t' "$REPLY"
      escape_text "$value"
      printf '%s\n' "$REPLY"
    fi
  done < <(sorted_keys "")
  $json && printf '}\n'
  return 0
}

cmd_import() {
  [ $# -eq 1 ] || usage "import FILE"
  local file=$1 line lineno=0 key keys=() values=() i
  if [ "$file" != - ]; then
    [ -f "$file" ] && [ -r "$file" ] || die "cannot read $file"
    exec < "$file"
  fi
  while IFS= read -r line || [ -n "$line" ]; do
    lineno=$((lineno + 1))
    [ -n "$line" ] || continue
    case $line in
      *"$TAB"*"$TAB"*) die "import: line $lineno: expected one tab" ;;
      *"$TAB"*) ;;
      *) die "import: line $lineno: expected one tab" ;;
    esac
    unescape_text "${line%%"$TAB"*}" || die "import: line $lineno: bad escape"
    key=$REPLY
    unescape_text "${line#*"$TAB"}" || die "import: line $lineno: bad escape"
    case $key in '' | *"$NL"*) die "import: line $lineno: invalid key" ;; esac
    keys+=("$key")
    values+=("$REPLY")
  done
  [ ${#keys[@]} -gt 0 ] || return 0
  acquire_lock
  for ((i = 0; i < ${#keys[@]}; i++)); do
    key_path "${keys[i]}"
    write_file "$REPLY" "${values[i]}"
  done
}

cmd_incr() {
  [ $# -eq 1 ] || [ $# -eq 2 ] || usage "incr KEY [N]"
  check_key "$1"
  local key=$1 step=${2-1} current=0 result
  is_integer "$step" || die "invalid increment: $step" 2
  acquire_lock
  key_path "$key"
  local path=$REPLY
  if [ -f "$path" ]; then
    read_file "$path"
    is_integer "$REPLY" || die "not an integer: $key"
    to_decimal "$REPLY"
    current=$REPLY
  fi
  to_decimal "$step"
  result=$((current + REPLY))
  if [ "$result" -gt "$MAX_INT" ] || [ "$result" -lt "-$MAX_INT" ]; then
    die "integer overflow: $key"
  fi
  write_file "$path" "$result"
  printf '%s\n' "$result"
}

# ---------------------------------------------------------------- main

main() {
  local dir=${KV_DIR:-.kv} command
  while [ $# -gt 0 ]; do
    case $1 in
      -d)
        [ $# -ge 2 ] && [ -n "$2" ] || die "missing value for -d" 2
        dir=$2
        shift 2
        ;;
      -*) die "unknown option: $1" 2 ;;
      *) break ;;
    esac
  done
  [ $# -gt 0 ] || die "missing command" 2
  # A leading ./ keeps a DIR starting with '-' from being read as an option by any tool.
  case $dir in -*) dir=./$dir ;; esac
  DIR=$dir
  command=$1
  shift
  case $command in
    set | get | del | list | export | import | incr) "cmd_$command" "$@" ;;
    *) die "unknown command: $command" 2 ;;
  esac
}

main "$@"
