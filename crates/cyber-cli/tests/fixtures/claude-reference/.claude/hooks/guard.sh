#!/bin/sh
# Blocks force pushes; the tool call arrives as JSON on stdin.
if grep -q -- '--force' ; then
  echo "force pushes are blocked" >&2
  exit 2
fi
