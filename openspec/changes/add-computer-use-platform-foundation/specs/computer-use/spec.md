## ADDED Requirements

### Requirement: Computer capability report
(P2) Computer health SHALL carry a `schema_version` and a platform-neutral capability report: a mode derived as `unsupported`, `capture_only` or `input_enabled`; separate capture and input availability, each with a list of reason codes; the input-assurance state; the authority kind (`os_permission`, `platform_picker` or `owner_review_only`) and whether owner setup can request anything; interactive-session facts that are null when unknown; OS family, version, build and architecture; the selectors the platform honors; and the evidence level of the running configuration. Admission SHALL require capture availability for every action and input availability for input actions. Existing health fields SHALL keep their macOS meaning, and on other platforms SHALL mirror the new availability facts without reporting an OS permission that does not exist. Owner review in Cyber SHALL be required whatever the authority kind. Health without an explicit owner setup request SHALL NOT capture pixels, send input, open a picker or OS prompt, enumerate window titles, or acquire or wait on the host input lease. Diagnostic values such as a desktop name or interface versions SHALL NOT enable a capability. A transient probe failure SHALL NOT by itself revoke a live grant.

#### Scenario: Capture-only host
- **WHEN** a host can capture the reviewed source but cannot prove target assurance for input
- **THEN** health reports mode `capture_only`, capture available with no reasons, and input unavailable with `target_assurance_unavailable`

#### Scenario: Platform without an OS permission
- **WHEN** health is read on a platform whose supported capture path needs no OS grant
- **THEN** it reports authority `owner_review_only`, does not tell the owner to enable an Accessibility or capture permission, and opens no OS UI

#### Scenario: Version 1 client reads version 2 health
- **WHEN** a client built for the macOS-only schema reads health from a server using this contract
- **THEN** every field it knows is present with its previous meaning and it ignores the added fields

### Requirement: Computer reason codes
(P2) Every computer refusal, unavailability, revocation and receipt reason SHALL use a stable snake_case code from one registry. A code SHALL have one meaning on every platform. Platform-only codes MAY be registered by a platform change within the registry's categories and SHALL NOT reuse a shared code with a different meaning. A response SHALL carry one primary code and MAY carry detail codes. Clients SHALL treat an unknown code as unavailable and show its text. The unsupported-platform error SHALL NOT name a single operating system and SHALL carry a code.

#### Scenario: Unsupported selector
- **WHEN** a request uses a selector kind the running platform does not honor
- **THEN** it is refused with `selector_unsupported_on_platform` before any native call

#### Scenario: New code reaches an older client
- **WHEN** a client receives a reason code it does not know
- **THEN** it reports the capability as unavailable with the server's text and does not treat it as available

### Requirement: Computer target selection and identity
(P2) A computer selector SHALL name exactly one of `app` (macOS bundle ID), `display` (macOS display index), `window` (a short-lived candidate token from an owner-only listing) or `pick` (`window` or `output`, chosen in the platform's own picker). Each platform SHALL advertise the selectors it honors and SHALL refuse the others, and SHALL NOT reinterpret a selector value as a different kind of identifier. Targets SHALL carry a server-issued opaque `target_id` and a typed source description. Raw OS window, display, stream or process handles SHALL NOT be accepted from callers or emitted for clients to interpret; legacy numeric macOS fields MAY remain as deprecated compatibility fields and SHALL be zero or null on other platforms. A caller-supplied handle, process ID, path or identity string SHALL NOT select or authorize a target. Candidate tokens SHALL expire 30 seconds after the listing that issued them.

#### Scenario: Caller supplies a numeric handle
- **WHEN** a request names a numeric window handle or process ID as its target on any platform
- **THEN** it is rejected without resolving, capturing or arming anything

#### Scenario: macOS bundle ID on another platform
- **WHEN** an `app` selector reaches a platform that does not honor it
- **THEN** it is refused with `selector_unsupported_on_platform` and is not treated as an executable path or application ID

### Requirement: Review-bound computer arming
(P2) Each selector kind SHALL be declared re-resolvable or not. For a selector that is not re-resolvable (an owner pick or a candidate token), a grant SHALL be armed only by confirming the identifier of a review whose target was shown to the owner, before that review's deadline; arming SHALL NOT start a review, open a picker or arm a different target, and SHALL re-check that the live target matches the reviewed one. A review not confirmed by its deadline, or superseded, SHALL have its platform scope torn down. Re-resolvable selectors SHALL keep their existing review-then-confirm behavior.

#### Scenario: Arm a picked source by review identifier
- **WHEN** the owner confirms the identifier of a pending pick review before its deadline
- **THEN** exactly the reviewed source is armed and no new picker opens

#### Scenario: Confirmation names an expired review
- **WHEN** an owner confirmation names an expired, superseded or other Session's review
- **THEN** nothing is armed and no picker opens

### Requirement: Computer grant lifecycle across platforms
(P2) Grants on every platform SHALL be armed only by an authenticated present owner for one Session and one reviewed target, SHALL keep the existing TTL, rate, action and frame limits, and SHALL NOT survive restart or pass to child, hub, peer or relay agents. Each platform SHALL classify grant-affecting events as binding loss or topology change. Binding loss (target destroyed or replaced, platform scope closed, lock, logoff, desktop or session switch, remote attachment, loss of a required session watch, OS permission revoked) SHALL revoke the grant through the controller fence before teardown, and unlock, reconnect or restart SHALL NOT restore it. A topology change that leaves the binding intact SHALL advance a topology epoch that makes every older frame stale without revoking the grant. An event a platform cannot classify SHALL be treated as binding loss. At most one native worker per OS user and interactive session SHALL hold input authority across all Cyber server homes. A server that detects a lease abandoned by a crashed holder SHALL warn the owner during review that input cleanup is uncertain and SHALL NOT read or settle another home's journal. Platforms added after macOS SHALL refuse remote-desktop or screen-sharing sessions with `remote_session` until their own change accepts them with evidence.

#### Scenario: Monitor added during a grant
- **WHEN** a display is added while the binding is unchanged and the model clicks with a frame captured before the change
- **THEN** the click is refused as `frame_stale` before any event, the grant stays armed, and a new screenshot allows further actions

#### Scenario: Lock then unlock
- **WHEN** a platform reports a session lock during a grant and the owner later unlocks
- **THEN** the grant was revoked at lock and control requires a new review

#### Scenario: Health while another home holds the lease
- **WHEN** health is read while another server home holds the host lease
- **THEN** health does not acquire or wait on the lease and does not cause a concurrent arming attempt to fail

### Requirement: Computer schema versioning
(P2) Health, previews, frames, receipts and tool status SHALL carry an integer `schema_version`. The macOS-only shape SHALL be version 1 and this contract version 2. Within a version, changes SHALL be additive only; a field SHALL be removed or change meaning only in a new version after at least one tagged release of deprecation. Fields defined by an earlier version SHALL remain present and correct in later output. The journal SHALL record each receipt's version and SHALL replay earlier rows without rewriting them. The model-facing computer tool SHALL offer the same action set on every platform; an action the platform cannot perform SHALL be refused with a reason code before admission and before any journal intent, with no native side effect.

#### Scenario: Input on a capture-only host
- **WHEN** a model calls a click on a host whose mode is `capture_only`
- **THEN** the call is refused with the input reason code, no journal intent is written and no native call is made

#### Scenario: Replay a version 1 receipt
- **WHEN** an operation settled before this contract is replayed after upgrade
- **THEN** the stored receipt is returned as written and marked replayed

### Requirement: Shared computer key chord policy
(P2) Key chords SHALL be checked by one platform-neutral policy before admission and before any journal intent, refusing with `key_chord_refused` and sending no event. The policy SHALL assign each platform a primary editing modifier and a system modifier. Under app or window scope it SHALL refuse the system modifier, SHALL allow a modified chord only when its key is in the shared edit-key list, and on platforms other than macOS SHALL refuse `alt`; clipboard chords SHALL therefore be refused under app or window scope on every platform. Platforms added after macOS SHALL also refuse, in every scope, secure-attention, session, task-switching, launcher and quit chords and any chord using the system modifier. Letters SHALL be resolved through the target's current keyboard layout and refused when a modifier the chord did not name is needed. On macOS the policy SHALL allow and refuse exactly the chords the native adapter allows and refuses today, and the native check SHALL remain as a second line.

#### Scenario: Paste under a window grant
- **WHEN** the model requests the primary modifier with `v` under an app or window grant on any platform
- **THEN** the chord is refused with `key_chord_refused`, no event is sent, no journal intent is written and the grant stays armed

#### Scenario: macOS undo under an app grant
- **WHEN** the model requests `command` with `z` under a macOS app grant
- **THEN** the chord is allowed, as it is today

### Requirement: Native computer worker boundary
(P2) The component that executes native capture and input SHALL own every native handle and descriptor, SHALL make each non-inheritable from receipt, and SHALL NOT spawn children. Stop SHALL fence the grant without waiting for the worker, and the host lease SHALL be released only after the worker has provably stopped dispatching. Native calls SHALL be bounded by the remaining action budget, late callbacks SHALL be dropped by generation, and pressed keys and buttons SHALL be released on cancel, timeout or failure with uncertainty recorded when release cannot be confirmed. If a platform uses a helper process, it SHALL be the same binary, run as the same user without added privilege, communicate only over a channel created by the server and passed at spawn, accept only typed size-bounded requests, never receive owner credentials, exit when the server exits and be terminable by Stop; a name, loopback address or same-user check SHALL NOT be treated as owner proof.

#### Scenario: Worker blocked during Stop
- **WHEN** Stop is issued while the native worker is blocked in a platform call
- **THEN** the grant is fenced at once and another grant can acquire the lease only after that worker has stopped dispatching

#### Scenario: Child process inherits handles
- **WHEN** any process is spawned while a grant is active
- **THEN** it inherits none of the native worker's capture or input handles

### Requirement: Computer action receipts
(P2) Receipts SHALL keep their existing fields and outcome values and SHALL add the schema version, platform, backend, grant, target and frame references, a reason code when the action was refused, failed or became unknown, requested and posted event counts for input, cleanup state, the names of assurance checks that passed before the last event, warnings, and start and settle times. Durable records and exports SHALL NOT contain raw typed text, key characters, window titles, clipboard content or screenshot bytes. A refusal decided before admission SHALL produce no journal row.

#### Scenario: Partial delivery
- **WHEN** a platform posts fewer input events than requested
- **THEN** the receipt is `unknown` with the requested and posted counts, a reason code and cleanup state, and the action is not repeated

### Requirement: Computer platform support evidence
(P2) Each platform configuration SHALL have an evidence level of `unsupported`, `implemented`, `fixture_tested` or `accepted`, compiled into the binary, reported in health and mirrored in documentation. On platforms added after macOS, input SHALL be enabled only for `accepted` configurations, and capture below `accepted` SHALL show its level in health and in the owner review. `accepted` SHALL require a recorded run on a real interactive desktop with a disposable fixture; compilation, hosted CI and fake adapters SHALL NOT count. Documentation and feature matrices SHALL claim support only for `accepted` configurations.

#### Scenario: Fixture-tested platform
- **WHEN** a configuration has passed only fixture tests
- **THEN** health reports `fixture_tested`, input is unavailable with `evidence_pending`, and the README does not list it as supported

### Requirement: macOS computer use compatibility
(P2) Adopting the platform-neutral contract SHALL NOT change macOS capture, input, consent, limits, lease, receipt outcome values or CLI behavior, except that chords the macOS adapter already refuses SHALL be refused before journal intent instead of settling unknown. macOS SHALL report authority `os_permission`, selectors `app` and `display`, capture availability equal to Screen Recording consent and input availability equal to Screen Recording and Accessibility consent together.

#### Scenario: macOS status after upgrade
- **WHEN** the owner runs status on macOS 14+ with both OS permissions granted
- **THEN** health reports mode `input_enabled`, the legacy fields report what they reported before, and existing `--app` and `--display` flows behave as before
