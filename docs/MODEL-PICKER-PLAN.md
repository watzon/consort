# Plan: model picker, coordinator model per channel, and effort

Status: asked for by the user on 2026-10-29. Paper artboards (Desktop page): "Model picker in the composer, dark" and "Effort menu in the composer, dark".

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
- **Popover:**
  - A search field at the top. Typing filters by model name, model ID, account name and provider name. Arrow keys move, Enter chooses, Escape closes only the popover.
  - Below the search field, one group per account label, in the order of the inventory, with signed-in accounts only. The group header has the provider logo, the account name, and the workers that have that account signed in, for example "work-laptop, work-desktop". The models are the union of the models of that account on those workers.
  - The selected row has the selected background and a check. A model with effort levels shows "Low to Max effort", using the first and last choice labels.
  - The list has a fixed maximum height (a new size token) and scrolls inside the popover, with the top and bottom fades of AGENTS.md rule 3. The popover flips up or down by the room it has (rule 4).
  - The group headers are sticky. While the list scrolls, the header of the group whose models are in view sticks to the top of the list, directly under the search field. The next group's header pushes it up and replaces it as it arrives. The sticky header has the popover background, so rows do not show through, and the top fade of the list goes under it, not over it.
  - A footer line says what the choice changes. It is one of:
    - "The assistant in Everything uses this model. Channels without their own model use it too."
    - "The assistant in <channel> uses this model."
    - "Tasks in <channel> start with this model."
  - In a channel that has its own model, the footer has "Use the Everything model" to clear the channel's own choice.
  - No signed-in account: "No account is signed in. Add one in Settings > Workers." with a button that opens it.
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

## Client

- Model the picker data in `consort-client` view state: groups (account label, provider kind and logo key, workers, models with id, name and effort choices), the current selection per channel (effective, with a flag that says whether it comes from the channel or from Everything), and the search filter as a pure function with tests.
- Remove the old flat `selections: Vec<String>` once nothing uses it.

## Tests

- Client: grouping by account label across workers, signed-out accounts left out, search filtering, effort fallback when the model changes, and the effective selection with its source.
- Hub: a selection change ends the open coordinator session after the running turn, the next turn starts with the new model and effort, and the Everything default applies to channels without their own.
- Desktop: the picker opens, filters, scrolls, chooses, and sends the right `channel.update`. Use the existing headless window tests.
- A live check: `scripts/dev.sh` with a signed-in account, choose a model and an effort in the composer, send a message, and confirm in the hub log that the session started with them.
