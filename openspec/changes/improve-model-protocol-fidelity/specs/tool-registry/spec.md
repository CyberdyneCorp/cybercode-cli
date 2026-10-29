## ADDED Requirements

### Requirement: Profile output budgets
(P1) When the model profile sets `tool_output.tokens`, the model-facing size limit for a result SHALL be the smaller of `tool_output.max_bytes` and `tokens × 4` bytes. When the profile sets `truncation: middle`, a truncated result SHALL keep its first and last lines, split evenly by bytes on line boundaries, around one marker line `…[<n> lines / <m> bytes omitted; full output at <path>]…`. `default` SHALL keep the end Output size budget describes for each tool; `head` and `tail` SHALL keep that end for every tool. These budgets SHALL apply to built-in, registered and MCP tool results alike; an MCP server's `output_token_limit` remains a further cap. The overflow file and `output_paths` SHALL be unchanged.

#### Scenario: Long build log for an OpenAI model
- **WHEN** a `bash` call on a model with `tool_output = { tokens: 10000, truncation: middle }` prints 200 KB
- **THEN** the model sees about 20 KB of the start and 20 KB of the end around one omission marker, and the full log is in the overflow file
