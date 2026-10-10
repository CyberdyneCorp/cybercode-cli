## MODIFIED Requirements

### Requirement: notebook_edit tool
(P1) `notebook_edit` SHALL accept `{ path, cell_id?, cell_index?, new_source, cell_type?, mode: replace|insert|delete }` for `.ipynb` files. It SHALL check the `edit` permission, preserve notebook metadata and outputs of untouched cells, and clear the outputs of edited code cells. Cell indices SHALL be zero-based. Both selectors, when supplied, SHALL identify the same cell. Replace/delete SHALL require a selector; insert without a selector SHALL append. Insertion at an index or cell ID SHALL occur before that cell. Edited code execution counts SHALL become null. Untouched cell IDs, source representation, attachments, outputs and extension fields SHALL remain unchanged. Opaque metadata and untouched outputs SHALL preserve exact numeric literals without floating-point conversion.

No-op replacement SHALL still check edit permission and SHALL preserve original file bytes. Mutations and no-ops SHALL revalidate file bytes under the host path lock after permission approval and refuse intervening user edits. Modern notebooks SHALL validate unique cell IDs and receive a fresh ID for an inserted cell; legacy notebook versions SHALL remain unchanged. The tool SHALL neither run notebook code nor start a kernel. Plan-mode catalog admission SHALL exclude it, and both ordinary and apply-patch-preferring model catalogs SHALL offer it.

#### Scenario: Insert markdown cell
- **WHEN** `mode` is `insert` with `cell_type: "markdown"` at index 0
- **THEN** a new markdown cell becomes the first cell and the other cells are unchanged

#### Scenario: Replace a code cell by ID
- **WHEN** a code cell is replaced using its unique ID
- **THEN** its metadata and ID SHALL remain unchanged while outputs are cleared and execution_count becomes null
- **AND** all other notebook fields and cells SHALL remain semantically unchanged

#### Scenario: File changes during approval
- **WHEN** a user changes notebook bytes while an edit or no-op request is awaiting permission
- **THEN** the request SHALL refuse after approval and preserve those user changes

#### Scenario: No-op is denied
- **WHEN** an unchanged replacement is requested while edit permission is denied
- **THEN** the tool SHALL refuse without rewriting the notebook

#### Scenario: Conflicting selectors
- **WHEN** cell_id and cell_index identify different cells
- **THEN** the request SHALL fail without changing notebook bytes


#### Scenario: Opaque numeric metadata
- **WHEN** notebook metadata, target-cell metadata or untouched outputs contain large numeric literals
- **THEN** editing a cell SHALL preserve those literals exactly, including values outside floating-point range
