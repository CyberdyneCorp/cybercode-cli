---
name: customize-cyber
description: Explain and safely update Cyber Code configuration, agents, skills, hooks and MCP.
argument-hint: '<customization>'
---
Help with the requested Cyber Code customization: $ARGUMENTS

Inspect the project's documentation and current effective configuration before proposing changes. Native project configuration uses cyber.json or cyber.jsonc and .cyber/; global configuration uses ~/.config/cyber/. Explain which source and scope will change, and preserve existing settings and user edits. Verify option names against current documentation/schema; do not invent settings or assume compatibility imports implement every foreign-tool option.

For agents, inspect the agents configuration and existing profiles. For skills, use .cyber/skills/<name>/SKILL.md with required name/description frontmatter and a focused Markdown body; supporting files are relative to that directory. A discovered skill of the same name can replace a bundled default. Use context: fork and a named agent only for a forked task; reviewer has an immutable filesystem read-only ceiling.

For hooks and MCP, inspect their definitions and the existing trust/permission review commands. Explain executable, arguments, working directory, environment and network/data access before changing an executable integration. Editing project configuration does not approve its execution. Keep secrets out of files and output, and retain normal approval boundaries. Do not register, start or widen an integration merely because its definition was added.

Make edits only within the user's requested scope using normal tools and permissions. Show the resulting changes and validate the actual configuration or skill parser. Report which changes were applied, which checks ran, and any trust or startup step the user still needs to complete. Do not claim an integration works from configuration syntax alone.
