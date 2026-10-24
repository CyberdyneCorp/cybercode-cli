## ADDED Requirements

### Requirement: Output style routes
(P0) `POST /api/v1/sessions` SHALL accept an optional `output_style`. `POST /api/v1/sessions/:sessionID/output-style` SHALL accept `{ style: string | null }`, record the switch and respond with the Session. Both SHALL respond `400` `InvalidRequestError` with the reason for an invalid, unknown or held style, and for a Session with a parent. `GET /api/v1/config/output-styles` SHALL be Location-scoped and SHALL return:
- `styles`, each with `name`, `description`, `origin` (`builtin`, `user` or `project`), and `path` and `shadows` for files;
- `issues`, each with `path` and `message`;
- `configured`, the configured name and the layer that set it, or `null`;
- `effective`, the style a new primary Session there would use.

`PUT /api/v1/config/output-style` SHALL accept `{ style: string | null }` and set `output_style` in the user's global configuration. `null` SHALL set it to `default`. Only built-in and user styles SHALL be accepted, and every other byte of the document SHALL be kept. These operations SHALL be in the OpenAPI document.

#### Scenario: Listing reports a held project style
- **WHEN** a client calls `GET /api/v1/config/output-styles` for an untrusted checkout with `.cyber/output-styles/reviewer.md`
- **THEN** `styles` has the four built-ins and the user styles, and `issues` has the trust hold for the project directory

#### Scenario: Global default must not name a project style
- **WHEN** a client calls `PUT /api/v1/config/output-style` with `{ "style": "reviewer" }` and `reviewer` is defined only by a project
- **THEN** the server responds `400` and the global configuration is unchanged
