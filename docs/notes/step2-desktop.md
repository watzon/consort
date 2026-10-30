# Step 2, desktop part: notes

These notes are for the merge into `docs/DECISIONS.md`. The plan is [CONNECTORS-SKILLS-PLAN.md](../CONNECTORS-SKILLS-PLAN.md). The work is in `crates/consort-client` and `crates/consort-desktop`, with new size tokens in `consort-theme` and `design/structure.json`. The contract (`consort-proto`) did not change.

## What was built

### Client (`consort-client`)

- **Cache** (`cache.rs`). `connections` and `skills` from the `all` snapshot and the feed, with deletes and the guard against an older snapshot. `merge_connection` and `merge_skill` take the answer to a request, unless the feed brought a newer one. `Extensions` holds what the client loads on request: the Composio status, the catalog, the tools of services, the skill directory and the logos. A reload keeps it. A sign-in feed entry shows when a sign-in card names it.
- **View state** (`view.rs`, `connectors.rs`, `skills.rs`). `connections`, `skills`, `composio`, `connector_catalog` (paged, with a search serial so an old answer does not replace a new one), `service_tools`, `skill_directory` (paged). `ChannelView.disabled_connections` and `disabled_skills`. `HarnessAccountView.extras_enabled` and `extras`. `ApprovalView.card` (a `HubCardView` for the five hub cards) and `button_labels` (the labels of the hub's options).
- **Reducers** (`extensions_view.rs`). Hub entities to view records, and view records back to the wire types.
- **Pure functions with tests** (`connectors.rs`, `skills.rs`): `use_in_matches` (checked against the proto rule), `matching_channels`, `use_in_short`, `use_in_sentence`, `use_in_detail`, `use_in_match_note`, `approvals_sentence`, `rule_note`, `channel_extensions` (the effective set with the exceptions and the items not given), `connectors_context` and `skills_context` (the blocks a session starts with), `filter_services`, `connections_of_service`, `connect_intro`, `suggested_connection_name`, `handle_for`, `connection_row_detail`, `connection_status_line`, `audit_summary`, `skill_row_detail`, `source_line`, `repo_name`.
- **Commands** (`command.rs`, `hub/extensions.rs`) for every new client request: the Composio key and status, the catalog (search, more, categories, the tools of a service), connect, reconnect, update, disconnect, the audit log, the channel exceptions, the skill directory (search, more, sources), preview, install, update, remove, files, read a file, and the account extras. An `unsupported` answer says "This hub cannot ... yet. Update the hub, then try again."
- **Logos** (`hub/extensions.rs`, `run_logos`). The client loads each logo URL once: `https` only, at most 256 KiB, SVG, PNG, JPEG, WebP or GIF. No new dependency.
- **Mock** (`mock_extensions.rs`, `mock_backend_ext.rs`). Six connections, five skills, the Composio plan, a catalog of eleven services, the skill directory, the extras of Claude Personal on bigboi, the channels acme and hiring (tagged work), and one of each hub card. The mock answers every new command the way the hub does, with a browser sign-in that ends after 2.5 seconds.

### Desktop (`consort-desktop`)

- **Settings > Connectors** (`ui/connectors.rs`). The list with logo, name, identity and Use in, or "Needs a new sign-in" in `status.failed`, and the row menu (Reconnect, Copy handle, Disconnect). The Composio block under the list: usage bar, usage line, Change key. A page to add the key when the hub has none. The connector page: header with Reconnect and the menu, Name (saved on Enter or when the field loses focus) and Use in, the three approval selects with the tool count of each kind, and "What the assistant sees".
- **Add a connector** (dialog). Search, the category chips, and a grid that scrolls inside the dialog with fades, paged with Show more.
- **Connect** (dialog). Name with the handle the assistant sees, Use in, the approval rules, then Continue in browser (the system browser opens the redirect URL, and the dialog waits for active) or the credential form from `AuthScheme.fields` (secret fields masked). Active closes the dialog and opens the connector page. Needs a new sign-in or failed shows in the dialog. Reconnect uses the same dialog for a scheme with a form.
- **Settings > Skills** (`ui/skills.rs`). The list with the state dot and the scripts badge, the row menu (Turn off or Turn on, View files, Remove), and the skill page: On, Use in, Scripts, Security audits, Updates (Update to a new commit), and "What the assistant sees". The files dialog lists the files and shows one.
- **Add a skill** (dialog). Directories (search, results with the audit state and Add, Added or Blocked), Git URL and Upload. Add checks the skill first (`skill.preview`) and shows the facts, then Add skill installs it. An `unsupported` hub shows the error in place of the results, and a check shows it as the dialog error.
- **Connectors and skills in a channel** (`ui/channel_extensions.rs`). Since the redesign of 2026-10-30 this is the Connectors tab of the Channel panel (`render_connectors_tab`), and the dialog is gone. The channel row menu item "Connectors and skills" opens it (`open_channel_connectors`). A switch for each item that Use in gives. Change for an item that it does not give, which opens that item's page in Settings.
- **Account settings** (same file). From the account name in Settings > Workers and from the account menu. Name, the provider extras switch, the counts and names, Show the list, and the text of `cannot_turn_off`. Sign out, Remove account, Done.
- **Hub cards** (`ui/hub_cards.rs`). A connector card with the fields in a readable form and the tool slug, a management card with the change and its effect, a connect card (Continue in browser, Change settings, Not now), a sign-in card and an install card with the safety facts. A connector card from a task shows inside its task card. Every answer is `approval.answer` with the hub's option.
- **Mock screens**: `--screen` connectors, add-connector, connect, connectors-in-channel, channel-connectors, skills, add-skill, add-skill-unsupported, account-settings, hub-cards, everything-manages.

### Tests

- Client: the pure functions, the scope rule against the proto rule, the cache (snapshot, feed, stale answers, reload), the reducers, the mock commands, and a fake hub test that checks the wire: a hub card answer is `approval.answer` with `allow`, a channel switch is `channel.set_connection_enabled`, `connection.connect` sends the scope and the rules and the status follows the feed to active, and an `unsupported` directory and preview.
- Desktop (headless windows): every page and dialog opens, the connect flow from connecting to active, card answers, the channel switch, the extras switch, an unsupported hub in Add skill, and the sign-in card.

## Where it differs from the Paper design, and why

- The connector card has the buttons Allow and Don't, not Send, Edit first and Don't send. The task says to answer with `ApprovalDetails::hub_card_options`, and the protocol has no way to edit the arguments.
- The note at the top right of a connector card says "Create and change tools ask every time", not "Sending asks every time". The card knows the tool kind, not the verb of the tool.
- The approval rows say what tools of the kind do in general ("Tools that create, send or change things. 27 tools."), not examples such as "Draft, send, label, move". The catalog gives no short text per kind.
- In "What the assistant sees", the handle of the open connector is not in the accent. The block is one selectable text, and a selectable text has one color.
- The Connect dialog shows the three approval rules as selects, not as three read-only values. The user can set them before the first call.
- Use in has two choices: Every channel and Channels tagged. "Channels tagged" opens a menu of tags to add or take away. A scope of single channels (from a channel assistant) shows its channel names in the hint. The designs have no control for it.
- The Composio block shows the key hint when the hub reports no plan or usage. The hub notes say that `composio.status` has neither yet.
- The channel header has a new button (link icon and a count) that opens the channel dialog. "Connectors in a channel" has no entry point in the header.
- The channel row menu has "Connectors and skills" after "Worker tags", and the account menu has "Account settings" first. AGENTS.md rule 2 lists both, and the menus of connection and skill rows.
- In the Add skill dialog, the tabs sit at the right of the search on the Directories tab, and at the right of a line of their own on the other tabs.
- The skill directory entry has no list of scripts, so the result rows have no "scripts" badge.
- The sign-in card does not show the sign-in steps inline. It opens the sign-in dialog of Settings > Workers on the same sign-in, so the flow is the same everywhere, and the card says how the sign-in goes.
- `--mock` adds two channels, acme and hiring, to Personal projects. The connect card of the design is in Everything only with `--screen everything-manages`, so the usual Everything stays as designed. hiring has the other cards.

## Decisions the plan did not settle

| Area | Decision |
|---|---|
| Client | The catalog, the skill directory, the Composio status and the logos are client state in the cache, not entities. A reload keeps them. |
| Client | A search sends a serial. An answer for an older search is dropped. |
| Client | A page holds 30 services or entries. |
| Client | The logo loader asks for each URL once. A logo that does not load stays a monogram until the app restarts. |
| Client | A sign-in feed entry shows when a sign-in card names it, as well as when this client started it. |
| Client | `SkillInstallInput::Upload` refuses at once with a message to use a git URL: the client cannot upload a blob yet. |
| Desktop | A new connection starts with the tags of the open channel, or every channel from Everything or from a channel with no tags. |
| Desktop | The Connect dialog suggests the service name as the name only when no connection has that name. |
| Desktop | Continue in browser on a connect card answers `allow`. The app opens the redirect URL when the card names the connection and the connection has a URL. |
| Desktop | Change settings on a connect card opens the Connect dialog with the card's settings. Its connect sends `connection.connect` with the `approval_id`. |
| Desktop | Sign in on a sign-in card with a method answers `allow`, and the hub starts the sign-in. Without a method, the app starts the sign-in, and answers `allow` when the account signs in. |
| Desktop | The mock opens no browser. It shows a notice in place of the sign-in page. |
| Desktop | Turning a skill with scripts on is the user's OK for its scripts. The page says so under On. |
| Theme | New tokens: `layout.dialog_catalog` 760, `component.connector_tile` 44, `connection_field` 240, `catalog_list` 310, `skill_results` 300, `usage_bar` 4, `files_list` 200, `files_view` 360. design-system.md lists them. |

## What is left

- Upload of a skill zip: the client needs the blob upload (`auth.http_token` and the blob endpoint) before `SkillInstallInput::Upload` can send `blob_sha256`.
- Add and remove a marketplace source: the client commands exist, but the Add skill dialog has no control for them yet.
- The audit log of a connection (`connection.calls`): the client command exists, and no page shows it yet.
- A review of a new skill version with its file diff (`skill.files` with `pending`): the page shows the changed files and Update, but no diff.
- The Reconnect of a connection with a form scheme needs the service in the catalog to know its fields. When the catalog has not loaded, Reconnect goes the browser way.
- The scroll fades of the dialog lists update on a scroll. Before the first scroll, the bottom fade may not show.
- A live run against a hub with the step 2 hub work and a real Composio key (step 4 of the plan).
