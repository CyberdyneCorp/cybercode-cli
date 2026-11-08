## ADDED Requirements

### Requirement: Image and PDF input
(P0) Adapters SHALL send user image and PDF parts in each protocol's native form:

- **`anthropic`:** `image` with a base64 source, and `document` with a base64 `application/pdf` source.
- **`openai_responses` and `chatgpt`:** `input_image` with a data URL, and `input_file` with `filename` and a data-URL `file_data`.
- **`openai_chat`:** `image_url` with a data URL, and `file` with `filename` and a data-URL `file_data`.
- **`copilot`:** its route's form, with `Copilot-Vision-Request: true` on requests that carry an image.

Image input SHALL be allowed only for models whose catalog entry has `capabilities.vision` (from models.dev input modalities, the Copilot model list, or `providers.<id>.models.<id>.capabilities`), and PDF input only with `capabilities.pdf`. A stored attachment reference that reaches an adapter unresolved SHALL fail the request with an invalid-request error instead of being sent or dropped.

#### Scenario: PDF to Anthropic
- **WHEN** a Turn on an Anthropic model with `capabilities.pdf` carries a PDF attachment
- **THEN** the request contains a `document` block with `media_type: "application/pdf"` and the base64 data

#### Scenario: Local model without declared vision
- **WHEN** a configured Ollama model has no `capabilities.vision` and the user attaches a screenshot
- **THEN** the prompt is refused with `UnsupportedModalityError`, whose message names the model
