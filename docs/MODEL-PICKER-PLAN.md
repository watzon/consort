# Plan: model picker, coordinator model per channel, and effort

Status: asked for by the user on 2026-10-29. Paper artboards (Desktop page): "Model picker with rail, dark", "Model picker, Favorites, dark" and "Effort menu in the composer, dark". The rail and Favorites design replaced the first design ("Model picker in the composer, dark", with sticky group headers and a footer) on 2026-10-29.

## What the user asked for

1. A way to change the model that the assistant (the coordinator) uses: one default for everything, and one per channel. The choice stays (it is stored on the hub).
2. The model list in the New channel dialog is a long flat menu that runs off the screen. Replace it with a searchable picker that scrolls inside itself and groups models by account, so two accounts of one provider (for example Claude Personal and Claude Work) are separate.
3. One picker component, used in the composer and in the New channel dialog (and anywhere else a model is chosen).
4. Reasoning effort: show the levels that the provider reports for the chosen model, and let the user choose one.

## Data that already exists

- `Channel.coordinator_selection` and `Channel::effective_coordinator_selection`: a channel without its own selection uses the Everything channel's selection. `channel.update` already takes `coordinator_selection` and `task_selection`.
- `ModelSelection { account_label, model, options }`. Effort is a model option (`options["effort"]`).
- `HarnessAccount.models: Vec<ModelInfo>`, and `ModelInfo.options: Vec<OptionDescriptor>`. Codex reports `effort` from `model/list`. ACP adapters report it from session config options (`thought_level` or `effort`). Check that Claude Code (claude-agent-acp) and OpenCode really report it with the managed tools, and fix the adapter where they do not.

## Picker component (desktop)

One component, `ModelPicker`, with a trigger and a popover.

- **Trigger:** the provider logo, the model name in medium weight, and the account name in muted text. A chevron follows. In the composer it is a compact chip, and in dialogs it is a full-width select field.
- **Popover:** a rail at the left and a main pane. It flips up or down by the room it has (AGENTS.md rule 4). It has a fixed size: `component.model_picker` wide and `component.model_picker_height` high.
  - **Rail:** Favorites (a filled star) at the top, a divider, then one button per signed-in account label, in the order of the inventory, with the provider logo. The selected button has the selected background and a short accent bar on the rail's right edge. The pointer on a button shows a tooltip with the account name and its workers.
  - **Badges:** when two or more accounts have the same provider, each of their buttons has a small badge with a short label from the account names. The rule is the pure function `picker::badge_labels`: keep the words that no other account of that provider has (a number joins the word before it), take the first letter of each kept word in upper case plus the digits at the end of the word, and cut it to 3 characters. "Claude Code Personal 1" and "Claude Code Personal 2" give P1 and P2, and "Claude Work" gives W. An account with no word of its own, or a label that two accounts share, gets its position (1, 2).
  - **Main pane:** a search field ("Search every account"), then a header line, then the rows. In an account, the header line has the account name in semibold and its workers in muted text, and each row has the model name, the effort range ("Low to Max effort", from the first and last choice labels), a star and a check for the selected model. The header line is outside the list, so it stays under the search field while a long list scrolls. The list scrolls inside the popover with the top and bottom fades of AGENTS.md rule 3.
  - **Favorites:** the rows of every account that the user starred, in the order they were starred, each with the provider logo and the account name in muted text after the model name. The list ends with "Star a model in any account to add it here.", which is also the empty state. A favorite whose account is not signed in now, or whose account no longer offers the model, stays stored but has no row.
  - **Stars:** a favorite has a filled star in the accent. Another row shows an outline star only while the pointer or the keyboard is on it. A click on the star adds or removes the favorite. It does not choose the model and does not close the popover.
  - **First view:** Favorites when at least one favorite has a row, else the account of the current selection.
  - **Search:** typing hides the rail and shows the matching models of every account, as rows of the Favorites kind. It filters by model name, model ID, account name and provider name. Escape clears the search first, then closes only the popover.
  - **Keyboard:** Up and Down move in the list, and Enter chooses. Left, with an empty search, moves to the rail. In the rail, Up and Down change the view, and Right, Enter or typing go back to the list.
  - **Use the Everything model:** a small text button at the right end of the header line, in every view, for a channel that has its own model. It clears the channel's own choice.
  - **No signed-in account:** "No account is signed in. Add one in Settings > Workers." with a button that opens it. There is no rail.
- **Effort control:** a second chip next to the model chip (bars icon plus the level), with a small menu titled "Effort for <model>". It lists the choices of the model's `effort` option, marks the provider default with "Default", and ends with the note "<Provider> offers these levels for this model. More effort thinks longer and uses more of your plan." It is hidden when the model has no effort option. In dialogs the effort control is a select next to the model field.
- Changing the model keeps the effort if the new model offers that level. Otherwise it falls back to the new model's default.

## Where it is used

- **Composer in a channel (and in Everything):** the chips show the channel's effective coordinator selection. Choosing a model or an effort sends `channel.update { coordinator_selection }` for that channel at once. In Everything this sets the default for every channel without its own model. The choice applies to the next coordinator turn.
- **New channel dialog:**
  - "Model for the assistant" defaults to "Same as Everything".
  - "Default account for tasks" becomes "Model for tasks", with effort.
  - Both use the same component.
- **Channel row menu:** "Model" opens the same picker for the channel's coordinator.
- **Task thread composer:** shows the task's model as read-only text. A task's model is set when the task starts.

## Hub

- When `coordinator_selection` changes for a channel, its next coordinator turn uses the new selection. If a coordinator session is open with another account or model, end it after the turn that runs now (do not cut a running turn) so that the next turn starts a session with the new selection. The coordinator's context comes from the hub, so nothing is lost. Do the same for channels that use the Everything default when Everything's selection changes. If a harness can switch model inside a session (ACP `session/set_config_option` or Codex per-turn model), you may use that instead, but test both paths.
- Pass `options` (effort) from the selection to `session.start` and to each turn where the adapter supports it. Check the path end to end.
- Placement: a selection names an account label and a model. Only workers where that account is signed in and offers that model qualify, and channel tags still apply.

## Favorites on the hub

- Favorites are a user preference that every client shares. The proto has `FavoriteModel { account_label, model }` and a `Preferences { favorite_models }` record (entity kind `preferences`, feed ID `preferences`, no channel).
- `preferences.set_favorite_model { account_label, model, favorite }` adds or removes one favorite and returns `PreferencesResult { preferences }`. One favorite per request, so two clients that star at the same time do not undo each other.
- The hub stores the record as JSON in the `meta` table (key `preferences`), so no migration is needed. A change sends a feed upsert with the whole record. The `all` snapshot has `preferences`.
- The client applies the snapshot and the feed, and the view state has `favorite_models`. A favorite whose account or model is gone stays stored and is hidden in the picker.

## Client

- Model the picker data in `consort-client` view state: groups (account label, provider kind and logo key, workers, models with id, name and effort choices), the current selection per channel (effective, with a flag that says whether it comes from the channel or from Everything), and the search filter as a pure function with tests.
- Remove the old flat `selections: Vec<String>` once nothing uses it.

## Tests

- Client: grouping by account label across workers, signed-out accounts left out, search filtering, effort fallback when the model changes, and the effective selection with its source. The badge rule, the rail order, the first view, Favorites with a gone account or model, a search across accounts, and the cache that applies the preferences from the snapshot and the feed.
- Proto: the round trip of `preferences.set_favorite_model`, its result, the `preferences` feed entry and the snapshot field.
- Hub: the hub stores favorites, a request that changes nothing sends nothing, and a change sends a feed upsert that reaches another client.
- Hub: a selection change ends the open coordinator session after the running turn, the next turn starts with the new model and effort, and the Everything default applies to channels without their own.
- Desktop: the picker opens, filters, scrolls, chooses, and sends the right `channel.update`. The rail takes the arrow keys, typing searches, Escape clears the search first, a click on a star does not choose the model, and a rail button shows its tooltip. Use the existing headless window tests.
- A live check: `scripts/dev.sh` with a signed-in account, choose a model and an effort in the composer, send a message, and confirm in the hub log that the session started with them.
