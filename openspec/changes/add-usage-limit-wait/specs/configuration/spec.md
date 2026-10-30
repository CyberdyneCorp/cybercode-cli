## ADDED Requirements

### Requirement: Usage limit settings
(P1) The schema SHALL accept the top-level key `usage_limit`, an object with:
- `mode`: `unattended` (default; wait only in Sessions with no interactive owner), `always` or `never`;
- `max_wait`: a duration `<n>s`, `<n>m`, `<n>h` or `<n>d` from 1 second to 7 days (default `6h`), the longest the system waits for one Turn's usage limit to reset.

Any other field, another `mode` value, or a `max_wait` that is not such a duration or is out of range SHALL be a validation error naming the field. `usage_limit` SHALL NOT be a security-sensitive definition, so project configuration may set it.

#### Scenario: Shorter maximum wait
- **WHEN** the project config sets `usage_limit: { "max_wait": "30m" }`
- **THEN** an unattended Turn whose usage limit resets in 40 minutes fails without waiting

#### Scenario: Invalid duration
- **WHEN** the global config sets `usage_limit.max_wait` to `"2 weeks"`
- **THEN** validation reports `usage_limit.max_wait: expected a duration from 1s to 7d, such as 30m or 6h`
