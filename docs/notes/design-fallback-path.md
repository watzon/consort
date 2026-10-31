# Design notes: fallback path

These are the Paper designs for a model fallback path that the user chooses. The Paper file is `01M6342D512B1CKH0D29HMDN8V`, page "Redesign, Oct 30". The section header "Section: 11 Fallback path" (`O6A-0`) is at worldX 0, worldY 12688. The four artboards are in one row at worldY 12808, from worldX 0, with an 80 px gap. Another agent works above worldY 12600.

No code changed. The proposal is a design only. Nothing here is built or decided.

Revision of 2026-10-30: the owner moved all accounts out of Settings > Models. The Accounts section of B1 is gone. Accounts, sign-ins, harness tools and API keys are now in a new item "Providers". B1 gained a "Command check" section for Auto mode. The new artboards and their decisions are in [design-providers.md](design-providers.md). Where this note and that note disagree, that note is newer.

The example data is the same in all four artboards. The user is Chris. The accounts are Claude Personal, Claude Work, Codex Work and Codex Personal. Codex Personal is signed out. The workers are the hub and bigboi, and truenas is offline, as in the sidebar. A worker never appears next to a model or an account. The only worker names are the task title and the composer line "Runs on bigboi", because they say where the tools run. The times are from 3:52 PM to 4:14 PM. The limit of Claude Personal resets at 4:10 PM. The numbers are example data.

## What the design proposes

The hub owns all providers, accounts and harness CLIs. A worker is only a machine where work runs. A worker has no account and no sign-in. The model choice and the place where work runs are two separate ideas.

- One ordered path of model selections. A selection is an account, a model and an effort.
- Settings > Models is the one place for model choices. It has two sections: "Fallback path" and "Command check". It has no accounts. The accounts are in Settings > Providers, the item before Models in the same group.
- One global path in Settings. A channel row "Fallback path: Same as Tutti" follows the inherit pattern of the model. The editor of a channel path is not drawn.
- The first step is always the model of the channel. It is fixed. The user cannot move it or remove it.
- The built-in row "Other accounts with the same model" is on by default. When it is off, it replaces the switch "Move to another account at a usage limit" in Settings > Hub. The old switch goes away.
- The user adds steps with the existing model picker. The user drags the handle to change the order, and can remove a step.
- The last row is fixed: "Stop and show the error".
- A step on another harness works only if the user adds it. The picker asks for a confirm, because the work and its context go to that provider.
- Triggers: a usage limit, and a provider outage or overload after retries. A sign-in problem is not a trigger.
- Consort skips a step when its account is at a limit or signed out, or it cannot enforce the approval mode. Consort never goes below the approval mode. Each skip shows in the activity detail with its reason.
- A worker is never a reason to skip a step. The channel tags still choose the worker of a task. An offline worker stops the work of that task. It does not change the model or the path.
- The assistant goes back to its first choice on its own when the limit resets. A task stays where it moved, because it holds a running session.

## Artboards

| Artboard | ID | What it shows |
|---|---|---|
| Settings, Models, fallback path and command check, dark (named P2 in the providers work) | `O6E-0` | The settings layout of R1 with two new items in the Assistant group: "Providers" and, after it, "Models" (selected). One 760 px column. A subtitle says that sign-ins and API keys are in Providers. The "Accounts" section is gone. "Fallback path" is a bordered list: the fixed first step, the built-in row "Other accounts with the same model" with its switch, three user steps, and the fixed end row "Stop and show the error". Each step has a drag handle, a number, a logo, the model and account, the effort and a remove button. A note line on a step on another harness has an info icon. A skipped step has a ring and its reason. The table "How the path works" follows, with a row "Where work runs". Then "Command check": a source select ("A model of an account"), the model that Consort chose ("Haiku 5.5, Claude Work"), and a Test button with its result line. A second table, "How the check works", ends the page. The artboard is 1648 px high. |
| Edit the path: each state, dark | `OMA-0` | A state sheet in two columns. The empty path with "Add a step". The add-step picker open, in the existing model picker with its rail, and the models that are in the path marked. A drag in progress: the lifted row, and an accent dashed slot where it will land. The right-click menu of a step. The confirm dialog when the first added step is on another harness. The picker lists accounts with their plan, and no workers. The panel "Menu of an account" left with the Accounts section. It is now in P3 of the providers work. The artboard is about 1456 px high. |
| Assistant moved to a fallback, composer chip open, dark | `P5A-0` | A full window, 1440 by 900. The timeline has the notice "Claude Personal reached its usage limit, so the assistant moved to Codex Work, GPT-5.5. It returns at 4:10 PM." with "Edit path". The composer chip shows the fallback model with a marker. The chip menu is open, with the line "Go back to Opus 5.5". The menu opens upward, because the chip is at the bottom edge. |
| Outcomes: moved, returned, every step failed, channel row, dark | `PFQ-0` | A sheet in two columns. The task header shows the model and the effort, with no worker. Left: three notices (moved with the time of return, moved with "Go back now" after the reset, returned on its own), and a 400 px task thread where the task moved and stays, with Details open. Right: the error card "No model in the path could answer" with one line for each step and the reason (a step is skipped for a usage limit or a signed-out account, never for a worker), and the actions "Choose another model", "Edit path" and "Try again". Then an excerpt of the channel settings dialog with the row "Fallback path: Same as Tutti". The artboard is about 1075 px high. |

The artboard B3 did not change in the revision. Its composer line "Runs on bigboi" stays, because it says where the tools run.

## Decisions the plan did not settle

- There was no plan file. The proposal came in the request. These notes list each choice that the proposal left open.
- The path lives in a new first item "Models" of the Assistant group, not in Hub. A fallback path is about which model answers, so it sits with the accounts that supply the models. The old Hub switch moves into the path as the built-in row, so there is one place for the rule.
- Settings > Providers replaces the account rows in Settings > Workers. The first draft of this design put the accounts in Models. The owner then split the two ideas: Models holds choices, and Providers holds sign-ins, harness tools and keys. The hub owns the accounts, so a list of accounts under a worker is wrong. Settings > Workers keeps the machines: name, tags, state, pause and remove. The account menu moves with the rows. It has Account settings, Rename, Sign out and Remove, as in the CLAUDE.md list for an account row, and the same order.
- The account list (now in Providers) shows the plan and the email, and no worker. The signed-in state is a dot with a word: a `status.done` dot for "Signed in", a `status.failed` dot for "Signed out". A signed-out account has a "Sign in" button in its row. There is one "Add account" button, at the top of the Providers page. See [design-providers.md](design-providers.md).
- The Command check section sits after "How the path works", because the path is the older and bigger setting. It is a different rule from the path, so it has its own section and its own table.
- Sign out keeps the account in the list and Remove deletes it. A path step that uses a removed account is skipped, and the row shows the reason. The design does not draw the removed state.
- A signed-out account is skipped, and the step shows "Skipped now. This account is signed out." This is different from a sign-in problem of the first choice during a turn. That case is not a trigger, and the error card says Sign in again. The two rules are in "How the path works".
- The table "How the path works" has a row "Where work runs". It says that channel tags still choose the worker of a task, and that an offline worker stops that task, not the model choice.
- The built-in row has a switch. Its off state keeps the old behavior of the Hub switch off. The first on state keeps DECISIONS 752: a chat or a task goes on with another signed-in account of the same harness and the same model.
- The built-in row always comes right after the first step. A model on another account is the cheapest move, so it is tried first. The user cannot reorder it.
- The first step is fixed and shows the line "First choice. This is the model of the channel." with a lock. It has no handle and no remove button, so the lanes of the handle and the remove button stay empty on that row. The lanes keep their width, so the columns line up across rows.
- A step on another harness does not need a different style. It has a note line with an info icon in `accent.text`: "Another harness. The work and its context go to OpenAI." The note shows in the list and in the confirm dialog.
- The confirm dialog is a 520 px dialog with the xl radius. It appears only when the first step on another harness is added, not on each one. The buttons are "Add step" and "Cancel".
- In the add-step picker, a model that is already in the path has a label with its step, for example "Step 2". The user can still pick it with another account. The same account and model twice is blocked, with the label. The picker rows and the account headers list the plan and no worker.
- The effort is part of a step, in a fixed 84 px lane, like the effort of the model picker.
- A skipped step shows a hollow ring and its reason in `text.secondary`. A step that failed shows a filled dot. The dot colors are the same in the settings list, the activity detail and the error card.
- The marker for a fallback model on the composer chip is a small dot in `accent.text` at the corner of the logo. The tooltip says why. The menu has "Go back to <model>". The marker is not a text label, because the chip is 28 px high.
- The moved notice is a centered UI-size line in `text.tertiary`, as in E2. It has the swap icon in `accent.text`, and the actions "Edit path" and "Details". "Go back now" shows only after the limit has reset and before the assistant has gone back on its own.
- The returned notice is a second line, with the undo icon in `status.done`. It says "The limit of Claude Personal has reset, so the assistant went back to Opus 5.5."
- The task thread notice says that the task stays where it moved, because the task holds a running session. This matches DECISIONS 752. The task header shows the new model. It has no "Go back" action.
- The error card follows E1 and DECISIONS 713: a title, one body paragraph, and the actions. The step lines sit between the body and the actions. The title says what happened. The body says what to do next.
- Details opens the activity detail with one line for each step that Consort tried or skipped. The text of each line has the account, the model, and a reason in plain words.
- A 3 tries count appears in the reason for an outage. The number is example data. The plan sets no retry count. See the questions.
- The settings page and the dialog excerpt show the path as selectable only where the user may copy it. The rows are chrome and are not selectable. The error card step lines are selectable, because they are error details.
- The picker, the chip menu and the right-click menu flip: they open downward when there is room and upward when there is not.
- The step list fades at its top and bottom edges when it scrolls inside a panel or a dialog. The design shows it at rest, with no fade.

## Proposed right-click menu for a step row

This menu is new. It is not in the CLAUDE.md list. If the user accepts it, add it to the GPUI rules in the same change.

- A step in the path: Move up, Move down, Remove.
- Move up is off on the first user step. Move down is off on the last user step.
- The first step, the built-in row and the end row have no menu.

The keyboard way to reorder is the same two items, because a drag is hard with a keyboard.

## Affected records

The revision changes the architecture. These records are affected. They are not changed here.

- DECISIONS 752. It moves a turn to another signed-in account of the same harness and model, with a switch on the Hub page. The design widens it to a user path, and moves the switch into Settings > Models. Row 752 needs a new text.
- ADR 0004, "credentials stay on workers". The hub now owns the sign-ins, so the ADR no longer holds. It needs a new ADR that replaces it.
- ADR 0010, "worker accounts sign in and tags". Accounts leave the worker. Tags stay with the worker and still choose where a task runs. The ADR needs a split: accounts move to the hub, tags stay.

## Doubts about the proposal

- One global path may be too coarse. A coding channel and a chat channel may want different paths. The design leaves room for a path of its own in a channel, but does not draw its editor.
- A usage limit and an outage are two causes with two kinds of user risk. A step on another harness may be fine for a limit and wrong for an outage of the same provider. The design treats them the same.
- "It returns at 4:10 PM" depends on a reset time from the provider. DECISIONS 753 says the hub holds a limit until the reset or one hour. If the provider gives no time, the notice has no "It returns at" sentence. The design does not draw that case.
- Going back on its own may surprise the user in the middle of a thread, because the model and its style change. The notice for the return is the only warning. The user may want a setting to stay.
- A first step on another harness sends context to another provider. The confirm covers the add, but not each move. The notice names the provider when the assistant moves, so the user can see it.
- The skip rule for the approval mode needs each harness to say what it can enforce. The harness docs do not list this yet.
- A hub that holds every sign-in is one place to lose. A hub that has the harness CLIs runs them for a task on a worker. The design does not say how a task on a worker uses an account of the hub. This is a question for the architecture, not for this screen.

## Conflicts

- DECISIONS 752 says the move happens only inside the same harness and the same model, and that the task keeps the account. This proposal widens it to other models and harnesses by user choice. Update row 752 if the user accepts the design.
- `settings_hub.rs` has the switch "Move to another account at a usage limit" with its hint. The design removes the switch. The code is not changed.
- Settings > Workers shows account rows today. The design takes them out and puts them in Settings > Providers, not in Settings > Models. The code is not changed.
- CLAUDE.md lists the right-click menu "An account row (Settings > Workers)". The design moves that row to Settings > Providers. Update the line if the user accepts the design.

## Answered by the user (2026-10-30)

- One global path for the first build. A channel row shows "Same as Tutti" only.
- The assistant goes back on its own at the reset, with a notice. A task stays where it moved.
- An outage counts as a trigger after 3 failed tries.
- A step on another harness asks for a confirm only when the user adds the first one. A move does not ask. The notice names the provider.
- The settings item "Models" holds the choices: the fallback path and the classifier model (the "Command check"). A new item "Providers" holds the harness CLIs, the accounts, the sign-ins and the API keys. Settings > Workers holds only machines. B1 changed to match. The Accounts section is out of B1.
- A signed-out account in the path shows a timeline notice when a move skips it. Settings always shows it.
