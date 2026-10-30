## ADDED Requirements

### Requirement: Restricted plugin process capabilities
(P1) Plugin bootstrap and descendants SHALL run under a native capability intersection that separately enforces reviewed runtime/package reads, private scratch, declared Location reads, declared writes, subprocess execution, network and secrets. `fs.write` SHALL NOT imply broad project read access. Without `fs.read`, native runtime APIs SHALL NOT read Location or arbitrary home content; without `shell`, the host SHALL NOT launch another shell, native executable or interpreter copy after its reviewed bootstrap. Permission callbacks SHALL NOT widen manifest capabilities. Protected owner credentials and private artifacts SHALL remain hidden even with filesystem capabilities. A platform/runtime without proven enforcement SHALL refuse startup explicitly rather than run with broader authority.

#### Scenario: Runtime bypass is denied
- **WHEN** a plugin without `fs.read` or `shell` tries a native file read and direct subprocess creation rather than a plugin helper API
- **THEN** the native boundary refuses both operations while the supported host can still initialize and execute an otherwise permitted tool

#### Scenario: Declared subprocess inherits restrictions
- **WHEN** a plugin with `shell` launches a child without Location read or network capability
- **THEN** the child cannot gain those capabilities through the host's broader Session Mode, inherited descriptors or environment

#### Scenario: Tool environment cannot inject plugin bootstrap
- **WHEN** the owner environment or ordinary-tool environment policy contains runtime loader options, search paths, proxy overrides or provider credentials
- **THEN** the plugin host receives only its explicit bootstrap environment and owner-created proxy settings, with its home and cache paths inside private scratch
- **AND** plugin configuration and authorized secrets continue through the host protocol rather than implicit environment inheritance

#### Scenario: Package startup does not borrow workspace configuration
- **WHEN** a Bun or Deno host starts for a Location containing unrelated runtime configuration or dotenv files
- **THEN** startup uses its package working directory and explicit package configuration or reviewed empty configuration
- **AND** permitted bundled preloads, imports and frozen package locks remain usable without rewriting the approved package
- **AND** prepared Deno packages retain an owner-created boundary against ancestor workspace discovery while the workspace Location remains in the initialize envelope

#### Scenario: Project PATH entries cannot choose the initial runtime
- **WHEN** PATH includes a Location/package directory, a relative entry or a shim resolving into either excluded tree
- **THEN** host runtime lookup ignores that entry and selects a suitable outside executable or refuses with a plugin-named diagnostic
- **AND** this search guard does not substitute for immutable reviewed runtime and dependency admission

#### Scenario: Initialization cleanup retains unconfirmed resources
- **WHEN** initialization returns early or its future is cancelled after host process creation
- **THEN** a separate owner continues process cleanup and proxy revocation and retains scratch until backend wait and proxy disposal are acknowledged
- **AND** runtime loss or an unconfirmed wait retains resources and reports uncertainty rather than treating a termination request as proof of disposal

### Requirement: Runtime contribution generation ownership
(P1) Runtime hooks, providers and catalog transforms SHALL be validated and owned by plugin identity, digest, Location and host generation. Revocation SHALL remove only that generation before later dispatch, and stale results SHALL NOT restore it. Runtime hooks SHALL follow ordinary plugin hook scope and managed-only policy. Provider streams SHALL use normal usage/cancellation paths and SHALL NOT supply trusted reserved bounds by declaration alone. Catalog transforms SHALL be bounded and ordered, preserve policy/configuration ceilings and leave the previous valid view intact on failure.

#### Scenario: Disabled host completes late
- **WHEN** a disabled plugin returns an old provider result or catalog transform
- **THEN** no contribution is republished and its invocation retains its actual settlement or unknown status

### Requirement: Scoped plugin prompt callbacks
(P1) A `session/prompt` callback SHALL require its capability and a live core-issued invocation context bound to plugin generation, Session and Location. It SHALL use durable ordinary inbox admission with a retained operation identity, Mode/trust/permission/budget/recursion ceilings and truthful non-interactive behavior. A callback SHALL NOT create human presence or grant authority over unrelated Sessions. Nested callback admission SHALL NOT wait under a lock required by the receiving actor.

#### Scenario: Lost callback acknowledgement
- **WHEN** the host retries an admitted prompt with the same authorized operation identity
- **THEN** it receives the existing admission rather than creating another prompt or model run

#### Scenario: Background host selects another Session
- **WHEN** a plugin sends a callback without a live invocation context or targets an unrelated Session
- **THEN** the core refuses it without admission, inference or an approval bypass
