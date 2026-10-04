# validator

`validator.py` checks decoded JSON (dicts, lists, str, int, float, bool, None) against a small
JSON-Schema-like dialect. `validate(schema, data)` returns a list of error strings, empty when
the data is valid. Each error is `<path>: <message>`, where the path starts at `$`, object
properties append `.<name>` and array elements append `[<index>]` (e.g. `$.items[2].name`).

Supported rules today:

| Rule | Message |
|---|---|
| `type` (`object`, `array`, `string`, `number`, `integer`, `boolean`, `null`) | `expected <type>, got <actual type>`; when the type is wrong no other rule is checked for that value |
| `minimum` / `maximum` (numbers) | `must be >= <minimum>` / `must be <= <maximum>` |
| `required` (objects) | `missing required property '<name>'` |
| `properties` (objects) | each present property is validated against its subschema |

Order schemas need two more rules, `enum` and `items` (see the task description).
Run the tests with `python3 -m unittest`.
