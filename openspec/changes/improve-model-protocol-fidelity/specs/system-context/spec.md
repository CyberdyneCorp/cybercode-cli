## ADDED Requirements

### Requirement: Base prompt coverage
(P1) Each base prompt SHALL be written for this product, and SHALL cover:
- identity and scope;
- reading before editing, and making the smallest sufficient change;
- which edit tool to use, and never editing files through shell redirection;
- fast search, and batching independent reads and searches as parallel tool calls;
- autonomy: carry the task through to verified completion, and ask only when a real decision is blocked;
- respecting existing uncommitted changes, and never reverting changes the agent did not make;
- caution before destructive actions;
- brief progress updates during long work;
- the final answer: lead with the result, refer to files as `path:line`, and add no filler.

The family addendum SHALL adapt this guidance to the family's edit tool and tool-calling habits. The base prompts SHALL carry a version, and a change of version SHALL start a new Context Epoch at the next Turn, as a change of model family does. No base prompt SHALL reproduce text from another product's prompts.

#### Scenario: OpenAI prompt names the patch tool
- **WHEN** the resolved family is OpenAI and the profile's `edit_tool` is `freeform_patch`
- **THEN** the base prompt tells the model to edit with `apply_patch` and to batch independent reads in parallel

#### Scenario: Prompt version change
- **WHEN** a Session's Context Epoch was built with base prompt version 1, and the binary ships version 2
- **THEN** the next Turn starts a new Context Epoch with the version 2 prompt
