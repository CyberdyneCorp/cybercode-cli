## ADDED Requirements

### Requirement: Reasoning items
(P1) The system SHALL keep the reasoning of an assistant message as an ordered list of reasoning items in stream order, each placed among the message's tool calls as the provider produced it. The message's text SHALL be kept as one segment that replays after the reasoning produced before the first call and ahead of the calls. Each item SHALL hold its text and optional opaque provider state: `encrypted_reasoning`, `signature`, `redacted` or `details`, together with the producing provider and model and an optional provider item ID. A tool call SHALL be able to carry opaque state of its own, such as a Gemini thought signature.

Each reasoning block SHALL be persisted as one `session.reasoning.ended.1` event with optional `index` and `opaque` fields. Opaque state on a tool call SHALL be an optional `opaque` field of `session.tool.called.1`. An event without these fields SHALL read as one text item, whose `signature` (when present) is `signature` state of the message's provider and model.

Opaque data SHALL be stored exactly as received and never decoded. It SHALL be replaced by `[opaque reasoning]` in exports, shares, logs and telemetry. Its size SHALL count towards context estimates at 3 tokens per 4 bytes. A single opaque value larger than 1 MiB SHALL be dropped when it is received, so that item replays as text, and one warning SHALL be logged.

#### Scenario: Interleaved thinking keeps every signed block
- **WHEN** an Anthropic response streams `thinking(sig A) → tool_use → thinking(sig B) → tool_use`
- **THEN** the message stores two reasoning items with signatures A and B, in that order between the calls, and the next request in the tool loop sends both blocks unchanged in the same order

#### Scenario: Redacted thinking is preserved
- **WHEN** an Anthropic response contains a `redacted_thinking` block before a tool call
- **THEN** the next request in the tool loop sends the same `redacted_thinking` block, and the request is accepted

#### Scenario: Old events still replay
- **WHEN** a Session recorded before this change holds `session.reasoning.ended.1 { text, signature }` from `anthropic/claude-sonnet-5-5`
- **THEN** a Turn on the same model sends one signed `thinking` block, as before

#### Scenario: Opaque data never leaves through an export
- **WHEN** a Session with encrypted reasoning is exported as HTML or JSON
- **THEN** the export shows the reasoning summary text and `[opaque reasoning]` in place of the blob

### Requirement: Reasoning replay policy
(P1) The system SHALL send reasoning items back to the provider according to the model profile's `reasoning_replay`:
- `never`: lower item text to assistant text, as Reasoning lowering across models describes;
- `tool_loop`: send native items only for assistant messages after the latest user message, and drop older reasoning;
- `always`: send native items for the whole history.

An item SHALL be sent natively only when its recorded provider and model equal the Turn's provider and model. Otherwise its text SHALL be lowered to assistant text, and an item without text SHALL be dropped.

When an Anthropic request with thinking enabled would continue a tool loop whose latest assistant message has no native reasoning item, the system SHALL send that one request with thinking off and log one `info` line naming the reason.

A compaction summary SHALL carry no opaque state. Items in the kept tail SHALL replay under the same policy.

#### Scenario: OpenAI keeps its reasoning across tool calls
- **WHEN** `openai/gpt-6-sol` returns a reasoning item with `encrypted_content` and a function call, and the tool result is sent back
- **THEN** the next request's `input` holds that reasoning item, with its `encrypted_content`, before the `function_call` and `function_call_output` items

#### Scenario: DeepSeek tool loop
- **WHEN** `deepseek/deepseek-v4` (profile carrier `reasoning_content`, replay `tool_loop`) calls a tool in thinking mode
- **THEN** the follow-up request's assistant message carries `reasoning_content` with the streamed reasoning, and after the next user message earlier `reasoning_content` is no longer sent

#### Scenario: Model switch inside a tool loop
- **WHEN** a tool loop started on `openai/gpt-6-sol` continues on `anthropic/claude-opus-5-5` with thinking enabled
- **THEN** the GPT reasoning is sent as assistant text, that one request is sent with thinking off, and later requests have thinking on again
