## ADDED Requirements

### Requirement: No credentials across redirects
(P0) Provider HTTP clients SHALL NOT follow redirects: a 3xx answer SHALL fail the request, so a key in a header such as `x-api-key` or `api-key`, and the prompt, never reach the redirect's host.

#### Scenario: Redirecting endpoint
- **WHEN** a provider endpoint answers `307` with a `Location` on another host
- **THEN** the request fails and nothing is sent to that host
