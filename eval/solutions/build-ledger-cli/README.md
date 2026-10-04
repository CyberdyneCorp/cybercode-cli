# ledger

A plain-text double-entry accounting CLI. Python 3.10+, standard library only. The full contract
(journal format, reports, error messages) is in `SPEC.md`.

## Usage

```sh
python3 -m ledger -f books.journal balance
python3 -m ledger -f books.journal balance assets expenses --depth 2 --end 2024-07-01
python3 -m ledger -f books.journal balance --flat --empty --value USD
python3 -m ledger -f books.journal register bank --begin 2024-01-01 --running
python3 -m ledger -f books.journal print food
python3 -m ledger -f books.journal accounts
python3 -m ledger -f books.journal payees
python3 -m ledger -f books.journal prices
python3 -m ledger -f books.journal stats
python3 -m ledger -f books.journal --strict check
```

A journal holds dated transactions with indented postings, `include`, `account` and `P` (market
price) directives:

```
account Assets:Bank
P 2024-01-31 AAPL 190.00 USD

2024-01-01 * Opening balance
    Assets:Bank      2,000.00 USD
    Equity:Opening
```

Exit codes: 0 success, 1 unreadable or invalid journal (errors are reported as
`error: FILE:LINE: MESSAGE`), 2 usage error or invalid option value.

## Tests

```sh
python3 -m unittest discover -s tests
```
