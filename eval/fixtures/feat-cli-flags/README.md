# wordfreq

`wordfreq.py` prints how often each word occurs in a UTF-8 text file:

    python3 wordfreq.py notes.txt

A word is a maximal run of ASCII letters, digits and apostrophes (`[A-Za-z0-9']+`). Each output
line is `<count>\t<word>`, sorted by count (highest first), then by word in ascending
string order. Every distinct word is printed. An unreadable file prints
`wordfreq: cannot read <path>: <reason>` to stderr and exits 1.

Requested: options to limit the output to the most frequent words, to count words
case-insensitively, and to skip short words. Run the tests with `python3 -m unittest`.
