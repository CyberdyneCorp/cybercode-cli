# Design

## Platform and native boundary

The owner selected macOS first on 2026-10-24. Require macOS 14+ for the initial native adapter and report a structured unsupported-platform/version result elsewhere. Define a narrow Rust port for capabilities, permission health, capture and bounded input operations. Use a native macOS adapter based on ScreenCaptureKit screenshots and CoreGraphics events with Accessibility trust checks. Pin maintained bindings/dependencies and include all required adapter artifacts in signed releases; distributed users must not install a compiler or Node runtime. Native adapter calls must not be synthesized by an LLM-written shell script.

Screen Recording and Accessibility are OS permissions granted to the actual server/helper identity, distinct from Cyber permission rules. Health reports both grants and the relevant binary identity. Only an explicit owner `setup` flow may request OS permission UI; a model call never triggers a hidden setup loop. Replacing/upgrading the binary can change OS trust behavior and must produce actionable diagnostics, not a bypass.

Primary references: [ScreenCaptureKit screenshot manager](https://developer.apple.com/documentation/screencapturekit/scscreenshotmanager), [Accessibility trust](https://developer.apple.com/documentation/applicationservices/1459186-axisprocesstrustedwithoptions), [CoreGraphics event posting](https://developer.apple.com/documentation/coregraphics/cgevent/post(tap:)). These establish platform APIs, not evidence that the project's adapter has been implemented or accepted.

## Explicit feature and control grants

Add `computer_use` to the existing feature registry, off by default and enableable only through trusted user/managed global configuration. The model-facing `computer` tool is offered only while enabled and natively supported. A status operation can explain missing capabilities without capturing pixels or injecting input.

An authenticated owner arms a specific Session and target through `cyber computer allow <id> --app <bundle-id>` (or a deliberate whole-display selection). Resolve and pin the actual application/process/window/display identity and show the capture/control scope before acceptance. Default grant TTL is 300 seconds, maximum 900 seconds; owner renewal is explicit. No grant survives a server restart. Only one Session may hold the host input lease at a time; conflicting acquisition fails rather than silently transferring control. Child agents, hub members and incoming messages inherit no control authority.

The grant requires an attached human and ordinary permission policy: `computer.capture` for observation, `computer.control` for input. Default decisions ask; Plan mode cannot inject input. Existing bypass/saved rules cannot arm a Session, replace OS consent or widen its target/expiry. A remote browser owner may review a grant only through the authenticated owner API and existing human-presence boundary; a project hub never receives an input endpoint or control credential.

Show an active-control banner in the TUI/dashboard with owner Session, target, expiry and Stop. `cyber computer stop`, owner API Stop, Session interrupt, logout/lease loss, grant expiry or OS revocation fences later input immediately. A process-level emergency-stop path must not wait for another model Turn or a busy tool queue. Native calls have bounded duration; release any pressed keys/buttons on cancellation where possible and report uncertainty if release cannot be established. Do not claim already-delivered events can be undone.

## Observation and coordinate contract

Capture returns a server-generated frame ID, Session/target/grant generation, monotonic timestamp, capture hash, image dimensions, logical desktop bounds, scale/rotation and display/window identity. Scale large images to a bounded image payload (default maximum edge 1920 pixels, at most 8 MiB encoded); retain exact coordinate transforms. Do not allow caller-provided output paths or arbitrary local image references to mint frames.

Input uses coordinates in the returned image frame and requires that frame ID. Default input-frame age limit is 10 seconds. Before dispatch, recheck grant, OS consent, current foreground app/target, display/window geometry and bounds. An expired frame, target/focus/geometry change or mismatched Session/generation requires a new capture and no input. Serialize capture/input admission per host and reject stale retries. Content can still change between observation and action; show that limitation and do not claim pixel freshness proves intent or correctness.

Window/app grants do not authorize global shortcuts, other applications, permission dialogs or credential prompts. Target-changing system actions require a separately owner-reviewed scope; never automatically expand from an app grant to the whole desktop. Bound drag paths, scroll distances, waits and key/text sizes; default action timeout is 5 seconds, waits at most 5 seconds and text at most 16 KiB. A multi-event click/drag/key chord is one admitted bounded action with cleanup, not a replayable open-ended script.

## Tool and execution lifecycle

Offer one typed `computer` tool with action-specific arguments: `status`, `screenshot`, `move`, `click`, `drag`, `scroll`, `type`, `key` and `wait`. Observation returns an image through existing image content handling plus frame metadata. Image-incapable models receive an explicit modality error. Text entry uses native Unicode/key events, not the clipboard. Key chords use an allowlisted symbolic-key grammar, never an arbitrary command string.

Every input action obtains its own admission/permission result under the grant and immutable displayed target. Bound action rate to 10 input actions/second and 300 per grant by default. Record durable native intent before delivery and settlement after the adapter responds. On crash after possible delivery, mark the action unknown and require fresh observation/owner reconciliation; never retype or reclick automatically on retry. A repeated action operation ID returns its known receipt, not another native event. Screenshots and on-screen text are untrusted external content and cannot alter policy, renew a grant, clear a denial or authorize a later action.

## Privacy and evidence

Store captures only in owner-protected managed artifact storage, bound to the Session and target, with a default one-day retention window (zero disables automatic pruning) and no implicit export/sharing. Authenticated previews use dedicated owner-authenticated, Session-bound native frame routes; screenshots do not enter general prompt-attachment storage or exports. Do not automatically capture unrelated displays, read clipboard contents, use microphone/camera or enumerate credential stores. App/window scoping reduces incidental exposure but is not automatic secret detection; the owner reviews what may be visible before capture.

Audit stores action type, target/grant/frame references, coordinate/key category, text length/digest, timing, permission and settlement. Raw typed text is excluded from durable tool-argument/log/error exports for this tool; actual input stays ephemeral until dispatch. Captures may still contain sensitive text and require explicit sharing/redaction. Avoid claiming this protects secrets already sent to a selected model/provider; the setup flow explains that authorized captures are supplied to that model.

## Public surfaces

`cyber computer status|setup|allow|stop` manages the native capability and grants. Owner control routes live under `/api/v1/sessions/{id}/computer` (the existing session group), and capability health under the existing tool group, with ordinary owner authentication, Location checks, request size/rate limits and idempotency. Model tool calls use the scoped runtime bridge, not the owner arming route. Add typed OpenAPI/SDK operations and visible TUI/dashboard state, target/expiry review, action result/unknown state and immediate Stop.

Update the authoritative CLI command registry to include `computer`. Feature/API health must distinguish disabled, unsupported platform/version, missing OS grants, unarmed, active and stopped states. Unsupported platforms must not fall back to unrestricted shell automation.

## Verification

Use a fake native adapter to test permission/grant/target/frame/coordinate logic and event settlement deterministically, including zero injected events on rejection. Native macOS acceptance uses an explicitly created test window with known text/click targets, Retina/non-Retina transforms and OS-grant denial/revocation. It must not type into a developer's unrelated application. Validate actual screenshots, pointer move/click, scrolling, text editing, key chords, drag and emergency stop. An adapter mock is not native acceptance; unsupported-platform tests must prove no OS side effects. Keep real desktop acceptance explicitly pending until the user opts into the controlled test and grants OS permissions.

## Owner API authentication across local transports

A model subprocess shares the owner's UID. Unix peer credentials establish OS identity, but cannot distinguish an owner client from an agent. All non-public Unix owner API requests therefore require the same existing owner Basic credential as TCP, in addition to same-UID validation; only trusted in-process Embedded calls bypass this boundary. Custom sockets must live under the owned hidden state directory, with existing non-symlink parents and no symlink leaf; clients refuse unsafe legacy socket registrations before transmitting credentials. This prevents an agent replacing a listener under a model-writable workspace and harvesting the owner credential. Authentication applies to custom socket paths as well as the default socket, and protects interactive presence, permission answers, configuration and native grants together. Existing public health/static resources and single-use PTY tickets keep their existing rules. First-party Unix clients send the existing credential; low-level unauthenticated clients must opt into the authenticated constructor.

Prepare and hide the complete owned state directory before child launches, protecting password rotation and default socket metadata. Explicit write/ancestor-replacement denial protects registered private roots inside otherwise writable projects. Filesystem read denial is not claimed to block Unix network connections, particularly with macOS network access enabled: server-side authentication supplies that boundary. The host reserves owner credential value digests before recovery or tools can launch. Final child environment filtering removes the reserved password variable and exact known-value aliases after allow/set and auxiliary environment overlays. This is not OS privilege separation against an arbitrary full-access owner shell.
