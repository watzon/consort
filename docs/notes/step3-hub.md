# Step 3, hub part: notes

These notes are for the merge into `docs/DECISIONS.md`. The plan is [CONNECTORS-SKILLS-PLAN.md](../CONNECTORS-SKILLS-PLAN.md), section "Skills". Step 2 is in [step2-hub.md](step2-hub.md). The contract (`consort-proto`) did not change.

## What was built

All of it is in `crates/consort-hub`. `src/skills.rs` is now the folder `src/skills/`.

- **Skill directories** (`skills/directory.rs`). `skill_directory.sources`, `.add_source`, `.remove_source` and `.search`. The built-in sources are Smithery and Anthropic. The user can add a git repo or the URL of a `marketplace.json` file. The added sources are JSON in `meta` under `skill_directory_sources`. Listings and searches are cached. A source that fails is named in `unavailable`, and the other sources still answer.
- **Git fetch** (`skills/git.rs`). It fetches one skill folder at one commit with the `git` CLI. See "Fetch method".
- **Zip upload** (`skills/upload.rs`). It reads an uploaded zip in memory. It extracts nothing to the disk.
- **Checks** (`skills/package.rs`). Every source goes through the same checks. First the listing: paths, links, devices, submodules, the file count and the sizes. Then the content: the `SKILL.md` front matter, the name and description rules, `validate_skill_files`, the license and the scripts. The hub removes `hooks` from the `SKILL.md` front matter.
- **Preview and install** (`skills/install.rs`). `skill.preview` and `skill.install` for a directory entry, a git URL (with an optional path and ref) and an uploaded blob. A preview keeps what it fetched for one hour. An install right after it stores the same files, at the same commit. `approval_id` closes the install card: the card becomes allowed, the card gets the `skill_id`, and the session gets `card_answered`.
- **Tools.** `search_skills` and `add_skill` are built. `add_skill` fetches and checks the skill, then posts an install card with the safety facts. Allow on the card installs the files that the card showed. The `NOT_BUILT` list of `tools/mod.rs` and the placeholder arms of `api/mod.rs` are gone.
- **Update check** (`skills/updates.rs`). Once a day the hub asks the repo of each git or directory skill for its newest commit. A new commit that changes the skill folder becomes `SkillUpdateState::Available` with the file changes and `scripts_changed`. The new files are stored as blobs and as a version row, so `skill.files` and `skill.read_file` with `pending` work before the accept. Only `skill.update { accept_new_version }` moves the pin. The accept also takes the new `description`, `license` and `hooks_removed`.
- **Store.** `Hub::store_skill` goes through the same checks now, so `HubHandle::add_skill_from_files` removes hooks too. `Hub::store_prepared` stores a checked skill.
- **`HubHandle`.** `set_skill_directory_urls`, `allow_local_skill_sources` (tests only) and `check_skill_updates`. The variables `CONSORT_SMITHERY_URL` and `CONSORT_ANTHROPIC_SKILLS_URL` set the directory addresses at start.

## Sources

| Source | How the hub reads it | Docs |
|---|---|---|
| Smithery (built in, ID `smithery`) | `GET https://api.smithery.ai/skills?pageSize=100&q=...` to search, `GET /skills/{namespace}/{slug}` for one entry. No key. `gitUrl` gives the repo and the folder. | <https://smithery.ai/docs/api-reference/skills/list-or-search-skills>, <https://smithery.ai/docs/api-reference/skills/get-a-skill> |
| Anthropic (built in, ID `anthropic`) | The git repo `https://github.com/anthropics/skills`, read as a marketplace. `.claude-plugin/marketplace.json` lists plugins with `skills` folders. The hub reads the `SKILL.md` of each skill. | <https://code.claude.com/docs/en/plugin-marketplaces>, <https://code.claude.com/docs/en/plugins-reference> |
| A git repo that the user adds | The same as Anthropic. A repo with no marketplace file is a list of every `SKILL.md` in it. | The same |
| A `marketplace.json` URL that the user adds | A raw GitHub URL is read from its repo, so relative plugin paths work. Any other URL is fetched over HTTPS. Then only plugins with a `github`, `url` or `git-subdir` source and a `skills` list give entries. | <https://code.claude.com/docs/en/plugins/marketplace-reference> |
| skills.sh | Not a source. The app links to it, and the user pastes an `owner/repo` under Git URL. The hub never calls it: its API needs a Vercel OIDC token and its robots.txt disallows `/api/`. | |

What I saw in read-only checks on 2026-10-29:

- `GET https://api.smithery.ai/skills?q=pdf&pageSize=2` answered 200 with no key. Each skill has `namespace`, `slug`, `displayName`, `description`, `gitUrl` (for example `https://github.com/openclaw/openclaw/tree/main/skills/nano-pdf`), `totalActivations`, `verified` and `qualityScore`. `pagination.totalCount` was 159.
- `GET https://api.smithery.ai/skills/openclaw/nano-pdf` answered 200 with the same fields and `prompt`. Neither answer has an audit or a security scan field. So Smithery entries have no audits. The OpenAPI file lists `bearerAuth` for the API, but search worked without it.
- `anthropics/skills` has `.claude-plugin/marketplace.json` with five plugins. Each has `"source": "./"` and a `skills` list such as `./skills/pdf`. The repo has 19 folders under `skills/`.
- A partial fetch of `openclaw/openclaw` (`--depth=1 --filter=blob:none`) took 1.2 seconds and 2.0 MB. A second fetch of only the blobs of `skills/nano-pdf` took 0.4 seconds.

## Fetch method

The hub uses the `git` CLI. I chose it over the GitHub archive, because an archive is the whole repo (Smithery skills live in large monorepos such as `openclaw/openclaw`), and because it works for every git host.

1. `git init --bare --template=` in a new temp folder under `<data dir>/tmp` (mode 0700). With no templates, the repo has no hooks.
2. `git fetch --depth=1 --filter=blob:none --no-tags --end-of-options <url> <ref or HEAD>`. This gives the commit and its trees, but no file content. The commit SHA is the pin.
3. `git ls-tree -r -z <commit>:<path>`. The hub counts the entries before it fetches content.
4. `git fetch --filter=blob:none --no-write-fetch-head <url> <blob IDs>` for only the blobs of the skill folder. Then `git cat-file --batch-check` gives the sizes, `check_listing` runs, and `git cat-file --batch` gives the content.

Nothing is checked out. A file of the repo never becomes a file on the disk, a link is only a blob with its target text, and no filter, hook or submodule of the repo runs. Git runs with an empty environment (only `PATH` and the proxy and certificate variables stay), `HOME` in the temp folder, no system or global config, `GIT_TERMINAL_PROMPT=0`, no credential helper, `GIT_NO_LAZY_FETCH=1`, `core.hooksPath` set to the null device, `transfer.fsckObjects=true`, and `GIT_ALLOW_PROTOCOL=https`. Each command has a limit of 120 seconds. A fetch stops when the temp folder grows past 256 MiB. The temp folder is deleted when the fetch ends, and the hub deletes old `skill-fetch-*` folders when it starts.

The update check uses `git ls-remote` first. It fetches only when the commit is new.

## Security checks and their tests

| Check | Where | Tests |
|---|---|---|
| Path traversal (`..`) and absolute paths (`/x`, `\x`, `C:x`) are refused | `package::check_listing` | `package::tests::unsafe_paths_are_refused`, `upload::tests::traversal_and_absolute_paths_are_refused`, `flow_tests::an_upload_installs_and_bad_uploads_are_refused`, `tests/skills.rs` `an_uploaded_zip_installs_and_a_malicious_one_is_refused` |
| A link that leaves the folder (relative or absolute) is refused. A link inside the folder becomes a copy of its target file. Loops are refused | `package::check_listing` | `package::tests::links_inside_become_copies_and_links_out_are_refused`, `upload::tests::a_link_out_is_refused_and_a_link_in_is_a_copy`, `flow_tests::unsafe_git_skills_are_refused` (git links) |
| Devices and other special files are refused | `package::check_listing`, `upload.rs` (mode bits) | `package::tests::devices_submodules_and_limits_are_refused`, `upload::tests::a_device_is_refused` |
| Git submodules are refused | `package::check_listing` | `package::tests::devices_submodules_and_limits_are_refused` |
| File count, one-file size and total size limits, checked before content is read | `package::check_listing` | `package::tests::devices_submodules_and_limits_are_refused`, `upload::tests::a_zip_with_too_many_files_is_refused`, `flow_tests::unsafe_git_skills_are_refused` (501 files, a 5 MiB + 1 byte file) |
| Zip bomb: the declared sizes are checked before decompression, and each entry is read to at most its declared size plus one byte. Content of another size is refused | `upload::unpack_zip`, `ReadPlan::assemble` | `upload::tests::a_zip_bomb_is_refused_before_it_is_extracted` (6 MiB of zeros in a zip under 64 KiB, and five files of 4.5 MiB), `package::tests::content_of_the_wrong_size_is_refused` |
| Upload: at most 25 MiB, the size must match the request, the hash must match the blob | `install::fetch_install_source` | `flow_tests::an_upload_installs_and_bad_uploads_are_refused` |
| Encrypted zip entries are refused. A file that is not a zip is refused | `upload::unpack_zip` | `upload::tests::not_a_zip_is_refused` |
| Only `https` URLs. No user name or password in the URL, no `ext::`, `ssh` or `git@`. `file` and `http` only with `allow_local_skill_sources` (tests) | `git::parse_git_url`, `GIT_ALLOW_PROTOCOL`, `directory::locate`, `reqwest` `https_only` | `git::tests::git_urls_are_read_and_checked`, `flow_tests::unsafe_git_skills_are_refused`, `directory::tests::sources_are_located` |
| A ref or a path cannot be a git option (`--upload-pack=...`) or leave the repo | `git::check_ref`, `git::normalize_repo_path`, `--end-of-options` | `git::tests::refs_and_paths_are_checked`, `flow_tests::unsafe_git_skills_are_refused` |
| Claude-specific `hooks` are removed from `SKILL.md`, and `hooks_removed` is set | `package::strip_hooks`, `package::prepare` | `package::tests::hooks_leave_the_front_matter_and_the_rest_stays`, `package::tests::prepare_reads_the_facts_and_removes_hooks`, `flow_tests::a_git_skill_is_previewed_pinned_and_installed_without_hooks` |
| A skill whose audit failed is blocked: preview shows `blocked_reason`, install fails with `skill_blocked`, `add_skill` refuses | `install::install_fetched` | `flow_tests::a_failed_audit_blocks_the_skill`, `skills::tests::a_skill_with_a_failed_audit_is_blocked` |
| A skill with scripts stays off, also when `enabled` is true | `install::install_fetched` | `flow_tests::a_git_skill_is_previewed_pinned_and_installed_without_hooks`, `flow_tests::add_skill_posts_an_install_card_and_allow_installs_it` |
| Temp folders go away | `tempfile::TempDir`, start-up clean-up | `flow_tests::unsafe_git_skills_are_refused` (no folder stays after many fetches) |
| Time and size limits on every fetch | `run_git` (120 s, 256 MiB), `get_json` (20 s, 2 MiB) | Not tested with a slow server. The limits are constants |
| Nothing from a skill runs | No checkout, no hooks, no filters, files only as bytes | By design. No test can prove an absence |

No test uses the network. The git repos are made in the test and used as `file://` URLs, with `uploadpack.allowFilter` and `uploadpack.allowAnySHA1InWant` on, so the partial fetch takes the same path as on GitHub. The directory is a fake axum server (`skills/fake.rs`), which `tests/skills.rs` includes with `#[path]`.

## Decisions to merge into DECISIONS.md

| Area | Decision | Why |
|---|---|---|
| Hub | The hub fetches skills with the `git` CLI: a shallow (`--depth=1`) partial (`--filter=blob:none`) fetch into a temp bare repo, then a fetch of only the blobs of the skill folder. No checkout. | Smithery skills live in large monorepos. This fetches a few MB of trees and only the skill's files, from any git host. A GitHub archive is the whole repo |
| Hub | The hub needs `git` on its machine for git and directory skills. Without it, the fetch fails with "git is not installed on the hub machine". | The CLI is the fetch method |
| Hub | Only public repos over `https`. Git gets no user config, no credential helper and no terminal prompt. | No secret of the hub machine reaches a fetch of untrusted content. Private repos are left for later |
| Hub | A link inside the skill folder becomes a copy of its target file. A link to a folder, a loop, or a link that leaves the folder is refused. | Skill files have no link type. The plan refuses only links that leave the folder |
| Hub | Git submodules and special files (devices, pipes, sockets) are refused. | The hub does not fetch submodules, and a skill holds only files |
| Hub | A zip with one top folder that holds `SKILL.md` uses that folder as the skill root. `__MACOSX/` and `.DS_Store` are left out. | Zip tools on macOS add them, and users zip the folder itself |
| Hub | `hooks` removal: the hub removes the top-level `hooks` key and its lines from the `SKILL.md` front matter. Other files stay. | Claude Code runs skill hooks from the front matter, also for plugin skills, which is how the worker delivers them (<https://code.claude.com/docs/en/hooks#hooks-in-skills-and-agents>) |
| Hub | License: the `license` of `SKILL.md`, then a license file at the root (`LICENSE`, `LICENSE.md`, `LICENSE.txt`, `LICENCE*`, `COPYING`) read as an SPDX ID for 12 common licenses or "See <file>", then the directory's license. | The proto says "from SKILL.md or a license file" |
| Hub | Entry IDs: `smithery:<namespace>/<slug>`, `anthropic:<folder>`, and `m<8 hex of the source ID hash>:<folder>` for an added source (`<repo>#<folder>` for a plugin in another repo). `add_skill` reads a source with one of these prefixes as an entry, and anything else as a git URL. | `add_skill` has one `source` text. `owner/repo` and Smithery's `namespace/slug` look the same, so entry IDs need a prefix |
| Hub | Directory skills follow the default branch. The ref in Smithery's `gitUrl` (`/tree/main/`) is not kept. | `SkillSource::Directory` has no ref. Install and the update check must follow the same branch |
| Hub | skills.sh is not in `skill_directory.sources`. | It has no search that the hub may call. The desktop already says so and links to it |
| Hub | Caches: a repo or marketplace listing for one day, a Smithery search for one hour (64 searches kept), a failed source for 60 seconds. A failed fetch gives the old copy when there is one, and the source is then not in `unavailable`. | The plan asks for cached results, and a short outage should not hide a source |
| Hub | Search order: Anthropic, then the added sources, then Smithery. Local sources filter on all words in the name, description, author and folder. The cursor is an offset into the merged list. Smithery is asked for one page of 100. | One list for the dialog, with the curated sources first |
| Hub | `add_source` reads the source once and refuses one with no skills. Adding a source that is there already returns the list unchanged. Built-in sources cannot be removed (`invalid`). | The user learns at once when a URL does not work |
| Hub | A fetch that fails (network, unknown repo, branch or folder) is `not_found` with git's last error line. A skill that breaks a rule is `skill_invalid`. A bad URL, ref or path is `invalid`. | The proto has no code for a failed fetch. `not_found` is what the user sees most: a wrong URL or branch |
| Hub | Previews are kept for one hour, at most 16, by the install source (an upload by its hash and size). An install card keeps its own preview. Allow on a card installs that preview, or fetches the card's commit again. If the repo no longer has it, the install fails with `conflict`. | The user installs what the card and the dialog showed |
| Hub | The update check runs when the last one is more than one day old. The hub looks every hour, and the time of the last check is in `meta` (`skill_updates_checked_at`). | "Once a day", and a restart does not run it again early |
| Hub | A new commit with the same skill files is not a new version. The pin does not move, and the hub keeps that commit in memory so that it does not fetch it again. | "Only approval moves the pin". A commit elsewhere in a monorepo is not a change to review |
| Hub | `scripts_changed` is true when an added or changed file is a script (executable, or under `scripts/`). A removed script does not count. | The proto: "adds or changes a script" |
| Hub | A new version with another `name` is `check_failed` with a message. | The name is the skill's key and its folder name on the worker |
| Hub | The accept takes the new `description`, `license` and `hooks_removed` from the stored version. `meta` keeps `skill_hooks_removed:<content hash>` for each stored version whose hooks the hub removed. | The version row holds only the file list |
| Hub | A failed update check sets `check_failed` and keeps the installed version. The next check that works sets `up_to_date` again. | |
| Hub | `search_skills` returns at most 20 entries. | The result goes into the model's context |
| Hub | `add_skill` refuses (`refused`, with the `skill_id`) a skill whose name is installed already, and a blocked skill. It uses the same default scope as `connect_service`. | The card would fail on Allow |
| Hub | `zip` and `flate2` (its default deflate backend) and `tempfile` are hub dependencies now. All three were in the workspace and the lock file. | |

## Proto changes

None.

## What is left

- Private repos and `ssh` URLs. The hub has no git credential of its own.
- A Smithery skill on a branch other than the default branch installs from the default branch, or fails when the folder is not there.
- Plugins in a `marketplace.json` with an `archive`, `npm` or `command` source give no entries. A plugin in another repo gives entries only for the `skills` folders that it lists, and the hub does not check that each one is a skill until the preview.
- No directory gives audit results now. The pipeline carries audits from an entry to the card and the skill, and blocks a failed one, but no source fills them.
- The time limits of git and HTTP have no test with a slow server.
- `tool.list` shows `search_skills` and `add_skill` to the sessions that the proto names. The desktop can now drop its `unsupported` text for these requests, and can add the controls for `add_source` and `remove_source` (step2-desktop.md, "What is left").
- The "Placeholders for step 2" list in CONNECTORS-SKILLS-PLAN.md can go: every entry is built.
