# Design notes: phases 1 and 2

Paper file `01M6342D512B1CKH0D29HMDN8V`, page Desktop. The artboards are at worldY 3200, 4180 and 5160, from worldX -720. The example channel is house-lights. The chat screens use "Base: Tutti with top tint and larger sidebar, dark" (`76M-0`) as their base.

## Artboards

### Phase 1: files and attachments

| Artboard | ID | What it shows |
|---|---|---|
| Composer with attachments, dark | `7D6-0` | The composer with two image chips (one with the remove badge on hover), a file chip, a chip that uploads with a progress bar, and a failed chip with Try again. |
| Drop files on the composer, dark | `7MW-0` | Files dragged over the window. The composer field becomes the drop zone. The timeline does not change. |
| Bubbles with attachments, dark | `7VV-0` | Your bubbles with two photos, a file and text. Assistant bubbles with an artifact chip that names its task (hover state with Save and Open) and a file from channel files. |
| Image light box, dark | `8EA-0` | One image opened large, with its name and details, Save, Open, close, and previous and next. |
| Channel panel, Files, dark | `8PM-0` | The new Channel panel on the right, Files tab: folders, files with size and date, an upload in progress, artifacts grouped by task, the row menu, and the drop hint. |
| Channel panel empty states, dark | `9FL-0` | The empty states of the Files, Routines and Memory tabs. The empty Files tab is also its drop zone. |
| Task thread reply with an attachment, dark | `9QD-0` | The task thread in the Tasks panel. The reply box has an image chip. The add menu opens upward. |

### Phase 2: routines and memory

| Artboard | ID | What it shows |
|---|---|---|
| Channel panel, Routines, dark | `AFH-0` | The Routines tab: name, schedule in words, action, last result, next run and the switch. A row on hover shows Run now, Run history and the row menu. |
| Routine run history, dark | `AZ7-0` | A routine opened in the panel: the summary, Run now, Edit, and the run history with succeeded, failed and skipped runs and links to messages. |
| Routine dialog, new, dark | `BLH-0` | The create dialog: name, channel, Ask the assistant with a prompt, the schedule presets, time and time zone, the next runs preview, and the model and effort chips. |
| Routine dialog, edit with errors, dark | `BYU-0` | The edit dialog: Start a task with title and brief, a cron with a validation error, a disabled Save, and Delete routine. |
| Routine schedule picker states, dark | `CK3-0` | The schedule field in six states: every day, weekly on chosen days, interval, once, once with a time in the past, and cron. |
| Settings, Routines, dark | `CRP-0` | Every routine of every channel, grouped by channel, with a text filter, a status filter and a channel filter. The settings rail uses the larger scale. |
| Channel panel, Memory, memory saved line, dark | `D8N-0` | The Memory tab as a tree of memory notes. The timeline shows the quiet line "Updated the memory note ..." with a link to the note. |
| Memory note, edit with conflict, dark | `DNN-0` | A channel memory note in Edit mode in the panel, with the stale-write message, Load the new version and Save my version. |
| Settings, Memory, dark | `E0W-0` | Global memory: the note tree, a note in rendered Markdown, Edit, and the note menu (Version history, Copy path, Rename, Delete). |
| Settings, Memory, version history, dark | `EAE-0` | The version list of a note, a preview of an old version with changed lines marked, and Restore this version. |

## Change after review

- The first drag-over design put a dashed overlay over the whole timeline. The user did not like it. Now the composer field itself becomes the drop zone. The field grows by about two lines and gets a dashed accent border and a soft accent fill. The upload icon, "Drop to attach 3 files" and "Up to 256 MB each" sit in the center. Text that you typed stays visible in the field.
- A release anywhere over the chat attaches the files, not only a release over the composer. The composer is the only visual target. This prevents a lost drop when the pointer is a few pixels off.

## Decisions that the plan did not settle

- One new top action, Channel, with the `channel` (hash) icon, sits to the left of Tasks.
- The Channel panel and the Tasks panel share the right side. Only one is open at a time. A click on Channel while Tasks is open replaces Tasks, and the other way around. Each button shows `surface.selected` while its panel is open.
- The Channel panel is 400 px wide, the same as the Tasks panel, with the same header, tabs and row styles.
- The Channel panel header shows the channel name and its purpose, because the channel has no header now.
- The Channel panel opens a routine or a memory note in place (drill-in with a back button), the same as a task thread in the Tasks panel.
- The add button in the composer and in the reply box opens a menu: Upload from this computer, and Choose from channel files. Paste and drop also attach files.
- An image attachment is a thumbnail chip. Every other file is a chip with a tile, the name and the type and size.
- Your images show as their own tiles above your text bubble. Files show as file bubbles in your lane.
- An artifact chip in an assistant bubble shows the status glyph and the name of its task, and the size. On hover it shows Save and Open.
- An assistant attachment from channel files is a plain file bubble. It does not name a task.
- A failed upload says what happened ("Upload failed. The hub is offline.") and offers Try again.
- The Files tab lists folders first, then files, then a section Artifacts with one group per task.
- Artifacts are files under `artifacts/<task-id>/`, so the group shows the task name and its status, not the folder name.
- The file row menu is Open, Download, Rename, Move, then Delete in `status.failed`, in this fixed order.
- File dates use "Today" for today, else the short date.
- A routine row has three lines: the name, the schedule in words with the action ("asks the assistant" or "starts a task"), and the last result with the next run.
- A routine that is off shows "Off. It does not run until you turn it on."
- The routine dialog calls the two actions "Ask the assistant" and "Start a task".
- The schedule presets are Every day, Weekdays, Weekly, Interval, Once and Cron. Every preset shows the cron that it makes, so experts can learn it.
- Interval runs count from the time you save the routine and use no time zone. The least interval is 1 minute, as in the data model.
- A once routine turns off after its run and stays in the list.
- The time zone starts as the time zone of this computer.
- The model field of a routine with "Ask the assistant" says "Same as house-lights" and uses the channel's model. A routine with "Start a task" has its own model and effort.
- Save stays off while a field has an error. Each error sits under its field, in `status.failed`, and says what to change.
- The run history shows the time, the result, the duration and a link (Message or Task). A failed run shows the error as selectable text with the next step. A run from Run now has the label "Run now".
- A run that would start while the last run of the same routine still runs is skipped.
- Settings > Routines filters by text (name or prompt), by status (All, On, Off, Last run failed) and by channel.
- Memory notes show as a tree by path. Folders open and close. A note name is in mono.
- The panel shows channel memory. Settings > Memory shows only global memory, which is Tutti's channel memory.
- A note that the assistant just changed shows an accent dot and "Just now" in the tree.
- The memory saved line sits in the assistant lane under the last bubble, in Label size and `text.tertiary`, with the `memory` icon and the note path as a link. When the note came from a memory suggestion, the line names the task that suggested it. This is how the user sees memory suggestions.
- The stale-write message uses `status.failed` and keeps your text. It offers Load the new version and Save my version. Save stays off until you choose.
- Restore saves the old text as a new version. No version is lost.
- The version preview marks changed lines with a bar in `status.working`, not with red or green, because a change is not an error.
- Delete of a memory note or a channel file asks for confirmation in a small dialog, as other deletes do. That dialog is not drawn.
- The icons are the hand-drawn set. The path numbers are rounded to one decimal place. At 14 to 20 px the difference does not show.
- A few small glyphs that the set does not have use plain strokes: the channel bulb, the select chevrons, the calendar in the date field and the play triangle.
- The lists in these screens fit their space. When a list is longer, it scrolls inside its panel with the top and bottom fades. The fades are not drawn.

## Questions for the user

1. Is a hash icon right for the Channel button, or do you want a panel icon?
2. Should the Channel panel open by default the first time you open a channel that has files or routines?
3. Should a routine with "Ask the assistant" have its own model, or always use the channel's model? The data model has `model_selection` only for `start_task`.
4. The data model has `last_run_status` as `ok` or `error`. Do you want a third status, `skipped`, for a run that did not start?
5. When the hub is offline at a run time, should the routine run once when the hub comes back, or skip the run?
6. Should Settings > Memory also list the memory of each channel, or only global memory?
7. Can a deleted memory note be restored from its history, or does Delete remove the history too?
8. Do you want a size limit for one attachment that is lower than the 256 MiB blob limit?
9. Should a dropped file also go into channel files, or stay only on the message?
