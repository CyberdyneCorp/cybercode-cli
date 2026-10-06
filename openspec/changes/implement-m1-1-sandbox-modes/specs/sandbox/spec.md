## MODIFIED Requirements

### Requirement: Credential masking
(P0) Sandboxed processes SHALL receive an environment filtered by `sandbox.env`. Variables matching `*_TOKEN`, `*_KEY`, `*_SECRET`, `*PASSWORD*`, `AWS_*`, `GITHUB_TOKEN` and the provider credential variables known to `provider-credentials` SHALL be removed unless listed in `sandbox.env.allow`. Credential files (`~/.aws`, `~/.config/gh`, `~/.netrc`, `~/.docker/config.json`, `~/.ssh`) SHALL be unreadable unless listed in `sandbox.readable_paths`.

#### Scenario: API key hidden from shell
- **WHEN** the server environment has `OPENAI_API_KEY` and the model runs `env` in the sandbox
- **THEN** the output does not contain `OPENAI_API_KEY`

#### Scenario: Platform-aware credential names
- **WHEN** environment filtering compares a variable with a known credential name or explicit allowed name
- **THEN** Windows SHALL use ordinal case-insensitive name comparison and Unix SHALL use case-sensitive comparison
- **AND** Unicode normalization SHALL NOT create an unintended allowed-name match
- **AND** a failed name comparison SHALL NOT authorize an exception or expose a potentially matched credential

#### Scenario: Catalog credentials reach tool environment filtering
- **WHEN** a sandboxed built-in command prepares its environment
- **THEN** filtering SHALL include credential environment names from every provider in the loaded catalog, including disabled or unavailable providers
- **AND** filtering SHALL additionally include credential names in the current location's provider configuration
- **AND** omission or replacement of a provider in location configuration SHALL NOT remove loaded catalog names from filtering
- **AND** filtering SHALL inspect credential names without exposing their values
- **AND** explicit `sandbox.env.allow` exceptions SHALL retain their platform-aware semantics
- **AND** explicit full-access execution SHALL preserve its unfiltered environment behavior

### Requirement: Windows enforcement
(P1) On Windows the system SHALL run sandboxed processes with a restricted token in an AppContainer, with ACLs granting write access to writable roots only. When unavailable, it SHALL report `sandbox unavailable` as on Linux.

#### Scenario: Windows restricted write
- **WHEN** a sandboxed PowerShell command writes `C:\Windows\Temp\x` outside the writable roots
- **THEN** access is denied

#### Scenario: Scoped file access cannot change security
- **WHEN** an AppContainer receives read or write access to an invocation-scoped file
- **THEN** it SHALL NOT receive permission to change that file's DACL or owner
- **AND** native validation SHALL distinguish these denied security rights from allowed ordinary file writes

#### Scenario: Verify Windows process identity before execution
- **WHEN** a Windows sandbox process is created
- **THEN** its primary thread SHALL remain suspended until parent-owned job assignment succeeds
- **AND** its token SHALL be verified as an AppContainer with the invocation's exact SID before user code can run
- **AND** setup or verification failure SHALL terminate the suspended process without falling back to unrestricted execution
- **AND** the child SHALL receive an explicit environment rather than inherit the parent's environment

#### Scenario: Explicit Windows standard streams
- **WHEN** a Windows sandbox invocation redirects stdin, stdout and stderr
- **THEN** the launcher SHALL duplicate only the three supplied stream handles for inheritance and specify an explicit handle list
- **AND** unrelated inheritable handles SHALL NOT be passed to the child
- **AND** redirected streams SHALL preserve input bytes, EOF and separate Unicode stdout/stderr output
- **AND** temporary parent duplicates SHALL be released after process creation or setup failure

#### Scenario: Windows private temporary storage
- **WHEN** a Windows sandbox invocation prepares its temporary storage
- **THEN** TEMP and TMP SHALL identify an invocation-owned directory beneath that profile's storage
- **AND** its ACL grant SHALL apply only to that identity and its private descendants
- **AND** inherited profile grants on a newly created private directory SHALL be replaced with an owned, revocable grant while preserving unrelated ACL entries
- **AND** caller-supplied TEMP/TMP values SHALL NOT redirect temporary storage outside that directory
- **AND** command-owner cleanup SHALL revoke the grant and remove the invocation-owned temporary data
- **AND** failed process creation SHALL release temporary storage and profile ownership
- **AND** concurrent command owners for the same profile SHALL be refused before temporary-storage changes
- **AND** preexisting nonempty or reparse-point temporary storage SHALL be refused without claiming cleanup ownership of that storage

#### Scenario: Invocation-owned Windows profile and ACL leases
- **WHEN** a Windows command invocation prepares an AppContainer identity and direct-object ACL grants
- **THEN** it SHALL create a fresh profile rather than reuse an existing identity
- **AND** ACL cleanup SHALL target the retained original object handle and remove only that profile's entries from the current ACL
- **AND** another identity's grant added during the lease SHALL survive cleanup
- **AND** profile deletion SHALL refuse while a grant lease still owns the identity
- **AND** reparse points and overlapping grants for the same identity SHALL be refused
- **AND** direct-object grant preparation SHALL refuse paths through reparse-point ancestors and retain checked ancestor handles until the original object is opened and its ACL updated
- **AND** relative, parent-traversing and unsupported device paths SHALL be refused before ACL mutation
- **AND** implementing these ownership primitives alone SHALL NOT report Windows confinement as available

#### Scenario: Successful invocation identities are single-use
- **WHEN** a top-level command has started under an AppContainer profile
- **THEN** the profile SHALL NOT launch another top-level command after the first owner completes or is cancelled
- **AND** the next invocation SHALL create a fresh profile rather than reuse earlier identity grants
- **AND** setup failure before successful resume SHALL release the reservation without consuming the profile
- **AND** grants and successful resume SHALL be serialized so its scope cannot widen after command execution starts
- **AND** revocation and profile cleanup SHALL remain possible after execution

#### Scenario: Relocatable object ACL cleanup
- **WHEN** recursive-root preparation records a scoped object by Windows file identity
- **THEN** it SHALL verify reopening that identity on its original volume before installing a grant
- **AND** cleanup SHALL reopen and reverify the original object rather than trust its previous path
- **AND** cleanup of a deleted identity SHALL NOT mutate a replacement object at its previous path
- **AND** ordinary directory moves SHALL NOT require keeping every child object handle open
- **AND** identity-based ACL updates SHALL NOT automatically propagate through unchecked children
- **AND** cleanup SHALL remove only this invocation's explicit or inherited entries while preserving unrelated entries
- **AND** this primitive alone SHALL NOT enable recursive roots or report Windows confinement as available

#### Scenario: Recursive root inventory before ACL mutation
- **WHEN** Windows filesystem preparation inventories an existing local directory tree
- **THEN** it SHALL retain checked ancestor and descendant directory handles during enumeration
- **AND** it SHALL record and verify each observed object's file and volume identity without changing any ACL
- **AND** unsupported names, reparse points, multiply linked files, identity reopening failures and the caller's object limit SHALL abort preparation and release all pins
- **AND** revalidation SHALL refuse deleted or changed recorded objects rather than accept replacement paths
- **AND** directory pins SHALL be released when the inventory owner is dropped
- **AND** this inventory SHALL NOT claim an atomic namespace snapshot, account for later-created children or enable recursive grants by itself

#### Scenario: Owned existing-tree grants
- **WHEN** a fresh Windows profile grants access to inventoried existing tree objects
- **THEN** preparation SHALL hold the profile's grant/start lock and checked directory pins through the grant transaction
- **AND** every grant SHALL target its verified object identity without implicit child propagation
- **AND** all owned existing identities SHALL be revalidated before preparation returns successfully
- **AND** a later setup failure SHALL revoke every prior grant owned by the transaction while preserving preexisting and unrelated grants
- **AND** cleanup SHALL attempt every owned object and retain failed revocations for retry
- **AND** ordinary directory movement and path replacement SHALL NOT redirect revocation onto replacement objects
- **AND** post-start preparation SHALL be refused before inventory or ACL mutation
- **AND** existing-object ownership SHALL NOT claim future-child inheritance, exclusions, recursive runtime policy or Windows sandbox availability

#### Scenario: Existing-object exclusion policies
- **WHEN** Windows preparation applies read-only and unreadable exclusions to inventoried existing objects
- **THEN** exclusion paths SHALL resolve to verified identities in the inventory before any ACL mutation
- **AND** missing, unsupported or out-of-tree exclusions SHALL fail preparation
- **AND** exclusions SHALL apply to recorded descendants by parent identity relationships, with unreadable taking precedence over read-only
- **AND** read-only objects SHALL receive invocation-specific write/delete/security-right denies while retaining allowed reads
- **AND** writable objects SHALL deny DACL/owner changes and parent delete-child bypass while retaining ordinary deletion through each writable child's own grant
- **AND** unreadable objects SHALL deny all file access for the invocation identity
- **AND** each object's allow/deny entries SHALL be installed together while preserving unrelated ACEs
- **AND** rollback and cleanup SHALL remove only the invocation identity's entries
- **AND** existing-object exclusions SHALL NOT claim protection of absent names or future children, or enable Windows runtime confinement by themselves

#### Scenario: Windows command process-tree lifetime
- **WHEN** a native Windows command leaves descendants running and its command owner exits normally or is terminated
- **THEN** the descendants SHALL terminate with the owner
- **AND** process-tree setup failure SHALL refuse to launch the command
- **AND** successful command completion SHALL preserve its exit code

#### Scenario: Owned asynchronous AppContainer wait
- **WHEN** an AppContainer command is awaited asynchronously
- **THEN** the wait future SHALL own the command's process and job lifetime without an uncancellable blocking wait worker
- **AND** dropping the unpolled future or aborting its task SHALL release that owner and terminate the command
- **AND** normal completion SHALL preserve the command exit code and release its temporary storage and profile reservation

#### Scenario: Windows parent dies during command launch
- **WHEN** the server dies before or after assigning a trusted launch helper to its parent-owned job
- **THEN** user code SHALL NOT start before successful assignment
- **AND** an unassigned helper SHALL refuse execution when its private parent channel closes
- **AND** assigned helpers and command descendants SHALL terminate when the server-owned job handle closes
- **AND** cancellation or dropping the command owner SHALL terminate descendants without relying on a reusable PID

#### Scenario: Full-access Windows command ownership
- **WHEN** a foreground shell command runs on Windows with explicit full-access policy
- **THEN** its process tree SHALL retain the same parent-owned job lifetime boundary
- **AND** timeout and cancellation SHALL terminate live descendants
- **AND** a missing process-owner helper SHALL refuse execution rather than bypass ownership
- **AND** this lifetime boundary SHALL NOT report AppContainer confinement as available

### Requirement: Network isolation and allowlist proxy
(P0) Sandboxed processes SHALL have no direct network access by default (`sandbox.network: "proxy"`). Their HTTP(S) traffic SHALL go through a local proxy that allows only domains in `sandbox.allowed_domains`, which defaults to the package registries `registry.npmjs.org`, `pypi.org`, `files.pythonhosted.org`, `crates.io`, `static.crates.io`, `proxy.golang.org` and `github.com`. A connection to another domain SHALL raise a `network` permission request with the domain as resource, and an approval SHALL add the domain for the Session (or persist it with `always`). `sandbox.network: "off"` SHALL block everything, and `"on"` SHALL allow everything.

#### Scenario: Unknown domain prompts
- **WHEN** a sandboxed `curl https://example.org` runs with the default allowlist
- **THEN** a `network` request for `example.org` is raised and the connection waits up to 120 s for a decision
