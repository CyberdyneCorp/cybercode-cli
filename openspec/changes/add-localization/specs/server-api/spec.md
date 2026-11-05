## ADDED Requirements

### Requirement: Language routes
(P1) `GET /api/v1/config/language` SHALL be Location-scoped and SHALL return `language` (`auto` or the tag the Location's configuration sets), `source` (the layer that set it, absent when unset), `reply` (the reply language, absent for `auto`) and `supported` (`auto`, `en`, `pt-BR`, `es`, `fr`). `PUT /api/v1/config/language` SHALL accept `{ language: string }`, matched ignoring case, write the canonical value to `language` in the user's global configuration keeping every other byte of the document, and return the Location's setting after the edit. An unsupported value SHALL respond `400` `InvalidRequestError` and change nothing. Both operations SHALL be in the OpenAPI document.

#### Scenario: Set the global language
- **WHEN** a client calls `PUT /api/v1/config/language` with `{"language": "PT-br"}` for a Location without its own `language`
- **THEN** the global document gains `"language": "pt-BR"` with its comments kept, and the response has `language` and `reply` `pt-BR`

#### Scenario: Unsupported language
- **WHEN** a client calls `PUT /api/v1/config/language` with `{"language": "de"}`
- **THEN** the response is `400` `InvalidRequestError` and the global document is unchanged
