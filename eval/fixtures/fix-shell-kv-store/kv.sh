#!/usr/bin/env bash
# kv.sh - a tiny file-backed key-value store. See README.md for the contract.

PROG=kv.sh
DIR=${KV_DIR:-.kv}
LOCK_TIMEOUT=${KV_LOCK_TIMEOUT:-10}

# ---------------------------------------------------------------- errors

die() {
  echo "$PROG: $1"
  exit 1
}

usage() {
  die "usage: $PROG [-d DIR] $1"
}

check_key() {
  if [ -z "$1" ]; then
    die "invalid key"
  fi
}

# ---------------------------------------------------------------- encoding

# File name for a key: escape the characters that are not allowed in file names.
encode_key() {
  echo $1 | sed -e 's/%/%25/g' -e 's/\//%2F/g' -e 's/ /%20/g' -e 's/^\./%2E/'
}

# Key for a file name.
decode_name() {
  printf "$(echo $1 | sed 's/%\([0-9A-F][0-9A-F]\)/\\x\1/g')"
}

escape_text() {
  echo "$1" | sed -e 's/\\/\\\\/g' -e 's/\t/\\t/g'
}

unescape_text() {
  echo -e "$1"
}

escape_json() {
  echo "$1" | sed -e 's/"/\\"/g'
}

# ---------------------------------------------------------------- store

key_path() {
  echo $DIR/$(encode_key $1)
}

acquire_lock() {
  mkdir -p $DIR
  local waited=0
  while [ -d $DIR/.lock ]; do
    if [ $waited -ge $LOCK_TIMEOUT ]; then
      die "store is locked"
    fi
    sleep 1
    waited=$((waited + 1))
  done
  mkdir -p $DIR/.lock
}

release_lock() {
  rmdir $DIR/.lock
}

sorted_keys() {
  for name in $(ls $DIR 2> /dev/null); do
    decode_name $name
    echo
  done | grep "^$1" | sort
}

# ---------------------------------------------------------------- commands

cmd_set() {
  [ $# -eq 2 ] || usage "set KEY VALUE"
  check_key "$1"
  acquire_lock
  echo "$2" > $(key_path "$1")
  release_lock
}

cmd_get() {
  [ $# -eq 1 ] || usage "get KEY"
  check_key "$1"
  local path=$(key_path "$1")
  if [ ! -f $path ]; then
    die "no such key: $1"
  fi
  local value=$(cat $path)
  echo $value
}

cmd_del() {
  [ $# -eq 1 ] || usage "del KEY"
  check_key "$1"
  acquire_lock
  local path=$(key_path "$1")
  if [ ! -f $path ]; then
    die "no such key: $1"
  fi
  rm $path
  release_lock
}

cmd_list() {
  sorted_keys "$1"
}

cmd_export() {
  local json=false
  if [ "$1" = "--json" ]; then
    json=true
  fi
  local first=true
  $json && printf '{'
  for key in $(sorted_keys); do
    local value=$(cat $(key_path $key))
    if $json; then
      $first || printf ','
      printf '"%s":"%s"' "$(escape_json "$key")" "$(escape_json "$value")"
      first=false
    else
      echo "$(escape_text "$key")	$(escape_text "$value")"
    fi
  done
  $json && echo '}'
}

cmd_import() {
  [ $# -eq 1 ] || usage "import FILE"
  local file=$1
  if [ "$file" = "-" ]; then
    file=/dev/stdin
  elif [ ! -e $file ]; then
    die "cannot read $file"
  fi
  local lineno=0
  acquire_lock
  while read line; do
    lineno=$((lineno + 1))
    [ -z "$line" ] && continue
    local key=$(echo "$line" | cut -f1)
    local value=$(echo "$line" | cut -f2)
    if [ "$key" = "$line" ]; then
      die "import: line $lineno: expected one tab"
    fi
    echo -n "$(unescape_text "$value")" > $(key_path "$(unescape_text "$key")")
  done < $file
  release_lock
}

cmd_incr() {
  [ $# -ge 1 ] || usage "incr KEY [N]"
  check_key "$1"
  local step=${2:-1}
  acquire_lock
  local path=$(key_path "$1")
  local current=0
  if [ -f $path ]; then
    current=$(cat $path)
  fi
  if ! [[ $current =~ ^-?[0-9]+$ ]]; then
    die "not an integer: $1"
  fi
  local result=$((current + step))
  echo $result > $path
  release_lock
  echo $result
}

# ---------------------------------------------------------------- main

while [ $# -gt 0 ]; do
  case $1 in
    -d)
      DIR=$2
      shift 2
      ;;
    *)
      break
      ;;
  esac
done

command=$1
shift

case $command in
  set) cmd_set "$@" ;;
  get) cmd_get "$@" ;;
  del) cmd_del "$@" ;;
  list) cmd_list "$@" ;;
  export) cmd_export "$@" ;;
  import) cmd_import "$@" ;;
  incr) cmd_incr "$@" ;;
  "") die "missing command" ;;
  *) die "unknown command: $command" ;;
esac
