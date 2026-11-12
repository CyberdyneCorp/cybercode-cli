## ADDED Requirements

### Requirement: Stopped answer notice
(P1) Under an assistant message without an error whose `finish` is `content_filter`, the transcript SHALL show a warning that the provider's safety filter stopped the answer, that only this request is affected, and that rephrasing or narrowing it may help. For `length` it SHALL show that the answer was cut off at the model's output limit and can be continued. Both texts SHALL be translated.

#### Scenario: Refused answer
- **WHEN** the model refuses a prompt and the provider ends the answer with `content_filter`
- **THEN** the transcript shows the refusal text followed by `⚠ Stopped by the provider's safety filter. Only this request is affected: rephrase or narrow it and send again.`
