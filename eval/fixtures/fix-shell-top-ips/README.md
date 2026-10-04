# top_ips.sh

Ops helper that reports the busiest clients in a web server access log (common log format: the
client IP is the first whitespace-separated field of each line).

Contract for `top_ips.sh LOGFILE [N]` (bash, standard POSIX tools only; must behave the same with
the BSD tools on macOS and the GNU tools on Linux):

- Prints at most `N` lines (default **10**) of the form `<count> <ip>`, single space, no leading
  spaces: the number of log lines per client IP. Blank lines are ignored.
- Sorted by count, highest first; equal counts are ordered by IP ascending as plain byte strings
  (the order of `LC_ALL=C sort`, so `10.0.0.2` comes before `9.9.9.9`).
- An empty log prints nothing and exits 0.
- Errors go to stderr with exit status 2: no LOGFILE argument, a LOGFILE that does not exist or
  cannot be read, or an `N` that is not a positive integer (digits only, at least 1).
- Paths containing spaces work.

The on-call team reports that it shows only five clients, puts ties in reverse order, breaks on
log paths with spaces and exits 1 with the error on stdout. Run the tests with
`python3 -m unittest`.
