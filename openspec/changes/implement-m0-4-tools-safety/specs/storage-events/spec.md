## MODIFIED Requirements

### Requirement: Project identity from git
(P0) For a Location directory the system SHALL find the enclosing git repository and derive the project ID, in order, from:
1. the SHA-256 of the normalized `origin` URL (scheme, credentials, port and `.git` suffix stripped, lowercase host)
2. an ID cached in `<git-common-dir>/cyber-project-id`
3. the first root commit hash
4. a newly generated ID, written to `<git-common-dir>/cyber-project-id` for later runs

The worktree root SHALL be the project directory. Directories outside git SHALL map to project `global`. All worktrees of one repository SHALL share one project ID.

#### Scenario: Same repo, two clones
- **WHEN** `/a/repo` and `/b/repo` are clones of `git@github.com:acme/app.git` and `https://github.com/acme/app`
- **THEN** both resolve to the same project ID

#### Scenario: Outside git
- **WHEN** the Location is `/tmp/scratch` with no repository
- **THEN** the project is `global` with directory `/`

#### Scenario: New repository without commits
- **WHEN** a Location is in a repository with no origin and no commits
- **THEN** a generated project ID is cached in the git common directory and reused on the next run
